use etc_os_release::OsRelease;
use git2::Repository;
use std::path::Path;
use std::process::Command;
use std::{fs, str::FromStr};
use sys_mount::{Mount, MountFlags};
use thiserror::Error;

mod partitions;

use crate::OverlayExtError::{BuildInit, Io, OsReleaseMissing};
use crate::partitions::find_uuid_from_name;

// FIXME horrible hack, necessary for now because of the way mkosi handles the pacman package db
const PACMAN_DB_PATH: &str = "lib/sysimage";

// Github repo for my mkosi build files
const OVERLAY_EXT_SKELETON: &str = "https://github.com/Smujb/overlay-ext-skeleton";

// Must use /dev/mapper/usr to refer to the active deployment as otherwise it cannot be mounted as it is busy
const ACTIVE_DEPLOYMENT_BLOCK: &str = "/dev/mapper/usr";

// Paths used by the program
const USR_DIR: &str = "mkosi.base/usr";
const VAR_DIR: &str = "mkosi.base/var";
const EXTENSIONS_DIR: &str = "/var/lib/extensions/";

// User-provided configuration files for the mkosi build
const MKOSI_CONFIG_LOCATION: &str = "/etc/overlay-ext/mkosi";

// FIXME un-hardcode this
const MKOSI_LOCATION: &str = "/opt/mkosi/bin/mkosi";

const OVERLAY_EXT: &str = "overlay-ext";

// Errors thrown by this program
#[derive(Error, Debug)]
pub enum OverlayExtError {
    #[error("Could not find UUID for device {0}")]
    NoUuid(String),
    #[error("CacheBuilderError: {0}")]
    CacheBuilder(#[from] rsblkid::cache::CacheBuilderError),
    #[error("CacheError: {0}")]
    Cache(#[from] rsblkid::cache::CacheError),
    #[error("IoError: {0}")]
    Io(#[from] std::io::Error),
    #[error("Git Error: {0}")]
    Git(#[from] git2::Error),
    #[error("Error initializing sysext build!")]
    BuildInit,
    #[error("Error reading os-release file: {0}")]
    OsRelease(#[from] etc_os_release::Error),
    #[error("os-release file does not contain: {0}")]
    OsReleaseMissing(String),
    #[error("FromUtf8Error: {0}")]
    FromUtf8(#[from] std::string::FromUtf8Error),
    #[error("ProbeBuilderError: {0}")]
    ProbeBuilder(#[from] rsblkid::probe::ProbeBuilderError),
    #[error("ProbeError: {0}")]
    Probe(#[from] rsblkid::probe::ProbeError),
}

pub type OverlayExtResult<T> = std::result::Result<T, OverlayExtError>;

// Get the name of the output before building
fn output_name(image_version: &str) -> String {
    format!("{OVERLAY_EXT}_{image_version}")
}

// Set up the necessary files for the sysexts to build
fn setup_builds(deployments: Vec<String>) -> OverlayExtResult<()> {
    let host_os_release = OsRelease::open()?;
    let version = host_os_release.image_version();

    // We can't have a missing image version on the host
    if version.is_none() {
        return Err(OsReleaseMissing("IMAGE_VERSION".to_string()));
    }
    let version = version.unwrap();

    println!("Temporarily unmerging sysexts.");
    Command::new("systemd-sysext").arg("unmerge").output()?;

    const WORKDIR: &str = "/tmp/overlay-ext";
    println!("Cloning repository {OVERLAY_EXT_SKELETON} into {WORKDIR}...");
    let _ = Repository::clone(OVERLAY_EXT_SKELETON, WORKDIR)?;

    // Grab our configuration from /etc
    println!("Pulling in local configuration from {MKOSI_CONFIG_LOCATION}...");
    dircpy::copy_dir(MKOSI_CONFIG_LOCATION, format!("{WORKDIR}/mkosi.local"))?;

    // Run the builds and store the output
    let builds_status = run_builds(deployments, WORKDIR, version);
    println!("Cleaning up {WORKDIR}...");
    fs::remove_dir_all(WORKDIR)?;

    // Load new sysexts
    println!("Merging sysexts again.");
    Command::new("systemd-sysext").arg("merge").output()?;

    builds_status
}

// Run each of the builds in turn and clean up after
fn run_builds(deployments: Vec<String>, workdir: &str, min_version: &str) -> OverlayExtResult<()> {
    let mut build_status = Err(BuildInit);

    // Now we run the actual build
    for deployment in deployments {
        println!();

        // Build the sysext, and store whether it succeeded
        build_status = build_sysext(deployment.clone(), workdir, min_version);

        // Unmount /usr and wipe /var after the build
        println!("Cleaning up build for {}...", deployment.clone());
        Command::new("umount")
            .arg(format!("{workdir}/{USR_DIR}"))
            .output()?;
        fs::remove_dir_all(format!("{workdir}/{VAR_DIR}"))?;

        if build_status.is_err() {
            break;
        }
    }

    // Return the status of the build to propagate the error
    build_status
}

// Build an individual sysext
fn build_sysext(deployment: String, workdir: &str, min_version: &str) -> OverlayExtResult<()> {
    let usr_dir = format!("{workdir}/{USR_DIR}");
    let var_dir = format!("{workdir}/{VAR_DIR}");

    println!("Starting system extension build for: {deployment}");

    // Create the build environment
    println!("Mounting /usr...");
    fs::create_dir_all(Path::new(&usr_dir))?;
    fs::create_dir_all(Path::new(&var_dir))?;
    Mount::builder()
        .flags(MountFlags::RDONLY)
        .mount(deployment.clone(), &usr_dir)?;

    println!("Checking os-release file");

    // Check the version of the deployment before running the build
    let deployment_os_release = match fs::read_to_string(format!("{usr_dir}/lib/os-release")) {
        Ok(filepath) => OsRelease::from_str(&filepath).unwrap(), // Use unwrap here as the error is "Infaillible"
        Err(error) => {
            // Tell the user which disk/partition is causing the issue
            println!("Failed to find os-release file for {deployment}");
            return Err(OverlayExtError::Io(error));
        }
    };

    let deployment_version = match deployment_os_release.image_version() {
        Some(version) => version,
        _ => {
            println!("Could not find image version for {deployment}, skipping.");
            return Ok(());
        }
    };

    if deployment_version < min_version {
        println!("Skipping {deployment} as it is an older version than the booted one.");
        return Ok(());
    }

    let mkosi_output_name = output_name(deployment_version);

    // Copy the DB path for pacman into /var so it can be used by overlay-ext
    println!("Copying package db...");
    dircpy::copy_dir(format!("{usr_dir}/{PACMAN_DB_PATH}"), format!("{var_dir}/"))?;

    // Actually run the mkosi build
    println!("Running mkosi build...");
    match Command::new(MKOSI_LOCATION)
        .arg("--image-id") // We hardcode these parameters in here so that this program is aware of exactly what the output will be called
        .arg(OVERLAY_EXT)
        .arg("--image-version")
        .arg(deployment_version)
        .arg("--output")
        .arg(mkosi_output_name)
        .current_dir(workdir)
        .spawn()
    {
        Err(error) => {
            return Err(Io(error));
        }
        Ok(child) => {
            child.wait_with_output()?;
        }
    };

    // Find and copy the output of it
    for entry in fs::read_dir(format!("{workdir}/mkosi.output"))? {
        let entry = entry?;

        // Copy if it is a file that ends in .raw
        if !entry.path().is_dir()
            && entry
                .path()
                .extension()
                .map(|s| s == "raw")
                .unwrap_or(false)
        {
            println!(
                "Copying extension {} to {EXTENSIONS_DIR}",
                entry.path().display()
            );
            fs::copy(
                entry.path(),
                format!("{EXTENSIONS_DIR}/{}", entry.file_name().display()),
            )?;
        }
    }

    Ok(())
}

fn main() {
    let cache = partitions::generate_cache().expect("Failed to generate rsblkid cache!");
    let disk_names =
        partitions::find_usr_partitions().expect("Failed to probe partitions for /usr partitions!");
    // We always want to process the active deployment
    let mut blocks_to_process: Vec<String> = vec![ACTIVE_DEPLOYMENT_BLOCK.to_string()];
    let active_deployment_uuid = find_uuid_from_name(&cache, ACTIVE_DEPLOYMENT_BLOCK)
        .expect("Unable to find UUID of the active deployment.");

    // As well as any other blocks known to contain deployments that are **not** the active one
    for disk_name in disk_names {
        let block_uuid = find_uuid_from_name(&cache, &disk_name);
        if active_deployment_uuid.as_str_safe() != block_uuid.unwrap().as_str_safe() {
            println!("Adding deployment {disk_name}...");
            blocks_to_process.push(disk_name);
        }
    }

    println!("Found the following devices: {:?}", blocks_to_process);

    setup_builds(blocks_to_process).expect("Failed to build sysexts!");
}

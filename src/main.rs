use etc_os_release::OsRelease;
use git2::Repository;
use rsblkid::{cache::Cache, device::TagName, partition::RawBytes};
use std::os::unix;
use std::path::Path;
use std::process::Command;
use std::{fs, str::FromStr};
use thiserror::Error;

use crate::OverlayExtError::{BuildInit, Io, OsReleaseMissing};

// FIXME horrible hack, necessary for now because of the way mkosi handles the pacman package db
const PACMAN_DB_PATH: &str = "lib/sysimage";

// Github repo for my mkosi build files
const OVERLAY_EXT_SKELETON: &str = "https://github.com/Smujb/overlay-ext-skeleton";

// FIXME un-hardcode these and use a config parser instead (prob .toml)
const DEPLOYMENT_BLOCKS: &[&str] = &[
    "/dev/nvme0n1p4",
    "/dev/nvme0n1p7",
    "/dev/nvme1n1p4",
    "/dev/nvme1n1p7",
];

// Must use /dev/mapper/usr to refer to the active deployment as otherwise it cannot be mounted as it is busy
const ACTIVE_DEPLOYMENT_BLOCK: &str = "/dev/mapper/usr";

// Paths used by the program
const USR_DIR: &str = "mkosi.base/usr";
const VAR_DIR: &str = "mkosi.base/var";
const EXTENSIONS_DIR: &str = "/var/lib/extensions/";

// User-provided configuration files for the mkosi build
const MKOSI_CONFIG_LOCATION: &str = "/etc/overay-ext/mkosi";

// FIXME un-hardcode this
const MKOSI_LOCATION: &str = "/opt/mkosi/bin/mkosi";

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
}

pub type OverlayExtResult<T> = std::result::Result<T, OverlayExtError>;

// Find the UUID of a disk specified by its device name (/dev/[device])
fn find_uuid(device_name: &str) -> OverlayExtResult<RawBytes> {
    // Build cache on block devices for the system
    let mut cache = Cache::builder().discard_changes_on_drop().build()?;
    cache.probe_all_devices()?;

    // Attempt to find UUID of the disk by name
    cache
        .tag_value_from_device(TagName::Uuid, device_name)
        .ok_or(OverlayExtError::NoUuid(device_name.to_owned()))
}

// Set up the necessary files for the sysexts to build
fn setup_builds(deployments: Vec<&str>) -> OverlayExtResult<()> {
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
    println!("Linking local configuration from {MKOSI_CONFIG_LOCATION}...");
    unix::fs::symlink(MKOSI_CONFIG_LOCATION, format!("{WORKDIR}/mkosi.local"))?;

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
fn run_builds(deployments: Vec<&str>, workdir: &str, min_version: &str) -> OverlayExtResult<()> {
    let mut build_status = Err(BuildInit);

    // Now we run the actual build
    for deployment in deployments {
        println!();

        // Build the sysext, and store whether it succeeded
        build_status = build_sysext(deployment, workdir, min_version);

        // Unmount /usr and wipe /var after the build
        println!("Cleaning up build for {deployment}...");
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
fn build_sysext(deployment: &str, workdir: &str, min_version: &str) -> OverlayExtResult<()> {
    let usr_dir = format!("{workdir}/{USR_DIR}");
    let var_dir = format!("{workdir}/{VAR_DIR}");

    println!("Starting system extension build for: {deployment}");

    // Create the directories
    println!("Mounting /usr...");
    fs::create_dir_all(Path::new(&usr_dir))?;
    fs::create_dir_all(Path::new(&var_dir))?;
    Command::new("mount")
        .arg(deployment)
        .arg(&usr_dir)
        .arg("-o")
        .arg("ro")
        .output()?;

    // Check the version of the deployment before running the build
    let deployment_os_release_file = fs::read_to_string(format!("{usr_dir}/lib/os-release"))?;
    let deployment_os_release = OsRelease::from_str(&deployment_os_release_file).unwrap(); // Use unwrap here as the error is "Infaillible"
    let deployment_version = deployment_os_release.image_version();

    if deployment_version.is_none() || deployment_version.unwrap() < min_version {
        println!("Skipping {deployment} as it is an older version than the booted one.");
        return Ok(());
    }

    // Copy the DB path for pacman into /var so it can be used by overlay-ext
    println!("Copying package db...");
    dircpy::copy_dir(format!("{usr_dir}/{PACMAN_DB_PATH}"), format!("{var_dir}/"))?;

    // Actually run the mkosi build
    println!("Running mkosi build...");
    match Command::new(MKOSI_LOCATION)
        .current_dir(workdir)
        .arg("--debug-shell")
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
    // We always want to process the active deployment
    let mut blocks_to_process = vec![ACTIVE_DEPLOYMENT_BLOCK];
    let active_deployment_uuid =
        find_uuid(ACTIVE_DEPLOYMENT_BLOCK).expect("Unable to find UUID of the active deployment.");

    // As well as any other blocks known to contain deployments that are **not** the active one
    for block in DEPLOYMENT_BLOCKS {
        let block_uuid = find_uuid(block);
        if block_uuid.is_ok() && active_deployment_uuid != block_uuid.unwrap() {
            blocks_to_process.push(block);
        }
    }

    println!("Found the following devices: {:?}", blocks_to_process);

    setup_builds(blocks_to_process).expect("Failed to build sysexts!");
}

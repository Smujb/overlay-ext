use clap::Parser;
use etc_os_release::OsRelease;
use git2::Repository;
use std::path::Path;
use std::process::Command;
use std::{fs, str::FromStr};
use sys_mount::{Mount, MountFlags, UnmountFlags, unmount};
use thiserror::Error;

mod os_hacks;
mod partitions;

use crate::OverlayExtError::{BuildInit, Io, OsReleaseMissing};
use crate::os_hacks::pre_build;
use crate::partitions::find_uuid_from_name;

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

#[derive(Debug, Parser)]
#[command(version, about)]
/// Build system extensions for live systemd-sysupdate managed systems using mkosi.
struct Args {
    /// Whether to forceably rebuild the overlay for the booted deployment
    #[arg(short, long)]
    force: bool,
}

pub type OverlayExtResult<T> = std::result::Result<T, OverlayExtError>;

// Get the name of the output before building
fn output_name(image_version: &str) -> String {
    format!("{OVERLAY_EXT}_{image_version}")
}

// Set up the necessary files for the sysexts to build
fn setup_builds(deployments: Vec<String>, force: bool) -> OverlayExtResult<()> {
    let host_os_release = OsRelease::open()?;
    let version = match host_os_release.image_version() {
        Some(version) => version,
        _ => return Err(OsReleaseMissing("IMAGE_VERSION".to_string())),
    };

    let workdir = Path::new("/tmp/overlay-ext");
    println!(
        "Cloning repository {OVERLAY_EXT_SKELETON} into {}...",
        workdir.to_string_lossy()
    );
    let _ = Repository::clone(OVERLAY_EXT_SKELETON, workdir)?;

    // Grab our configuration from /etc
    println!("Pulling in local configuration from {MKOSI_CONFIG_LOCATION}...");
    dircpy::copy_dir(MKOSI_CONFIG_LOCATION, workdir.join("mkosi.local"))?;

    // Put this file into mkosi.extra
    let bin_dir = &workdir.join(USR_DIR).join("bin");
    fs::create_dir_all(bin_dir)?;
    if let Ok(path) = std::env::current_exe()
        && let Some(filename) = &path.file_name()
    {
        fs::copy(&path, bin_dir.join(filename))?;
    }

    // Run the builds and store the output
    let builds_status = run_builds(deployments, workdir, version, force);
    println!("Cleaning up {}...", workdir.to_string_lossy());
    fs::remove_dir_all(workdir)?;

    // Load new sysexts
    println!("Refreshing system extensions...");
    match Command::new("systemd-sysext").arg("refresh").spawn() {
        Err(error) => {
            return Err(Io(error));
        }
        Ok(child) => {
            child.wait_with_output()?;
        }
    };

    builds_status
}

// Run each of the builds in turn and clean up after
fn run_builds(
    deployments: Vec<String>,
    workdir: &Path,
    min_version: &str,
    force: bool,
) -> OverlayExtResult<()> {
    let mut build_status = Err(BuildInit);

    // Now we run the actual build
    for deployment in deployments {
        println!();

        // Build the sysext, and store whether it succeeded
        build_status = build_sysext(deployment.clone(), workdir, min_version, force);

        // Unmount /usr and wipe /var after the build
        println!("Cleaning up build for {}...", deployment.clone());
        unmount(workdir.join(USR_DIR), UnmountFlags::empty())?;
        fs::remove_dir_all(workdir.join(VAR_DIR))?;

        if build_status.is_err() {
            break;
        }
    }

    // Return the status of the build to propagate the error
    build_status
}

// Build an individual sysext
fn build_sysext(
    deployment: String,
    workdir: &Path,
    min_version: &str,
    force: bool,
) -> OverlayExtResult<()> {
    let usr_dir = workdir.join(USR_DIR);
    let var_dir = workdir.join(VAR_DIR);

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
    let deployment_os_release = match fs::read_to_string(usr_dir.join("lib/os-release")) {
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

    let deployment_id = deployment_os_release.id();
    let deployment_id_like: Vec<&str> = match deployment_os_release.id_like() {
        Some(version) => version.collect(),
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

    // Run pre-build os-specific hacks
    pre_build(deployment_id, &deployment_id_like, workdir)?;

    // Actually run the mkosi build
    println!("Running mkosi build...");
    let mut mkosi = Command::new(MKOSI_LOCATION);
    let mut mkosi = mkosi
        .arg("--image-id") // We hardcode these parameters in here so that this program is aware of exactly what the output will be called
        .arg(OVERLAY_EXT)
        .arg("--image-version")
        .arg(deployment_version)
        .arg("--output")
        .arg(mkosi_output_name)
        .arg("--output-directory")
        .arg(EXTENSIONS_DIR)
        .current_dir(workdir);

    // Always rebuild images that are not booted, require --force to be passed to overlay-ext itself to update the currently booted one
    // This saves time when building extensions for new versions and avoids issues with live reloading the extension in certain scenarios
    // (e.g. a daemon is running on it)
    if deployment_version != min_version || force {
        mkosi = mkosi.arg("--force");
    }

    match mkosi.spawn() {
        Err(error) => {
            return Err(Io(error));
        }
        Ok(child) => {
            child.wait_with_output()?;
        }
    };

    Ok(())
}

fn main() {
    // Grab cli args
    let args = Args::parse();

    let cache = partitions::generate_cache().expect("Failed to generate rsblkid cache!");
    let disk_names =
        partitions::find_usr_partitions().expect("Failed to probe partitions for /usr partitions!");
    // We always want to process the active deployment
    let mut blocks_to_process: Vec<String> = vec![ACTIVE_DEPLOYMENT_BLOCK.to_string()];
    let active_deployment_uuid = find_uuid_from_name(&cache, ACTIVE_DEPLOYMENT_BLOCK)
        .expect("Unable to find UUID of the active deployment.");

    // As well as any other blocks known to contain deployments that are **not** the active one
    for disk_name in disk_names {
        match find_uuid_from_name(&cache, &disk_name) {
            Ok(block_uuid) => {
                if active_deployment_uuid != block_uuid {
                    println!("Adding deployment {disk_name}...");
                    blocks_to_process.push(disk_name);
                }
            }
            _ => println!("Could not find UUID for block partition {disk_name}"),
        }
    }

    println!("Found the following devices: {:?}", blocks_to_process);

    setup_builds(blocks_to_process, args.force).expect("Failed to build sysexts!");
}

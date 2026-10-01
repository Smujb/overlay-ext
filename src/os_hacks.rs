use std::path::Path;

use crate::{OverlayExtResult, USR_DIR, VAR_DIR};

// We put the pacman package db in /usr/lib/sysimage/lib/pacman
const PACMAN_DB_PATH: &str = "lib/sysimage";

// match by ID and ID_LIKE from os-release
pub fn pre_build(id: &str, id_like: &Vec<&str>, workdir: &Path) -> OverlayExtResult<()> {
    // Run os-specific hacks for each id_like
    for similar in id_like {
        if *similar == "arch" {
            pre_build_archlinux(id, workdir)?;
        }
    }
    Ok(())
}

fn pre_build_archlinux(id: &str, workdir: &Path) -> OverlayExtResult<()> {
    // Package db is in /usr, but mkosi expects it to be in /var so we copy it across
    let src_dir = workdir.join(USR_DIR).join(PACMAN_DB_PATH);
    let dest_dir = workdir.join(VAR_DIR);
    println!(
        "Copying package db from {} to {} for {id}",
        src_dir.to_string_lossy(),
        dest_dir.to_string_lossy()
    );
    dircpy::copy_dir(src_dir, dest_dir)?;
    Ok(())
}

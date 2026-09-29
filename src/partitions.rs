use std::process::Command;

use rsblkid::{
    cache::Cache,
    device::TagName,
    partition::{PartitionTableType, RawBytes},
    probe::{Filter, Probe, ScanResult},
};

use crate::{OverlayExtError, OverlayExtResult};

// /usr partition type UUIDs according to:
// https://uapi-group.org/specifications/specs/discoverable_partitions_specification/#defined-partition-type-uuids
const USR_PARTITION_TYPE_UUIDS: &[&str] = &[
    "8484680c-9521-48c6-9c11-b0720656f69e", // x86_64 / amd64
    "b0e01050-ee5f-4390-949a-9101b17104e9", // arm64
];

// Build cache on block devices for the system
pub fn generate_cache() -> OverlayExtResult<Cache> {
    let mut cache = Cache::builder().discard_changes_on_drop().build()?;
    cache.probe_all_devices()?;

    Ok(cache)
}

// Find the UUID of a disk specified by its device name (/dev/[device])
pub fn find_uuid_from_name(cache: &Cache, device_name: &str) -> OverlayExtResult<RawBytes> {
    // Attempt to find UUID of the disk by name
    cache
        .tag_value_from_device(TagName::Uuid, device_name)
        .ok_or(OverlayExtError::NoUuid(device_name.to_owned()))
}

// Find a list of partitions which contain a Linux /usr tree according to
// https://uapi-group.org/specifications/specs/discoverable_partitions_specification/#defined-partition-type-uuids
pub fn find_usr_partitions() -> OverlayExtResult<Vec<String>> {
    // Find a list of names of disks
    let disks = Command::new("lsblk").arg("-dno").arg("name").output()?;
    let disk_names = String::from_utf8(disks.stdout)?;
    let disk_names_list = disk_names.split_terminator("\n");

    let mut disk_names: Vec<String> = vec![];

    // Probe for partition information
    let probe_builder = Probe::builder();
    for disk_name in disk_names_list {
        let disk_ref = format!("/dev/{disk_name}");
        let disk_probe_builder = probe_builder.clone().scan_device(&disk_ref);

        let mut probe = disk_probe_builder
            .scan_device_partitions(true)
            // Search for partition in the following partition tables
            .scan_partitions_for_partition_tables(Filter::In, vec![PartitionTableType::GPT])
            .build()?;

        match probe.run_scan() {
            ScanResult::FoundProperties => {
                for partition in probe.iter_partitions() {
                    // Check the partition type according to the spec
                    let partition_type =
                        partition.partition_type_string().unwrap_or("".to_string());

                    if USR_PARTITION_TYPE_UUIDS.contains(&partition_type.as_str()) {
                        // Format as /dev/nvme0n1p4 or whatever for the specific disk
                        disk_names.push(format!("{}p{}", disk_ref, partition.number()));
                    }
                }
            }
            _ => {
                // Warn that the disk found no partition info
                println!("Could not find any partition metadata for disk {disk_name}.");
            }
        };
    }
    Ok(disk_names)
}

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use crate::block::BlockDevice;
use crate::{ata, block_io, fat32, gpt, partition, swap, vfs};

const READ_ONLY_ATTRIBUTE: u8 = 0x01;
const DIRECTORY_ATTRIBUTE: u8 = 0x10;
const MAX_BOOT_IMPORT_DEPTH: usize = 2;

const MOUNT_UNKNOWN: u8 = 0;
const MOUNT_NO_DEVICE: u8 = 1;
const MOUNT_NOT_FAT32: u8 = 2;
const MOUNT_MOUNTED: u8 = 3;
const MOUNT_FAILED: u8 = 4;
const MOUNT_UNMOUNTED: u8 = 5;

static MNT_STATUS: AtomicU8 = AtomicU8::new(MOUNT_UNKNOWN);
static MNT_DIRTY: AtomicBool = AtomicBool::new(false);
static MNT_IMPORTED: AtomicUsize = AtomicUsize::new(0);
static MNT_SYNCS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountLifecycleStatus {
    Unknown,
    NoDevice,
    NotFat32,
    Mounted,
    Failed,
    Unmounted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MountInfo {
    pub status: MountLifecycleStatus,
    pub imported_entries: usize,
    pub dirty: bool,
    pub sync_count: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountControlError {
    NoDevice,
    NotMounted,
    SyncFailed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpaceError {
    Unmounted,
    NoDevice,
    NotFat32,
    Failed,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MountStatus {
    NoDevice,
    NotFat32,
    Mounted(usize),
    Failed,
}

fn status_code(status: MountStatus) -> u8 {
    match status {
        MountStatus::NoDevice => MOUNT_NO_DEVICE,
        MountStatus::NotFat32 => MOUNT_NOT_FAT32,
        MountStatus::Mounted(_) => MOUNT_MOUNTED,
        MountStatus::Failed => MOUNT_FAILED,
    }
}

fn lifecycle_from_code(code: u8) -> MountLifecycleStatus {
    match code {
        MOUNT_NO_DEVICE => MountLifecycleStatus::NoDevice,
        MOUNT_NOT_FAT32 => MountLifecycleStatus::NotFat32,
        MOUNT_MOUNTED => MountLifecycleStatus::Mounted,
        MOUNT_FAILED => MountLifecycleStatus::Failed,
        MOUNT_UNMOUNTED => MountLifecycleStatus::Unmounted,
        _ => MountLifecycleStatus::Unknown,
    }
}

fn record_mount_status(status: MountStatus) {
    MNT_STATUS.store(status_code(status), Ordering::Release);
    match status {
        MountStatus::Mounted(count) => {
            MNT_IMPORTED.store(count, Ordering::Release);
            MNT_DIRTY.store(false, Ordering::Release);
        }
        _ => {
            MNT_IMPORTED.store(0, Ordering::Release);
            MNT_DIRTY.store(false, Ordering::Release);
        }
    }
}

pub fn mount_info() -> MountInfo {
    MountInfo {
        status: lifecycle_from_code(MNT_STATUS.load(Ordering::Acquire)),
        imported_entries: MNT_IMPORTED.load(Ordering::Acquire),
        dirty: MNT_DIRTY.load(Ordering::Acquire),
        sync_count: MNT_SYNCS.load(Ordering::Acquire),
    }
}

pub fn mnt_mounted() -> bool {
    MNT_STATUS.load(Ordering::Acquire) == MOUNT_MOUNTED
}

pub fn mark_mnt_dirty() {
    if mnt_mounted() {
        MNT_DIRTY.store(true, Ordering::Release);
    }
}

fn mark_mnt_clean() {
    MNT_DIRTY.store(false, Ordering::Release);
}

fn mnt_path(path: &str) -> bool {
    path == "/mnt" || path.starts_with("/mnt/")
}

fn unavailable_ensure_error() -> EnsureError {
    match lifecycle_from_code(MNT_STATUS.load(Ordering::Acquire)) {
        MountLifecycleStatus::NoDevice => EnsureError::NoDevice,
        MountLifecycleStatus::NotFat32 => EnsureError::NotFat32,
        MountLifecycleStatus::Failed => EnsureError::Failed,
        _ => EnsureError::Unmounted,
    }
}

fn unavailable_persist_error() -> PersistError {
    match lifecycle_from_code(MNT_STATUS.load(Ordering::Acquire)) {
        MountLifecycleStatus::NoDevice => PersistError::NoDevice,
        _ => PersistError::Unmounted,
    }
}

fn unavailable_mutation_error() -> MutationError {
    match lifecycle_from_code(MNT_STATUS.load(Ordering::Acquire)) {
        MountLifecycleStatus::NoDevice => MutationError::NoDevice,
        _ => MutationError::Unmounted,
    }
}

fn unavailable_space_error() -> SpaceError {
    match lifecycle_from_code(MNT_STATUS.load(Ordering::Acquire)) {
        MountLifecycleStatus::NoDevice => SpaceError::NoDevice,
        MountLifecycleStatus::NotFat32 => SpaceError::NotFat32,
        MountLifecycleStatus::Failed => SpaceError::Failed,
        _ => SpaceError::Unmounted,
    }
}
pub fn mount_ata_root() -> MountStatus {
    let status = ata::with_primary_master(|disk| {
        let direct = mount_device(disk);
        if direct != MountStatus::NotFat32 {
            if matches!(direct, MountStatus::Mounted(_)) {
                configure_direct_swap(disk);
            }
            return direct;
        }
        match partition::find_fat32(disk) {
            Ok(Some(partition)) => {
                let status = mount_partition(disk, partition);
                if matches!(status, MountStatus::Mounted(_)) {
                    configure_partition_swap(disk, partition);
                }
                status
            }
            Ok(None) => match gpt::find_fat_partition(disk) {
                Ok(Some(partition)) => {
                    let status = mount_partition(disk, partition);
                    if matches!(status, MountStatus::Mounted(_)) {
                        configure_partition_swap(disk, partition);
                    }
                    status
                }
                Ok(None) | Err(gpt::Error::MissingProtectiveMbr) => MountStatus::NotFat32,
                Err(_) => MountStatus::Failed,
            },
            Err(_) => MountStatus::Failed,
        }
    })
    .unwrap_or(MountStatus::NoDevice);
    record_mount_status(status);
    status
}

fn mount_partition(
    device: &mut impl crate::block::BlockDevice,
    partition: partition::Partition,
) -> MountStatus {
    let Ok(mut view) = partition::PartitionDevice::new(device, partition) else {
        return MountStatus::Failed;
    };
    mount_device(&mut view)
}

fn configure_direct_swap(device: &mut impl crate::block::BlockDevice) {
    let Ok(volume) = fat32::mount(device) else {
        return;
    };
    configure_swap_area(volume.total_sectors as u64, device.sector_count());
}

fn configure_partition_swap(
    device: &mut impl crate::block::BlockDevice,
    partition: partition::Partition,
) {
    let Ok(mut view) = partition::PartitionDevice::new(device, partition) else {
        return;
    };
    let Ok(volume) = fat32::mount(&mut view) else {
        return;
    };
    let Some(start_lba) = partition.start_lba.checked_add(volume.total_sectors as u64) else {
        return;
    };
    let Some(limit_lba) = partition.start_lba.checked_add(partition.sectors) else {
        return;
    };
    configure_swap_area(start_lba, limit_lba);
}

fn configure_swap_area(start_lba: u64, limit_lba: u64) {
    if limit_lba <= start_lba {
        return;
    }
    let _ = swap::configure_ata_backing(start_lba, limit_lba - start_lba);
}

fn mount_device(device: &mut impl crate::block::BlockDevice) -> MountStatus {
    let volume = match fat32::mount(device) {
        Ok(volume) => volume,
        Err(fat32::Error::InvalidBootSector | fat32::Error::UnsupportedGeometry) => {
            return MountStatus::NotFat32;
        }
        Err(_) => return MountStatus::Failed,
    };

    let _ = vfs::mkdir("/mnt");

    let writable_import = !device.is_read_only();

    match import_directory(
        device,
        volume,
        volume.root_cluster,
        "/mnt",
        0,
        writable_import,
    ) {
        Ok(count) => MountStatus::Mounted(count),
        Err(_) => MountStatus::Failed,
    }
}

fn import_directory(
    device: &mut impl crate::block::BlockDevice,
    volume: fat32::Volume,
    dir_cluster: u32,
    vfs_prefix: &str,
    depth: usize,
    writable_import: bool,
) -> Result<usize, fat32::Error> {
    let mut mounted = 0usize;
    let mut children = alloc::vec::Vec::new();

    fat32::for_each_directory_entry(device, volume, dir_cluster, |entry| {
        if entry.attributes & 0x08 != 0 {
            return Ok(());
        }
        // Skip . and ..
        if entry.short_name[0] == b'.' {
            return Ok(());
        }

        let mut name = [0u8; 12];
        let Some(name_len) = short_name_to_str(&entry.short_name, &mut name) else {
            return Ok(());
        };
        let Ok(name_str) = core::str::from_utf8(&name[..name_len]) else {
            return Ok(());
        };

        let mut path_buf = [0u8; crate::config::MAX_PATH_SIZE];
        let Some(path_len) = join_path(vfs_prefix, name_str, &mut path_buf) else {
            return Ok(());
        };
        let Ok(path) = core::str::from_utf8(&path_buf[..path_len]) else {
            return Ok(());
        };

        let is_dir = entry.attributes & DIRECTORY_ATTRIBUTE != 0;
        if is_dir {
            match vfs::mkdir(path) {
                Ok(()) => mounted = mounted.saturating_add(1),
                Err(vfs::Error::AlreadyExists) | Err(vfs::Error::Full) => {}
                Err(_) => return Err(fat32::Error::DirectoryFull),
            }
            if depth < MAX_BOOT_IMPORT_DEPTH && entry.first_cluster >= 2
                && children.len() < crate::config::MAX_VFS_NODES {
                    children.push((entry.first_cluster, alloc::string::String::from(path)));
            }
            return Ok(());
        }

        if entry.size as usize > vfs::NODE_CAPACITY {
            return Ok(());
        }
        let writable = writable_import && entry.attributes & READ_ONLY_ATTRIBUTE == 0;
        match vfs::create_disk_file_with_writable(path, entry.size as usize, writable) {
            Ok(()) => mounted = mounted.saturating_add(1),
            Err(vfs::Error::AlreadyExists) | Err(vfs::Error::Full) => {}
            Err(_) => {}
        }
        Ok(())
    })?;

    // Release the directory iterator's mutable device borrow before recursion.
    for (cluster, path) in children {
        mounted = mounted.saturating_add(import_directory(
            device, volume, cluster, &path, depth + 1, writable_import,
        )?);
    }
    Ok(mounted)
}
fn join_path(prefix: &str, name: &str, out: &mut [u8]) -> Option<usize> {
    let slash = !prefix.ends_with('/');
    let need = prefix.len() + usize::from(slash) + name.len();
    if need > out.len() {
        return None;
    }
    out[..prefix.len()].copy_from_slice(prefix.as_bytes());
    let mut len = prefix.len();
    if slash {
        out[len] = b'/';
        len += 1;
    }
    out[len..len + name.len()].copy_from_slice(name.as_bytes());
    Some(len + name.len())
}

fn short_name_to_str(short_name: &[u8; 11], out: &mut [u8; 12]) -> Option<usize> {
    let mut len = 0usize;
    for byte in short_name[..8].iter().copied().take_while(|b| *b != b' ') {
        out[len] = to_lower(byte)?;
        len += 1;
    }
    if len == 0 {
        return None;
    }
    if short_name[8..].iter().any(|b| *b != b' ') {
        out[len] = b'.';
        len += 1;
        for byte in short_name[8..].iter().copied().take_while(|b| *b != b' ') {
            out[len] = to_lower(byte)?;
            len += 1;
        }
    }
    Some(len)
}

fn to_lower(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte + (b'a' - b'A')),
        b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' => Some(byte),
        _ => None,
    }
}

pub fn self_test() -> bool {
    let mut name = [0u8; 12];
    let nlen = short_name_to_str(b"KERNEL  BIN", &mut name).unwrap_or(0);
    let kernel_ok = &name[..nlen] == b"kernel.bin";
    let nlen = short_name_to_str(b"README     ", &mut name).unwrap_or(0);
    let readme_ok = &name[..nlen] == b"readme";
    let bad_ok = short_name_to_str(b"BAD?    TXT", &mut name).is_none();
    let encode_ok = fat32::encode_short_name("kernel.bin") == Some(*b"KERNEL  BIN")
        && fat32::encode_short_name("readme") == Some(*b"README     ");
    kernel_ok && readme_ok && bad_ok && encode_ok
}

/// Ensure a path under `/mnt` exists in the VFS by resolving it on the live ATA volume.
///
/// If the path is already present, returns success immediately. Otherwise opens the
/// primary ATA master, mounts FAT32 (superfloppy / MBR / GPT), resolves the relative
/// path, and imports the file or directory into the VFS.
pub fn ensure_path(path: &str) -> Result<(), EnsureError> {
    if mnt_path(path) && !mnt_mounted() {
        return Err(unavailable_ensure_error());
    }
    if !mnt_path(path) {
        return if vfs::stat(path).is_ok() {
            Ok(())
        } else {
            Err(EnsureError::NotUnderMount)
        };
    }

    if let Ok(stat) = vfs::stat(path) {
        if stat.kind != vfs::NodeKind::Directory {
            return Ok(());
        }
    }

    if !block_io::primary_ata_present() {
        return Err(EnsureError::NoDevice);
    }
    let mut disk = block_io::primary_ata();

    if path == "/mnt" {
        match vfs::mkdir("/mnt") {
            Ok(()) | Err(vfs::Error::AlreadyExists) => {}
            Err(_) => return Err(EnsureError::Vfs),
        }
        return ensure_root_listing_on_disk(&mut disk);
    }

    let relative = &path[5..]; // strip "/mnt/"
    ensure_on_disk(&mut disk, relative, path)
}
fn ensure_root_listing_on_disk(
    device: &mut impl crate::block::BlockDevice,
) -> Result<(), EnsureError> {
    match fat32::mount(device) {
        Ok(volume) => {
            let imported = import_directory(
                device,
                volume,
                volume.root_cluster,
                "/mnt",
                MAX_BOOT_IMPORT_DEPTH,
                !device.is_read_only(),
            )
            .map_err(map_fat_err)?;
            MNT_IMPORTED.fetch_add(imported, Ordering::AcqRel);
            return Ok(());
        }
        Err(fat32::Error::InvalidBootSector | fat32::Error::UnsupportedGeometry) => {}
        Err(_) => return Err(EnsureError::Failed),
    }

    if let Ok(Some(part)) = partition::find_fat32(device) {
        let mut view =
            partition::PartitionDevice::new(device, part).map_err(|_| EnsureError::Failed)?;
        let volume = fat32::mount(&mut view).map_err(map_fat_err)?;
        let writable_import = !view.is_read_only();
        let imported = import_directory(
            &mut view,
            volume,
            volume.root_cluster,
            "/mnt",
            MAX_BOOT_IMPORT_DEPTH,
            writable_import,
        )
        .map_err(map_fat_err)?;
        MNT_IMPORTED.fetch_add(imported, Ordering::AcqRel);
        return Ok(());
    }

    match gpt::find_fat_partition(device) {
        Ok(Some(part)) => {
            let mut view =
                partition::PartitionDevice::new(device, part).map_err(|_| EnsureError::Failed)?;
            let volume = fat32::mount(&mut view).map_err(map_fat_err)?;
            let writable_import = !view.is_read_only();
            let imported = import_directory(
                &mut view,
                volume,
                volume.root_cluster,
                "/mnt",
                MAX_BOOT_IMPORT_DEPTH,
                writable_import,
            )
            .map_err(map_fat_err)?;
            MNT_IMPORTED.fetch_add(imported, Ordering::AcqRel);
            Ok(())
        }
        Ok(None) | Err(gpt::Error::MissingProtectiveMbr) => Err(EnsureError::NotFat32),
        Err(_) => Err(EnsureError::Failed),
    }
}
fn ensure_on_disk(
    device: &mut impl crate::block::BlockDevice,
    relative: &str,
    full_path: &str,
) -> Result<(), EnsureError> {
    // Prefer superfloppy at LBA 0.
    match fat32::mount(device) {
        Ok(volume) => return import_resolved(device, volume, relative, full_path),
        Err(fat32::Error::InvalidBootSector | fat32::Error::UnsupportedGeometry) => {}
        Err(_) => return Err(EnsureError::Failed),
    }

    if let Ok(Some(part)) = partition::find_fat32(device) {
        let mut view =
            partition::PartitionDevice::new(device, part).map_err(|_| EnsureError::Failed)?;
        let volume = fat32::mount(&mut view).map_err(map_fat_err)?;
        return import_resolved(&mut view, volume, relative, full_path);
    }

    match gpt::find_fat_partition(device) {
        Ok(Some(part)) => {
            let mut view =
                partition::PartitionDevice::new(device, part).map_err(|_| EnsureError::Failed)?;
            let volume = fat32::mount(&mut view).map_err(map_fat_err)?;
            import_resolved(&mut view, volume, relative, full_path)
        }
        Ok(None) | Err(gpt::Error::MissingProtectiveMbr) => Err(EnsureError::NotFat32),
        Err(_) => Err(EnsureError::Failed),
    }
}

fn import_resolved(
    device: &mut impl crate::block::BlockDevice,
    volume: fat32::Volume,
    relative: &str,
    full_path: &str,
) -> Result<(), EnsureError> {
    let entry = fat32::resolve_path(device, volume, relative).map_err(map_fat_err)?;
    let is_dir = entry.attributes & DIRECTORY_ATTRIBUTE != 0;

    create_parent_dirs(full_path)?;

    if is_dir {
        match vfs::mkdir(full_path) {
            Ok(()) | Err(vfs::Error::AlreadyExists) => {}
            Err(_) => return Err(EnsureError::Vfs),
        }
        let imported = import_directory(
            device,
            volume,
            entry.first_cluster,
            full_path,
            MAX_BOOT_IMPORT_DEPTH,
            !device.is_read_only(),
        )
        .map_err(map_fat_err)?;
        MNT_IMPORTED.fetch_add(imported, Ordering::AcqRel);
        return Ok(());
    }
    if entry.size as usize > vfs::NODE_CAPACITY {
        return Err(EnsureError::TooLarge);
    }
    let writable = !device.is_read_only() && entry.attributes & READ_ONLY_ATTRIBUTE == 0;
    match vfs::create_disk_file_with_writable(full_path, entry.size as usize, writable) {
        Ok(()) | Err(vfs::Error::AlreadyExists) => Ok(()),
        Err(_) => Err(EnsureError::Vfs),
    }
}

fn create_parent_dirs(path: &str) -> Result<(), EnsureError> {
    let bytes = path.as_bytes();
    if !path.starts_with('/') {
        return Err(EnsureError::InvalidPath);
    }
    let mut i = 1;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i] != b'/' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let prefix = core::str::from_utf8(&bytes[..i]).map_err(|_| EnsureError::InvalidPath)?;
        match vfs::mkdir(prefix) {
            Ok(()) | Err(vfs::Error::AlreadyExists) => {}
            Err(_) => return Err(EnsureError::Vfs),
        }
        i += 1;
    }
    Ok(())
}

fn map_fat_err(err: fat32::Error) -> EnsureError {
    match err {
        fat32::Error::NotFound => EnsureError::NotFound,
        fat32::Error::InvalidBootSector | fat32::Error::UnsupportedGeometry => {
            EnsureError::NotFat32
        }
        _ => EnsureError::Failed,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EnsureError {
    Unmounted,
    NoDevice,
    NotFat32,
    NotFound,
    NotUnderMount,
    InvalidPath,
    TooLarge,
    Vfs,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersistError {
    Unmounted,
    NotSupported,
    NotFound,
    NoDevice,
    BadName,
    TooLarge,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutationError {
    Unmounted,
    NotSupported,
    NotFound,
    NoDevice,
    BadName,
    AlreadyExists,
    NotEmpty,
    ReadOnly,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LiveMutationTestStatus {
    Skipped,
    Passed,
    Failed(&'static str),
}

/// Exercise the live ATA/FAT32 mutation path when a mounted `/mnt` volume exists.
///
/// This is intended for disposable QEMU disks. It mirrors the diagnostic-shell
/// sequence for mkdir/write/persist/directory rename/delete and skips when the
/// boot has no writable FAT32 ATA mount.
pub fn live_mutation_self_test() -> LiveMutationTestStatus {
    if !block_io::primary_ata_present() || vfs::stat("/mnt").is_err() {
        return LiveMutationTestStatus::Skipped;
    }

    const OLD_DIR: &str = "/mnt/tmutd";
    const NEW_DIR: &str = "/mnt/tmuta";
    const OLD_FILE: &str = "/mnt/tmutd/a.txt";
    const NEW_FILE: &str = "/mnt/tmuta/a.txt";
    const CONTENT: &[u8] = b"hello";

    if vfs::stat(OLD_DIR).is_ok() || vfs::stat(NEW_DIR).is_ok() {
        return LiveMutationTestStatus::Failed("fixture exists");
    }
    if vfs::mkdir(OLD_DIR).is_err() {
        return LiveMutationTestStatus::Failed("vfs mkdir");
    }
    if persist_directory(OLD_DIR).is_err() {
        return LiveMutationTestStatus::Failed("fat mkdir");
    }
    if vfs::write_file(OLD_FILE, CONTENT).is_err() {
        return LiveMutationTestStatus::Failed("vfs write");
    }
    if persist_path(OLD_FILE).is_err() {
        return LiveMutationTestStatus::Failed("fat persist");
    }
    if rename_path(OLD_DIR, NEW_DIR).is_err() {
        return LiveMutationTestStatus::Failed("fat rename");
    }
    if vfs::rename(OLD_DIR, NEW_DIR).is_err() {
        return LiveMutationTestStatus::Failed("vfs rename");
    }
    if ensure_path(OLD_FILE) != Err(EnsureError::NotFound) || vfs::stat(OLD_FILE).is_ok() {
        return LiveMutationTestStatus::Failed("old path survived");
    }
    if ensure_path(NEW_FILE).is_err() {
        return LiveMutationTestStatus::Failed("new path missing");
    }
    let mut bytes = [0_u8; 8];
    if vfs::read_all(NEW_FILE, &mut bytes) != Ok(CONTENT.len())
        || &bytes[..CONTENT.len()] != CONTENT
    {
        return LiveMutationTestStatus::Failed("new data mismatch");
    }
    if delete_path(NEW_FILE).is_err() {
        return LiveMutationTestStatus::Failed("fat delete file");
    }
    if vfs::remove(NEW_FILE).is_err() {
        return LiveMutationTestStatus::Failed("vfs delete file");
    }
    if delete_path(NEW_DIR).is_err() {
        return LiveMutationTestStatus::Failed("fat delete dir");
    }
    if vfs::remove(NEW_DIR).is_err() {
        return LiveMutationTestStatus::Failed("vfs delete dir");
    }

    if let Err(stage) = live_directory_growth_self_test() {
        return LiveMutationTestStatus::Failed(stage);
    }
    if df_mnt().is_err() {
        return LiveMutationTestStatus::Failed("df");
    }
    match fscheck_mnt() {
        Ok(report) if report.fs_info_matches => {}
        Ok(_) => return LiveMutationTestStatus::Failed("fsinfo mismatch"),
        Err(_) => return LiveMutationTestStatus::Failed("fscheck"),
    }
    if sync_all_mounted().is_err() {
        return LiveMutationTestStatus::Failed("sync");
    }
    if mount_info().dirty {
        return LiveMutationTestStatus::Failed("sync dirty");
    }
    if unmount_mnt().is_err() {
        return LiveMutationTestStatus::Failed("umount");
    }
    if ensure_path("/mnt/cache.txt") != Err(EnsureError::Unmounted) {
        return LiveMutationTestStatus::Failed("umount ensure guard");
    }
    if persist_path("/mnt/cache.txt") != Err(PersistError::Unmounted) {
        return LiveMutationTestStatus::Failed("umount persist guard");
    }
    if !matches!(remount_mnt(), MountStatus::Mounted(_)) || !mnt_mounted() {
        return LiveMutationTestStatus::Failed("remount");
    }

    LiveMutationTestStatus::Passed
}

fn live_numbered_child_path<'a>(
    dir: &str,
    prefix: u8,
    index: usize,
    out: &'a mut [u8; 40],
) -> Option<&'a str> {
    if index >= 100 {
        return None;
    }
    let dir_bytes = dir.as_bytes();
    let total = dir_bytes.len().checked_add(1)?.checked_add(7)?;
    if total > out.len() {
        return None;
    }
    out[..dir_bytes.len()].copy_from_slice(dir_bytes);
    let mut offset = dir_bytes.len();
    out[offset] = b'/';
    offset += 1;
    out[offset] = prefix;
    out[offset + 1] = b'0' + (index / 10) as u8;
    out[offset + 2] = b'0' + (index % 10) as u8;
    out[offset + 3] = b'.';
    out[offset + 4] = b't';
    out[offset + 5] = b'x';
    out[offset + 6] = b't';
    core::str::from_utf8(&out[..total]).ok()
}

fn create_live_test_file(path: &str, content: &[u8]) -> Result<(), &'static str> {
    if vfs::write_file(path, content).is_err() {
        return Err("growth vfs write");
    }
    if persist_path(path).is_err() {
        return Err("growth fat persist");
    }
    Ok(())
}

fn remove_live_test_file(path: &str) -> Result<(), &'static str> {
    if delete_path(path).is_err() {
        return Err("growth fat delete file");
    }
    if vfs::remove(path).is_err() {
        return Err("growth vfs delete file");
    }
    Ok(())
}

fn remove_live_test_dir(path: &str) -> Result<(), &'static str> {
    if delete_path(path).is_err() {
        return Err("growth fat delete dir");
    }
    if vfs::remove(path).is_err() {
        return Err("growth vfs delete dir");
    }
    Ok(())
}

fn live_directory_growth_self_test() -> Result<(), &'static str> {
    const GROW_DIR: &str = "/mnt/tgrow";
    const FULL_DIR: &str = "/mnt/tfull";
    const MOVE_FILE: &str = "/mnt/tmv.txt";
    const MOVED_FILE: &str = "/mnt/tfull/tmv.txt";

    if vfs::stat(GROW_DIR).is_ok() || vfs::stat(FULL_DIR).is_ok() || vfs::stat(MOVE_FILE).is_ok() {
        return Err("growth fixture exists");
    }

    if vfs::mkdir(GROW_DIR).is_err() {
        return Err("growth vfs mkdir");
    }
    if persist_directory(GROW_DIR).is_err() {
        return Err("growth fat mkdir");
    }
    for index in 0..15 {
        let mut path = [0_u8; 40];
        let Some(path) = live_numbered_child_path(GROW_DIR, b'g', index, &mut path) else {
            return Err("growth path build");
        };
        create_live_test_file(path, b"x")?;
    }
    let mut last_path = [0_u8; 40];
    let Some(last_path) = live_numbered_child_path(GROW_DIR, b'g', 14, &mut last_path) else {
        return Err("growth path build");
    };
    if ensure_path(last_path).is_err() {
        return Err("growth ensure");
    }
    let mut byte = [0_u8; 1];
    if vfs::read_all(last_path, &mut byte) != Ok(1) || byte[0] != b'x' {
        return Err("growth readback");
    }

    if vfs::mkdir(FULL_DIR).is_err() {
        return Err("full vfs mkdir");
    }
    if persist_directory(FULL_DIR).is_err() {
        return Err("full fat mkdir");
    }
    for index in 0..14 {
        let mut path = [0_u8; 40];
        let Some(path) = live_numbered_child_path(FULL_DIR, b'd', index, &mut path) else {
            return Err("full path build");
        };
        create_live_test_file(path, b"q")?;
    }
    create_live_test_file(MOVE_FILE, b"z")?;
    if rename_path(MOVE_FILE, MOVED_FILE).is_err() {
        return Err("full fat rename");
    }
    if vfs::rename(MOVE_FILE, MOVED_FILE).is_err() {
        return Err("full vfs rename");
    }
    if ensure_path(MOVE_FILE) != Err(EnsureError::NotFound) || vfs::stat(MOVE_FILE).is_ok() {
        return Err("full old survived");
    }
    if ensure_path(MOVED_FILE).is_err() {
        return Err("full new missing");
    }
    if vfs::read_all(MOVED_FILE, &mut byte) != Ok(1) || byte[0] != b'z' {
        return Err("full readback");
    }

    remove_live_test_file(MOVED_FILE)?;
    for index in 0..14 {
        let mut path = [0_u8; 40];
        let Some(path) = live_numbered_child_path(FULL_DIR, b'd', index, &mut path) else {
            return Err("full path build");
        };
        remove_live_test_file(path)?;
    }
    remove_live_test_dir(FULL_DIR)?;
    for index in 0..15 {
        let mut path = [0_u8; 40];
        let Some(path) = live_numbered_child_path(GROW_DIR, b'g', index, &mut path) else {
            return Err("growth path build");
        };
        remove_live_test_file(path)?;
    }
    remove_live_test_dir(GROW_DIR)
}

/// Persist a VFS file under `/mnt/` to the live ATA FAT32 volume.
///
/// Multi-component paths are supported. Missing FAT32 directories are created
/// automatically; every component currently follows FAT 8.3 naming rules.
pub fn persist_path(path: &str) -> Result<(), PersistError> {
    if !path.starts_with("/mnt/") {
        return Err(PersistError::NotSupported);
    }
    if !mnt_mounted() {
        return Err(unavailable_persist_error());
    }
    let relative = &path[5..];
    if relative.is_empty() {
        return Err(PersistError::BadName);
    }
    for component in relative.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || fat32::encode_short_name(component).is_none()
        {
            return Err(PersistError::BadName);
        }
    }

    let mut data = [0_u8; vfs::NODE_CAPACITY];
    let length = match vfs::read_all(path, &mut data) {
        Ok(n) => n,
        Err(vfs::Error::NotFound) => return Err(PersistError::NotFound),
        Err(_) => return Err(PersistError::Failed),
    };
    if length > vfs::NODE_CAPACITY {
        return Err(PersistError::TooLarge);
    }

    if !block_io::primary_ata_present() {
        return Err(PersistError::NoDevice);
    }
    let mut disk = block_io::primary_ata();
    persist_on_device(&mut disk, relative, &data[..length])
}

fn persist_on_device(
    device: &mut impl crate::block::BlockDevice,
    path: &str,
    data: &[u8],
) -> Result<(), PersistError> {
    // The ATA cache survives this transaction; flush before reporting success.
    FILE_PAGES.lock().invalidate();
    let result = persist_on_cached_device(device, path, data);
    let flushed = device.flush().map_err(|_| PersistError::Failed);
    let status = result.and(flushed);
    if status.is_ok() {
        mark_mnt_dirty();
    }
    status
}

fn persist_on_cached_device(
    device: &mut impl crate::block::BlockDevice,
    path: &str,
    data: &[u8],
) -> Result<(), PersistError> {
    // Same mount order as import: superfloppy → MBR → GPT.
    match fat32::mount(device) {
        Ok(volume) => {
            return fat32::create_path_file(device, volume, path, data).map_err(map_persist_err);
        }
        Err(fat32::Error::InvalidBootSector | fat32::Error::UnsupportedGeometry) => {}
        Err(_) => return Err(PersistError::Failed),
    }

    if let Ok(Some(part)) = partition::find_fat32(device) {
        let mut view =
            partition::PartitionDevice::new(device, part).map_err(|_| PersistError::Failed)?;
        let volume = fat32::mount(&mut view).map_err(map_persist_err)?;
        return fat32::create_path_file(&mut view, volume, path, data).map_err(map_persist_err);
    }

    match gpt::find_fat_partition(device) {
        Ok(Some(part)) => {
            let mut view =
                partition::PartitionDevice::new(device, part).map_err(|_| PersistError::Failed)?;
            let volume = fat32::mount(&mut view).map_err(map_persist_err)?;
            fat32::create_path_file(&mut view, volume, path, data).map_err(map_persist_err)
        }
        Ok(None) | Err(gpt::Error::MissingProtectiveMbr) => Err(PersistError::Failed),
        Err(_) => Err(PersistError::Failed),
    }
}

/// Persist a VFS directory under `/mnt/`, creating missing FAT32 components.
pub fn persist_directory(path: &str) -> Result<(), PersistError> {
    if path == "/mnt" {
        return if mnt_mounted() {
            Ok(())
        } else {
            Err(unavailable_persist_error())
        };
    }
    if !path.starts_with("/mnt/") {
        return Err(PersistError::NotSupported);
    }
    if !mnt_mounted() {
        return Err(unavailable_persist_error());
    }
    let relative = &path[5..];
    if relative.is_empty() {
        return Err(PersistError::BadName);
    }
    for component in relative.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || fat32::encode_short_name(component).is_none()
        {
            return Err(PersistError::BadName);
        }
    }
    if !block_io::primary_ata_present() {
        return Err(PersistError::NoDevice);
    }
    let mut disk = block_io::primary_ata();
    FILE_PAGES.lock().invalidate();
    let result = mkdir_on_cached_device(&mut disk, relative);
    let flushed = disk.flush().map_err(|_| PersistError::Failed);
    let status = result.and(flushed);
    if status.is_ok() {
        mark_mnt_dirty();
    }
    status
}

fn mkdir_on_cached_device(
    device: &mut impl crate::block::BlockDevice,
    path: &str,
) -> Result<(), PersistError> {
    match fat32::mount(device) {
        Ok(volume) => return fat32::mkdir_path(device, volume, path).map_err(map_persist_err),
        Err(fat32::Error::InvalidBootSector | fat32::Error::UnsupportedGeometry) => {}
        Err(_) => return Err(PersistError::Failed),
    }
    if let Ok(Some(part)) = partition::find_fat32(device) {
        let mut view =
            partition::PartitionDevice::new(device, part).map_err(|_| PersistError::Failed)?;
        let volume = fat32::mount(&mut view).map_err(map_persist_err)?;
        return fat32::mkdir_path(&mut view, volume, path).map_err(map_persist_err);
    }
    match gpt::find_fat_partition(device) {
        Ok(Some(part)) => {
            let mut view =
                partition::PartitionDevice::new(device, part).map_err(|_| PersistError::Failed)?;
            let volume = fat32::mount(&mut view).map_err(map_persist_err)?;
            fat32::mkdir_path(&mut view, volume, path).map_err(map_persist_err)
        }
        _ => Err(PersistError::Failed),
    }
}

pub fn delete_path(path: &str) -> Result<(), MutationError> {
    if path == "/mnt" || !path.starts_with("/mnt/") {
        return Err(MutationError::NotSupported);
    }
    if !mnt_mounted() {
        return Err(unavailable_mutation_error());
    }
    let relative = &path[5..];
    validate_fat_relative(relative)?;
    if !block_io::primary_ata_present() {
        return Err(MutationError::NoDevice);
    }
    let mut disk = block_io::primary_ata();
    if disk.is_read_only() {
        return Err(MutationError::ReadOnly);
    }
    FILE_PAGES.lock().invalidate();
    let result = delete_on_cached_device(&mut disk, relative);
    let flushed = disk.flush().map_err(|_| MutationError::Failed);
    let status = result.and(flushed);
    if status.is_ok() {
        mark_mnt_dirty();
    }
    status
}

fn delete_on_cached_device(
    device: &mut impl crate::block::BlockDevice,
    path: &str,
) -> Result<(), MutationError> {
    match fat32::mount(device) {
        Ok(volume) => return fat32::delete_path(device, volume, path).map_err(map_mutation_err),
        Err(fat32::Error::InvalidBootSector | fat32::Error::UnsupportedGeometry) => {}
        Err(_) => return Err(MutationError::Failed),
    }
    if let Ok(Some(part)) = partition::find_fat32(device) {
        let mut view =
            partition::PartitionDevice::new(device, part).map_err(|_| MutationError::Failed)?;
        let volume = fat32::mount(&mut view).map_err(map_mutation_err)?;
        return fat32::delete_path(&mut view, volume, path).map_err(map_mutation_err);
    }
    match gpt::find_fat_partition(device) {
        Ok(Some(part)) => {
            let mut view =
                partition::PartitionDevice::new(device, part).map_err(|_| MutationError::Failed)?;
            let volume = fat32::mount(&mut view).map_err(map_mutation_err)?;
            fat32::delete_path(&mut view, volume, path).map_err(map_mutation_err)
        }
        _ => Err(MutationError::Failed),
    }
}

pub fn rename_path(old: &str, new: &str) -> Result<(), MutationError> {
    if old == "/mnt" || new == "/mnt" || !old.starts_with("/mnt/") || !new.starts_with("/mnt/") {
        return Err(MutationError::NotSupported);
    }
    if !mnt_mounted() {
        return Err(unavailable_mutation_error());
    }
    let old_relative = &old[5..];
    let new_relative = &new[5..];
    validate_fat_relative(old_relative)?;
    validate_fat_relative(new_relative)?;
    if !block_io::primary_ata_present() {
        return Err(MutationError::NoDevice);
    }
    let mut disk = block_io::primary_ata();
    if disk.is_read_only() {
        return Err(MutationError::ReadOnly);
    }
    FILE_PAGES.lock().invalidate();
    let result = rename_on_cached_device(&mut disk, old_relative, new_relative);
    let flushed = disk.flush().map_err(|_| MutationError::Failed);
    let status = result.and(flushed);
    if status.is_ok() {
        mark_mnt_dirty();
    }
    status
}

fn rename_on_cached_device(
    device: &mut impl crate::block::BlockDevice,
    old: &str,
    new: &str,
) -> Result<(), MutationError> {
    match fat32::mount(device) {
        Ok(volume) => {
            return fat32::rename_path(device, volume, old, new).map_err(map_mutation_err)
        }
        Err(fat32::Error::InvalidBootSector | fat32::Error::UnsupportedGeometry) => {}
        Err(_) => return Err(MutationError::Failed),
    }
    if let Ok(Some(part)) = partition::find_fat32(device) {
        let mut view =
            partition::PartitionDevice::new(device, part).map_err(|_| MutationError::Failed)?;
        let volume = fat32::mount(&mut view).map_err(map_mutation_err)?;
        return fat32::rename_path(&mut view, volume, old, new).map_err(map_mutation_err);
    }
    match gpt::find_fat_partition(device) {
        Ok(Some(part)) => {
            let mut view =
                partition::PartitionDevice::new(device, part).map_err(|_| MutationError::Failed)?;
            let volume = fat32::mount(&mut view).map_err(map_mutation_err)?;
            fat32::rename_path(&mut view, volume, old, new).map_err(map_mutation_err)
        }
        _ => Err(MutationError::Failed),
    }
}

fn validate_fat_relative(relative: &str) -> Result<(), MutationError> {
    if relative.is_empty() {
        return Err(MutationError::BadName);
    }
    for component in relative.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || fat32::encode_short_name(component).is_none()
        {
            return Err(MutationError::BadName);
        }
    }
    Ok(())
}

fn map_mutation_err(err: fat32::Error) -> MutationError {
    match err {
        fat32::Error::NotFound => MutationError::NotFound,
        fat32::Error::NameTooLong | fat32::Error::InvalidPath => MutationError::BadName,
        fat32::Error::AlreadyExists => MutationError::AlreadyExists,
        fat32::Error::DirectoryNotEmpty => MutationError::NotEmpty,
        fat32::Error::ReadOnly | fat32::Error::Block(crate::block::Error::ReadOnly) => {
            MutationError::ReadOnly
        }
        fat32::Error::Block(_) | fat32::Error::WriteFailed => MutationError::Failed,
        _ => MutationError::Failed,
    }
}

fn map_persist_err(err: fat32::Error) -> PersistError {
    match err {
        fat32::Error::NotFound => PersistError::NotFound,
        fat32::Error::NameTooLong | fat32::Error::DirectoryFull | fat32::Error::InvalidPath => {
            PersistError::BadName
        }
        fat32::Error::NoSpace => PersistError::TooLarge,
        fat32::Error::Block(_) | fat32::Error::WriteFailed | fat32::Error::ReadOnly => {
            PersistError::Failed
        }
        _ => PersistError::Failed,
    }
}

#[allow(dead_code)]
pub fn fat32_writable() -> bool {
    mnt_mounted() && block_io::primary_ata_present()
}

pub fn remount_mnt() -> MountStatus {
    FILE_PAGES.lock().invalidate();
    mount_ata_root()
}

pub fn unmount_mnt() -> Result<(), MountControlError> {
    if !mnt_mounted() {
        return Err(
            match lifecycle_from_code(MNT_STATUS.load(Ordering::Acquire)) {
                MountLifecycleStatus::NoDevice => MountControlError::NoDevice,
                _ => MountControlError::NotMounted,
            },
        );
    }
    sync_all_mounted().map_err(|_| MountControlError::SyncFailed)?;
    FILE_PAGES.lock().invalidate();
    if !block_io::primary_ata_present() {
        record_mount_status(MountStatus::NoDevice);
        return Err(MountControlError::NoDevice);
    }
    let mut disk = block_io::primary_ata();
    if disk.flush().is_err() {
        return Err(MountControlError::Failed);
    }
    MNT_STATUS.store(MOUNT_UNMOUNTED, Ordering::Release);
    MNT_IMPORTED.store(0, Ordering::Release);
    mark_mnt_clean();
    Ok(())
}

pub fn df_mnt() -> Result<fat32::SpaceInfo, SpaceError> {
    if !mnt_mounted() {
        return Err(unavailable_space_error());
    }
    if !block_io::primary_ata_present() {
        return Err(SpaceError::NoDevice);
    }
    let mut disk = block_io::primary_ata();
    df_on_device(&mut disk)
}

pub fn fscheck_mnt() -> Result<fat32::CheckReport, SpaceError> {
    if !mnt_mounted() {
        return Err(unavailable_space_error());
    }
    if !block_io::primary_ata_present() {
        return Err(SpaceError::NoDevice);
    }
    let mut disk = block_io::primary_ata();
    check_on_device(&mut disk)
}

fn df_on_device(
    device: &mut impl crate::block::BlockDevice,
) -> Result<fat32::SpaceInfo, SpaceError> {
    match fat32::mount(device) {
        Ok(volume) => return fat32::space_info(device, volume).map_err(map_space_err),
        Err(fat32::Error::InvalidBootSector | fat32::Error::UnsupportedGeometry) => {}
        Err(_) => return Err(SpaceError::Failed),
    }
    match partition::find_fat32(device) {
        Ok(Some(part)) => {
            let mut view =
                partition::PartitionDevice::new(device, part).map_err(|_| SpaceError::Failed)?;
            let volume = fat32::mount(&mut view).map_err(map_space_err)?;
            fat32::space_info(&mut view, volume).map_err(map_space_err)
        }
        Ok(None) => match gpt::find_fat_partition(device) {
            Ok(Some(part)) => {
                let mut view = partition::PartitionDevice::new(device, part)
                    .map_err(|_| SpaceError::Failed)?;
                let volume = fat32::mount(&mut view).map_err(map_space_err)?;
                fat32::space_info(&mut view, volume).map_err(map_space_err)
            }
            Ok(None) | Err(gpt::Error::MissingProtectiveMbr) => Err(SpaceError::NotFat32),
            Err(_) => Err(SpaceError::Failed),
        },
        Err(_) => Err(SpaceError::Failed),
    }
}

fn check_on_device(
    device: &mut impl crate::block::BlockDevice,
) -> Result<fat32::CheckReport, SpaceError> {
    match fat32::mount(device) {
        Ok(volume) => return fat32::check_volume(device, volume).map_err(map_space_err),
        Err(fat32::Error::InvalidBootSector | fat32::Error::UnsupportedGeometry) => {}
        Err(_) => return Err(SpaceError::Failed),
    }
    match partition::find_fat32(device) {
        Ok(Some(part)) => {
            let mut view =
                partition::PartitionDevice::new(device, part).map_err(|_| SpaceError::Failed)?;
            let volume = fat32::mount(&mut view).map_err(map_space_err)?;
            fat32::check_volume(&mut view, volume).map_err(map_space_err)
        }
        Ok(None) => match gpt::find_fat_partition(device) {
            Ok(Some(part)) => {
                let mut view = partition::PartitionDevice::new(device, part)
                    .map_err(|_| SpaceError::Failed)?;
                let volume = fat32::mount(&mut view).map_err(map_space_err)?;
                fat32::check_volume(&mut view, volume).map_err(map_space_err)
            }
            Ok(None) | Err(gpt::Error::MissingProtectiveMbr) => Err(SpaceError::NotFat32),
            Err(_) => Err(SpaceError::Failed),
        },
        Err(_) => Err(SpaceError::Failed),
    }
}

fn map_space_err(error: fat32::Error) -> SpaceError {
    match error {
        fat32::Error::InvalidBootSector | fat32::Error::UnsupportedGeometry => SpaceError::NotFat32,
        _ => SpaceError::Failed,
    }
}

/// Best-effort walk of every VFS file under `/mnt/` and persist each one.
/// Returns the number of files successfully written.
pub fn sync_all_mounted() -> Result<usize, PersistError> {
    if !mnt_mounted() {
        return Err(unavailable_persist_error());
    }
    // Snapshot names while holding VFS; persistence reacquires VFS to read data.
    // Never call persist_path from the registry's locked callback.
    let mut paths = alloc::vec::Vec::new();
    vfs::for_each_file_with_prefix("/mnt/", |path| {
        paths.push(alloc::string::String::from(path));
    });
    let mut ok = 0usize;
    let mut failed = false;
    for path in paths {
        if persist_path(&path).is_ok() {
            ok += 1;
        } else {
            failed = true;
        }
    }
    if block_io::primary_ata_present() {
        let mut disk = block_io::primary_ata();
        if disk.flush().is_err() {
            failed = true;
        }
    } else {
        failed = true;
    }
    if failed {
        mark_mnt_dirty();
        Err(PersistError::Failed)
    } else {
        mark_mnt_clean();
        MNT_SYNCS.fetch_add(1, Ordering::AcqRel);
        Ok(ok)
    }
}
// Lock order: ATA device, then file pages. Never call VFS while holding pages.
static FILE_PAGES: spin::Mutex<crate::page_cache::PageCache<16>> =
    spin::Mutex::new(crate::page_cache::PageCache::new());

pub fn page_cache_stats() -> crate::page_cache::Stats {
    FILE_PAGES.lock().stats()
}

pub fn read_disk_file(
    path: &str,
    offset: usize,
    output: &mut [u8],
) -> Result<usize, crate::block::Error> {
    if !mnt_mounted() {
        return Err(crate::block::Error::DeviceFault);
    }
    let relative = path
        .strip_prefix("/mnt/")
        .ok_or(crate::block::Error::InvalidBuffer)?;
    if !block_io::primary_ata_present() {
        return Err(crate::block::Error::DeviceFault);
    }
    let mut disk = block_io::primary_ata();
    FILE_PAGES.lock().read(path, offset, output, |start, page| {
        read_disk_page(&mut disk, relative, start, page)
            .map_err(|_| crate::block::Error::DeviceFault)
    })
}

fn read_volume_page(
    device: &mut impl crate::block::BlockDevice,
    volume: fat32::Volume,
    path: &str,
    offset: usize,
    output: &mut [u8],
) -> Result<usize, fat32::Error> {
    let entry = fat32::resolve_path(device, volume, path)?;
    if entry.attributes & DIRECTORY_ATTRIBUTE != 0 {
        return Err(fat32::Error::NotFound);
    }
    fat32::read_file_at(device, volume, entry, offset, output)
}

fn read_disk_page(
    device: &mut impl crate::block::BlockDevice,
    path: &str,
    offset: usize,
    output: &mut [u8],
) -> Result<usize, fat32::Error> {
    match fat32::mount(device) {
        Ok(volume) => return read_volume_page(device, volume, path, offset, output),
        Err(fat32::Error::InvalidBootSector | fat32::Error::UnsupportedGeometry) => {}
        Err(error) => return Err(error),
    }
    let part = match partition::find_fat32(device) {
        Ok(Some(part)) => part,
        _ => gpt::find_fat_partition(device)
            .map_err(|_| fat32::Error::InvalidBootSector)?
            .ok_or(fat32::Error::InvalidBootSector)?,
    };
    let mut view = partition::PartitionDevice::new(device, part)
        .map_err(|_| fat32::Error::InvalidBootSector)?;
    let volume = fat32::mount(&mut view)?;
    read_volume_page(&mut view, volume, path, offset, output)
}

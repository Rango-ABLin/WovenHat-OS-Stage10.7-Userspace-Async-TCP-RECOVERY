use crate::block::{BlockDevice, Error as BlockError, SECTOR_SIZE};

const FAT32_MIN_CLUSTERS: u32 = 65_525;
const DIRECTORY_ENTRY_SIZE: usize = 32;
const DIRECTORY_ENTRIES_PER_SECTOR: usize = SECTOR_SIZE / DIRECTORY_ENTRY_SIZE;
const FAT32_ENTRY_MASK: u32 = 0x0fff_ffff;
const FAT32_BAD_CLUSTER: u32 = 0x0fff_fff7;
const FAT32_END_MIN: u32 = 0x0fff_fff8;
const FSINFO_UNKNOWN: u32 = 0xffff_ffff;
const FSINFO_LEAD_SIGNATURE: u32 = 0x4161_5252;
const FSINFO_STRUCT_SIGNATURE: u32 = 0x6141_7272;
const FSINFO_TRAIL_SIGNATURE: u32 = 0xaa55_0000;
const FSINFO_FREE_COUNT_OFFSET: usize = 488;
const FSINFO_NEXT_FREE_OFFSET: usize = 492;
const READ_ONLY_ATTRIBUTE: u8 = 0x01;
const VOLUME_ID_ATTRIBUTE: u8 = 0x08;
const DIRECTORY_ATTRIBUTE: u8 = 0x10;
const LONG_NAME_ATTRIBUTE: u8 = 0x0f;
const MAX_DIRECTORY_CLUSTERS: usize = 128;
const MAX_FS_CHECK_DEPTH: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Block(BlockError),
    InvalidBootSector,
    UnsupportedGeometry,
    CorruptDirectory,
    DirectoryFull,
    CorruptChain,
    ChainLoop,
    ChainTooLong,
    TruncatedFile,
    NotFound,
    NoSpace,
    NameTooLong,
    AlreadyExists,
    DirectoryNotEmpty,
    ReadOnly,
    InvalidPath,
    WriteFailed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Volume {
    pub total_sectors: u32,
    pub sectors_per_cluster: u8,
    pub fat_count: u8,
    pub fat_size: u32,
    pub root_cluster: u32,
    pub first_fat_sector: u64,
    pub first_data_sector: u64,
    pub fs_info_sector: Option<u32>,
    pub backup_fs_info_sector: Option<u32>,
    cluster_count: u32,
}

impl Volume {
    pub fn cluster_lba(&self, cluster: u32) -> Result<u64, Error> {
        if cluster < 2 || cluster >= self.cluster_count.saturating_add(2) {
            return Err(Error::CorruptChain);
        }
        self.first_data_sector
            .checked_add((cluster - 2) as u64 * self.sectors_per_cluster as u64)
            .ok_or(Error::UnsupportedGeometry)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub short_name: [u8; 11],
    pub first_cluster: u32,
    pub size: u32,
    pub attributes: u8,
}

#[derive(Clone, Copy)]
struct DirectorySlot {
    lba: u64,
    offset: usize,
    entry: DirectoryEntry,
    raw: [u8; DIRECTORY_ENTRY_SIZE],
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FsInfo {
    free_count: u32,
    next_free: u32,
}

#[derive(Clone, Copy)]
struct DirectoryExtension {
    parent_cluster: u32,
    cluster: u32,
}

#[derive(Clone, Copy)]
struct DirectorySlotReservation {
    lba: u64,
    offset: usize,
    existing: Option<DirectoryEntry>,
    extension: Option<DirectoryExtension>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClusterLink {
    Next(u32),
    End,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SpaceInfo {
    pub total_clusters: u32,
    pub free_clusters: u32,
    pub bytes_per_cluster: u32,
    pub fs_info_free_count: Option<u32>,
    pub fs_info_next_free: Option<u32>,
    pub fs_info_matches: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CheckReport {
    pub files: usize,
    pub directories: usize,
    pub total_clusters: u32,
    pub free_clusters: u32,
    pub fs_info_free_count: Option<u32>,
    pub fs_info_matches: bool,
}
pub fn mount(device: &mut impl BlockDevice) -> Result<Volume, Error> {
    let mut sector = [0_u8; SECTOR_SIZE];
    device.read_sector(0, &mut sector).map_err(Error::Block)?;

    if sector[510] != 0x55 || sector[511] != 0xaa {
        return Err(Error::InvalidBootSector);
    }
    let bytes_per_sector = read_u16(&sector, 11);
    let sectors_per_cluster = sector[13];
    let reserved_sectors = read_u16(&sector, 14);
    let fat_count = sector[16];
    let root_entries = read_u16(&sector, 17);
    let total_sectors_16 = read_u16(&sector, 19);
    let fat_size_16 = read_u16(&sector, 22);
    let total_sectors = read_u32(&sector, 32);
    let fat_size = read_u32(&sector, 36);
    let root_cluster = read_u32(&sector, 44);
    let fs_info_sector_field = read_u16(&sector, 48) as u32;
    let backup_boot_sector = read_u16(&sector, 50) as u32;

    if bytes_per_sector as usize != SECTOR_SIZE
        || sectors_per_cluster == 0
        || !sectors_per_cluster.is_power_of_two()
        || sectors_per_cluster > 128
        || reserved_sectors == 0
        || !matches!(fat_count, 1 | 2)
        || root_entries != 0
        || total_sectors_16 != 0
        || fat_size_16 != 0
        || total_sectors == 0
        || fat_size == 0
        || root_cluster < 2
    {
        return Err(Error::UnsupportedGeometry);
    }
    if total_sectors as u64 > device.sector_count() {
        return Err(Error::UnsupportedGeometry);
    }

    let fat_sectors = (fat_count as u32)
        .checked_mul(fat_size)
        .ok_or(Error::UnsupportedGeometry)?;
    let first_data = (reserved_sectors as u32)
        .checked_add(fat_sectors)
        .ok_or(Error::UnsupportedGeometry)?;
    let data_sectors = total_sectors
        .checked_sub(first_data)
        .ok_or(Error::UnsupportedGeometry)?;
    let cluster_count = data_sectors / sectors_per_cluster as u32;
    if cluster_count < FAT32_MIN_CLUSTERS || root_cluster >= cluster_count.saturating_add(2) {
        return Err(Error::UnsupportedGeometry);
    }

    let fs_info_sector = valid_fs_info_sector(device, fs_info_sector_field, reserved_sectors);
    let backup_fs_info_sector = if backup_boot_sector == 0 {
        None
    } else {
        backup_boot_sector
            .checked_add(1)
            .and_then(|sector| valid_fs_info_sector(device, sector, reserved_sectors))
    };

    Ok(Volume {
        total_sectors,
        sectors_per_cluster,
        fat_count,
        fat_size,
        root_cluster,
        first_fat_sector: reserved_sectors as u64,
        first_data_sector: first_data as u64,
        fs_info_sector,
        backup_fs_info_sector,
        cluster_count,
    })
}

/// Look up a short name in the volume root directory.
pub fn find_root(
    device: &mut impl BlockDevice,
    volume: Volume,
    short_name: &[u8; 11],
) -> Result<DirectoryEntry, Error> {
    find_in_directory(device, volume, volume.root_cluster, short_name)
}

/// Look up a short name in any directory starting at `dir_cluster`.
pub fn find_in_directory(
    device: &mut impl BlockDevice,
    volume: Volume,
    dir_cluster: u32,
    short_name: &[u8; 11],
) -> Result<DirectoryEntry, Error> {
    let mut cluster = dir_cluster;
    let mut visited = [0_u32; MAX_DIRECTORY_CLUSTERS];
    let mut visited_count = 0;
    let mut sector = [0_u8; SECTOR_SIZE];

    loop {
        if visited_count == visited.len() {
            return Err(Error::ChainTooLong);
        }
        if visited[..visited_count].contains(&cluster) {
            return Err(Error::ChainLoop);
        }
        visited[visited_count] = cluster;
        visited_count += 1;

        let cluster_lba = volume.cluster_lba(cluster)?;
        for sector_index in 0..volume.sectors_per_cluster as u64 {
            device
                .read_sector(cluster_lba + sector_index, &mut sector)
                .map_err(Error::Block)?;
            match scan_directory_sector(&sector, short_name)? {
                DirectoryScan::Found(entry) => return Ok(entry),
                DirectoryScan::End => return Err(Error::NotFound),
                DirectoryScan::Continue => {}
            }
        }

        cluster = match next_cluster(device, volume, cluster)? {
            ClusterLink::Next(next) => next,
            ClusterLink::End => return Err(Error::NotFound),
        };
    }
}

/// List entries in the volume root directory.
pub fn list_root(
    device: &mut impl BlockDevice,
    volume: Volume,
    output: &mut [Option<DirectoryEntry>],
) -> Result<usize, Error> {
    list_directory(device, volume, volume.root_cluster, output)
}

/// Visit entries in any directory starting at `dir_cluster`.
pub fn for_each_directory_entry<F>(
    device: &mut impl BlockDevice,
    volume: Volume,
    dir_cluster: u32,
    mut visitor: F,
) -> Result<(), Error>
where
    F: FnMut(DirectoryEntry) -> Result<(), Error>,
{
    let mut cluster = dir_cluster;
    let mut visited = [0_u32; MAX_DIRECTORY_CLUSTERS];
    let mut visited_count = 0;
    let mut sector = [0_u8; SECTOR_SIZE];

    loop {
        if visited_count == visited.len() {
            return Err(Error::ChainTooLong);
        }
        if visited[..visited_count].contains(&cluster) {
            return Err(Error::ChainLoop);
        }
        visited[visited_count] = cluster;
        visited_count += 1;

        let cluster_lba = volume.cluster_lba(cluster)?;
        for sector_index in 0..volume.sectors_per_cluster as u64 {
            device
                .read_sector(cluster_lba + sector_index, &mut sector)
                .map_err(Error::Block)?;
            for index in 0..DIRECTORY_ENTRIES_PER_SECTOR {
                let offset = index * DIRECTORY_ENTRY_SIZE;
                let first = sector[offset];
                if first == 0 {
                    return Ok(());
                }
                let attributes = sector[offset + 11];
                if first == 0xe5
                    || attributes == LONG_NAME_ATTRIBUTE
                    || attributes & VOLUME_ID_ATTRIBUTE != 0
                {
                    continue;
                }
                visitor(directory_entry_from_sector(&sector, offset)?)?;
            }
        }
        cluster = match next_cluster(device, volume, cluster)? {
            ClusterLink::Next(next) => next,
            ClusterLink::End => return Ok(()),
        };
    }
}

/// List entries in any directory starting at `dir_cluster`.
pub fn list_directory(
    device: &mut impl BlockDevice,
    volume: Volume,
    dir_cluster: u32,
    output: &mut [Option<DirectoryEntry>],
) -> Result<usize, Error> {
    let mut count = 0;
    for_each_directory_entry(device, volume, dir_cluster, |entry| {
        let slot = output.get_mut(count).ok_or(Error::DirectoryFull)?;
        *slot = Some(entry);
        count += 1;
        Ok(())
    })?;
    Ok(count)
}
/// Resolve a Unix-style absolute path of 8.3 components against a FAT32 volume.
///
/// Example: `"/BIN/SH"` or `"BIN/SH"` (leading slash optional). Each component is
/// converted to a FAT 8.3 short name. Returns the final directory entry.
pub fn resolve_path(
    device: &mut impl BlockDevice,
    volume: Volume,
    path: &str,
) -> Result<DirectoryEntry, Error> {
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        return Err(Error::NotFound);
    }

    // Collect up to 8 path components without allocation.
    let mut parts = [""; 8];
    let mut part_count = 0usize;
    for component in path.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." || part_count == parts.len() {
            return Err(Error::NotFound);
        }
        parts[part_count] = component;
        part_count += 1;
    }
    if part_count == 0 {
        return Err(Error::NotFound);
    }

    let mut cluster = volume.root_cluster;
    let mut current = None;
    for (i, component) in parts[..part_count].iter().enumerate() {
        let short = encode_short_name(component).ok_or(Error::NotFound)?;
        let entry = find_in_directory(device, volume, cluster, &short)?;
        let is_last = i + 1 == part_count;
        if !is_last {
            if entry.attributes & DIRECTORY_ATTRIBUTE == 0 || entry.first_cluster < 2 {
                return Err(Error::NotFound);
            }
            cluster = entry.first_cluster;
        }
        current = Some(entry);
    }
    current.ok_or(Error::NotFound)
}

/// Encode a path component into a FAT 8.3 short name (space-padded).
pub fn encode_short_name(name: &str) -> Option<[u8; 11]> {
    if name.is_empty() || name.len() > 12 {
        return None;
    }
    let mut out = [b' '; 11];
    let (base, ext) = match name.find('.') {
        Some(pos) => (&name[..pos], &name[pos + 1..]),
        None => (name, ""),
    };
    if base.is_empty() || base.len() > 8 || ext.len() > 3 {
        return None;
    }
    if base.contains('.') || ext.contains('.') {
        return None;
    }
    for (i, byte) in base.bytes().enumerate() {
        out[i] = to_fat_char(byte)?;
    }
    for (i, byte) in ext.bytes().enumerate() {
        out[8 + i] = to_fat_char(byte)?;
    }
    Some(out)
}

fn to_fat_char(byte: u8) -> Option<u8> {
    match byte {
        b'a'..=b'z' => Some(byte - (b'a' - b'A')),
        b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' => Some(byte),
        _ => None,
    }
}

enum DirectoryScan {
    Found(DirectoryEntry),
    Continue,
    End,
}

fn scan_directory_sector(
    sector: &[u8; SECTOR_SIZE],
    short_name: &[u8; 11],
) -> Result<DirectoryScan, Error> {
    for index in 0..DIRECTORY_ENTRIES_PER_SECTOR {
        let offset = index * DIRECTORY_ENTRY_SIZE;
        let first = sector[offset];
        if first == 0 {
            return Ok(DirectoryScan::End);
        }
        let attributes = sector[offset + 11];
        if first == 0xe5
            || attributes == LONG_NAME_ATTRIBUTE
            || attributes & VOLUME_ID_ATTRIBUTE != 0
        {
            continue;
        }
        let mut entry_name = [0_u8; 11];
        entry_name.copy_from_slice(&sector[offset..offset + 11]);
        if &entry_name != short_name {
            continue;
        }

        let high_cluster = read_u16(sector, offset + 20) as u32;
        let low_cluster = read_u16(sector, offset + 26) as u32;
        let first_cluster = (high_cluster << 16) | low_cluster;
        if first_cluster < 2 && read_u32(sector, offset + 28) != 0 {
            return Err(Error::CorruptDirectory);
        }
        return Ok(DirectoryScan::Found(DirectoryEntry {
            short_name: entry_name,
            first_cluster,
            size: read_u32(sector, offset + 28),
            attributes,
        }));
    }
    Ok(DirectoryScan::Continue)
}
pub fn next_cluster(
    device: &mut impl BlockDevice,
    volume: Volume,
    cluster: u32,
) -> Result<ClusterLink, Error> {
    volume.cluster_lba(cluster)?;
    let fat_offset = (cluster as u64).checked_mul(4).ok_or(Error::CorruptChain)?;
    let fat_sector = volume
        .first_fat_sector
        .checked_add(fat_offset / SECTOR_SIZE as u64)
        .ok_or(Error::CorruptChain)?;
    if fat_sector >= volume.first_data_sector {
        return Err(Error::CorruptChain);
    }

    let mut sector = [0_u8; SECTOR_SIZE];
    device
        .read_sector(fat_sector, &mut sector)
        .map_err(Error::Block)?;
    let offset = (fat_offset % SECTOR_SIZE as u64) as usize;
    let value = read_u32(&sector, offset) & FAT32_ENTRY_MASK;
    if value >= FAT32_END_MIN {
        return Ok(ClusterLink::End);
    }
    if value < 2 || value == FAT32_BAD_CLUSTER {
        return Err(Error::CorruptChain);
    }
    volume.cluster_lba(value)?;
    Ok(ClusterLink::Next(value))
}

pub fn read_file(
    device: &mut impl BlockDevice,
    volume: Volume,
    entry: DirectoryEntry,
    buffer: &mut [u8],
) -> Result<usize, Error> {
    read_file_at(device, volume, entry, 0, buffer)
}

/// Read a byte range without loading preceding file contents.
pub fn read_file_at(
    device: &mut impl BlockDevice,
    volume: Volume,
    entry: DirectoryEntry,
    offset: usize,
    buffer: &mut [u8],
) -> Result<usize, Error> {
    let target = (entry.size as usize)
        .saturating_sub(offset)
        .min(buffer.len());
    if target == 0 {
        return Ok(0);
    }
    let mut cluster = entry.first_cluster;
    let mut chain = ChainCycleGuard::new(cluster);
    let mut traversed = 0_u32;
    let mut position = 0usize;
    let mut copied = 0;
    let mut sector = [0_u8; SECTOR_SIZE];
    while copied < target {
        traversed = traversed.checked_add(1).ok_or(Error::ChainTooLong)?;
        if traversed > volume.cluster_count {
            return Err(Error::ChainTooLong);
        }

        let cluster_lba = volume.cluster_lba(cluster)?;
        for sector_index in 0..volume.sectors_per_cluster as u64 {
            if copied == target {
                break;
            }
            if position + SECTOR_SIZE > offset {
                device
                    .read_sector(cluster_lba + sector_index, &mut sector)
                    .map_err(Error::Block)?;
                let start = offset.saturating_sub(position);
                let count = (SECTOR_SIZE - start).min(target - copied);
                buffer[copied..copied + count].copy_from_slice(&sector[start..start + count]);
                copied += count;
            }
            position += SECTOR_SIZE;
        }
        if copied == target {
            return Ok(copied);
        }
        cluster = match next_cluster(device, volume, cluster)? {
            ClusterLink::Next(next) => {
                chain.advance(device, volume)?;
                next
            }
            ClusterLink::End => return Err(Error::TruncatedFile),
        };
    }
    Ok(copied)
}

pub fn space_info(device: &mut impl BlockDevice, volume: Volume) -> Result<SpaceInfo, Error> {
    let total_clusters = volume.cluster_count;
    let bytes_per_cluster = volume.sectors_per_cluster as u32 * SECTOR_SIZE as u32;
    let end = total_clusters
        .checked_add(2)
        .ok_or(Error::UnsupportedGeometry)?;
    let entries_per_sector = (SECTOR_SIZE / 4) as u32;
    let mut free_clusters = 0_u32;
    let mut cluster = 2_u32;
    let mut sector = [0_u8; SECTOR_SIZE];

    while cluster < end {
        volume.cluster_lba(cluster)?;
        let fat_offset = (cluster as u64).checked_mul(4).ok_or(Error::CorruptChain)?;
        let fat_sector = volume
            .first_fat_sector
            .checked_add(fat_offset / SECTOR_SIZE as u64)
            .ok_or(Error::CorruptChain)?;
        if fat_sector >= volume.first_data_sector {
            return Err(Error::CorruptChain);
        }
        device
            .read_sector(fat_sector, &mut sector)
            .map_err(Error::Block)?;

        let sector_first_cluster =
            ((fat_sector - volume.first_fat_sector) * entries_per_sector as u64) as u32;
        let first_index = (cluster - sector_first_cluster) as usize;
        for index in first_index..entries_per_sector as usize {
            let current_cluster = sector_first_cluster + index as u32;
            if current_cluster >= end {
                break;
            }
            let entry = read_u32(&sector, index * 4) & FAT32_ENTRY_MASK;
            if entry == 0 {
                free_clusters = free_clusters.saturating_add(1);
            }
        }

        let next = sector_first_cluster.saturating_add(entries_per_sector);
        if next <= cluster {
            return Err(Error::CorruptChain);
        }
        cluster = next;
    }

    let fs_info = read_fs_info(device, volume)?;
    let fs_info_free_count = fs_info.and_then(|info| {
        if info.free_count == FSINFO_UNKNOWN {
            None
        } else {
            Some(info.free_count)
        }
    });
    let fs_info_next_free = fs_info.and_then(|info| {
        if info.next_free == FSINFO_UNKNOWN || !is_allocatable_cluster(volume, info.next_free) {
            None
        } else {
            Some(info.next_free)
        }
    });
    let fs_info_matches = fs_info_free_count.is_none_or(|hint| hint == free_clusters);

    Ok(SpaceInfo {
        total_clusters,
        free_clusters,
        bytes_per_cluster,
        fs_info_free_count,
        fs_info_next_free,
        fs_info_matches,
    })
}

pub fn check_volume(device: &mut impl BlockDevice, volume: Volume) -> Result<CheckReport, Error> {
    volume.cluster_lba(volume.root_cluster)?;
    validate_cluster_chain(device, volume, volume.root_cluster)?;

    let space = space_info(device, volume)?;
    let mut report = CheckReport {
        files: 0,
        directories: 1,
        total_clusters: space.total_clusters,
        free_clusters: space.free_clusters,
        fs_info_free_count: space.fs_info_free_count,
        fs_info_matches: space.fs_info_matches,
    };
    check_directory_tree(device, volume, volume.root_cluster, 0, &mut report)?;
    Ok(report)
}

fn check_directory_tree(
    device: &mut impl BlockDevice,
    volume: Volume,
    dir_cluster: u32,
    depth: usize,
    report: &mut CheckReport,
) -> Result<(), Error> {
    let mut cluster = dir_cluster;
    let mut visited = [0_u32; MAX_DIRECTORY_CLUSTERS];
    let mut visited_count = 0_usize;
    let mut sector = [0_u8; SECTOR_SIZE];

    loop {
        if visited_count == visited.len() {
            return Err(Error::ChainTooLong);
        }
        if visited[..visited_count].contains(&cluster) {
            return Err(Error::ChainLoop);
        }
        visited[visited_count] = cluster;
        visited_count += 1;

        let base = volume.cluster_lba(cluster)?;
        for sector_index in 0..volume.sectors_per_cluster as u64 {
            device
                .read_sector(base + sector_index, &mut sector)
                .map_err(Error::Block)?;
            for index in 0..DIRECTORY_ENTRIES_PER_SECTOR {
                let offset = index * DIRECTORY_ENTRY_SIZE;
                let first = sector[offset];
                if first == 0 {
                    return Ok(());
                }
                if first == 0xe5 {
                    continue;
                }
                let attributes = sector[offset + 11];
                if attributes == LONG_NAME_ATTRIBUTE || attributes & VOLUME_ID_ATTRIBUTE != 0 {
                    continue;
                }

                let entry = directory_entry_from_sector(&sector, offset)?;
                if is_dot_directory_short_name(&entry.short_name) {
                    continue;
                }

                if entry.attributes & DIRECTORY_ATTRIBUTE != 0 {
                    validate_cluster_chain(device, volume, entry.first_cluster)?;
                    report.directories = report.directories.saturating_add(1);
                    if depth < MAX_FS_CHECK_DEPTH {
                        check_directory_tree(
                            device,
                            volume,
                            entry.first_cluster,
                            depth + 1,
                            report,
                        )?;
                    }
                } else {
                    if entry.first_cluster >= 2 {
                        validate_cluster_chain(device, volume, entry.first_cluster)?;
                    }
                    report.files = report.files.saturating_add(1);
                }
            }
        }

        cluster = match next_cluster(device, volume, cluster)? {
            ClusterLink::Next(next) => next,
            ClusterLink::End => return Ok(()),
        };
    }
}

fn is_dot_directory_short_name(short_name: &[u8; 11]) -> bool {
    (short_name[0] == b'.' && short_name[1..].iter().all(|byte| *byte == b' '))
        || (short_name[0] == b'.'
            && short_name[1] == b'.'
            && short_name[2..].iter().all(|byte| *byte == b' '))
}

struct ChainCycleGuard {
    tortoise: u32,
    hare: u32,
    active: bool,
    steps: u32,
}

impl ChainCycleGuard {
    const fn new(start: u32) -> Self {
        Self {
            tortoise: start,
            hare: start,
            active: true,
            steps: 0,
        }
    }

    fn advance(&mut self, device: &mut impl BlockDevice, volume: Volume) -> Result<(), Error> {
        if !self.active {
            return Ok(());
        }
        self.steps = self.steps.checked_add(1).ok_or(Error::ChainTooLong)?;
        if self.steps > volume.cluster_count {
            return Err(Error::ChainTooLong);
        }

        let Some(tortoise) = advance_chain_once(device, volume, self.tortoise)? else {
            self.active = false;
            return Ok(());
        };
        let Some(first_hare) = advance_chain_once(device, volume, self.hare)? else {
            self.tortoise = tortoise;
            self.active = false;
            return Ok(());
        };
        let Some(hare) = advance_chain_once(device, volume, first_hare)? else {
            self.tortoise = tortoise;
            self.hare = first_hare;
            self.active = false;
            return Ok(());
        };

        self.tortoise = tortoise;
        self.hare = hare;
        if self.tortoise == self.hare {
            return Err(Error::ChainLoop);
        }
        Ok(())
    }
}

fn advance_chain_once(
    device: &mut impl BlockDevice,
    volume: Volume,
    cluster: u32,
) -> Result<Option<u32>, Error> {
    match next_cluster(device, volume, cluster)? {
        ClusterLink::Next(next) => Ok(Some(next)),
        ClusterLink::End => Ok(None),
    }
}

fn validate_cluster_chain(
    device: &mut impl BlockDevice,
    volume: Volume,
    start: u32,
) -> Result<(), Error> {
    let mut cluster = start;
    let mut chain = ChainCycleGuard::new(start);
    let mut traversed = 0_u32;
    loop {
        traversed = traversed.checked_add(1).ok_or(Error::ChainTooLong)?;
        if traversed > volume.cluster_count {
            return Err(Error::ChainTooLong);
        }
        match next_cluster(device, volume, cluster)? {
            ClusterLink::Next(next) => {
                chain.advance(device, volume)?;
                cluster = next;
            }
            ClusterLink::End => return Ok(()),
        }
    }
}

fn free_cluster_chain(
    device: &mut impl BlockDevice,
    volume: Volume,
    start: u32,
) -> Result<(), Error> {
    if start < 2 {
        return Ok(());
    }
    validate_cluster_chain(device, volume, start)?;
    let mut cluster = start;
    let mut freed = 0_u32;
    loop {
        freed = freed.checked_add(1).ok_or(Error::ChainTooLong)?;
        if freed > volume.cluster_count {
            return Err(Error::ChainTooLong);
        }
        let link = next_cluster(device, volume, cluster)?;
        write_fat_entry(device, volume, cluster, 0)?;
        match link {
            ClusterLink::Next(next) => cluster = next,
            ClusterLink::End => {
                let _ = update_fs_info_after_free(device, volume, freed, start);
                return Ok(());
            }
        }
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    let le = value.to_le_bytes();
    bytes[offset] = le[0];
    bytes[offset + 1] = le[1];
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    let le = value.to_le_bytes();
    bytes[offset] = le[0];
    bytes[offset + 1] = le[1];
    bytes[offset + 2] = le[2];
    bytes[offset + 3] = le[3];
}

fn fs_info_signatures_valid(sector: &[u8]) -> bool {
    sector.len() == SECTOR_SIZE
        && read_u32(sector, 0) == FSINFO_LEAD_SIGNATURE
        && read_u32(sector, 484) == FSINFO_STRUCT_SIGNATURE
        && read_u32(sector, 508) == FSINFO_TRAIL_SIGNATURE
}

fn valid_fs_info_sector(
    device: &mut impl BlockDevice,
    sector_lba: u32,
    reserved_sectors: u16,
) -> Option<u32> {
    if sector_lba == 0 || sector_lba >= reserved_sectors as u32 {
        return None;
    }
    let mut sector = [0_u8; SECTOR_SIZE];
    if device.read_sector(sector_lba as u64, &mut sector).is_err() {
        return None;
    }
    fs_info_signatures_valid(&sector).then_some(sector_lba)
}

fn sanitize_fs_info_value(value: u32, max: u32) -> u32 {
    if value == FSINFO_UNKNOWN || value <= max {
        value
    } else {
        FSINFO_UNKNOWN
    }
}

fn sanitize_next_free(volume: Volume, value: u32) -> u32 {
    if is_allocatable_cluster(volume, value) {
        value
    } else {
        FSINFO_UNKNOWN
    }
}

fn read_fs_info(device: &mut impl BlockDevice, volume: Volume) -> Result<Option<FsInfo>, Error> {
    let Some(sector_lba) = volume.fs_info_sector else {
        return Ok(None);
    };
    let mut sector = [0_u8; SECTOR_SIZE];
    device
        .read_sector(sector_lba as u64, &mut sector)
        .map_err(Error::Block)?;
    if !fs_info_signatures_valid(&sector) {
        return Ok(None);
    }
    Ok(Some(FsInfo {
        free_count: sanitize_fs_info_value(
            read_u32(&sector, FSINFO_FREE_COUNT_OFFSET),
            volume.cluster_count,
        ),
        next_free: sanitize_next_free(volume, read_u32(&sector, FSINFO_NEXT_FREE_OFFSET)),
    }))
}

fn write_fs_info_sector(
    device: &mut impl BlockDevice,
    sector_lba: u32,
    info: FsInfo,
) -> Result<(), Error> {
    let mut sector = [0_u8; SECTOR_SIZE];
    device
        .read_sector(sector_lba as u64, &mut sector)
        .map_err(Error::Block)?;
    if !fs_info_signatures_valid(&sector) {
        return Ok(());
    }
    write_u32(&mut sector, FSINFO_FREE_COUNT_OFFSET, info.free_count);
    write_u32(&mut sector, FSINFO_NEXT_FREE_OFFSET, info.next_free);
    device
        .write_sector(sector_lba as u64, &sector)
        .map_err(Error::Block)
}

fn write_fs_info(device: &mut impl BlockDevice, volume: Volume, info: FsInfo) -> Result<(), Error> {
    if let Some(sector_lba) = volume.fs_info_sector {
        write_fs_info_sector(device, sector_lba, info)?;
    }
    if let Some(sector_lba) = volume.backup_fs_info_sector {
        write_fs_info_sector(device, sector_lba, info)?;
    }
    Ok(())
}

fn is_allocatable_cluster(volume: Volume, cluster: u32) -> bool {
    cluster >= 2 && cluster < volume.cluster_count.saturating_add(2)
}

fn next_cluster_hint(volume: Volume, cluster: u32) -> u32 {
    let next = cluster.saturating_add(1);
    if is_allocatable_cluster(volume, next) {
        next
    } else {
        2
    }
}

fn fs_info_next_free_hint(device: &mut impl BlockDevice, volume: Volume) -> u32 {
    match read_fs_info(device, volume) {
        Ok(Some(info)) if is_allocatable_cluster(volume, info.next_free) => info.next_free,
        _ => 2,
    }
}

fn update_fs_info_after_alloc(
    device: &mut impl BlockDevice,
    volume: Volume,
    allocated_cluster: u32,
) -> Result<(), Error> {
    let Some(mut info) = read_fs_info(device, volume)? else {
        return Ok(());
    };
    if info.free_count != FSINFO_UNKNOWN {
        info.free_count = info.free_count.saturating_sub(1);
    }
    info.next_free = next_cluster_hint(volume, allocated_cluster);
    write_fs_info(device, volume, info)
}

fn update_fs_info_after_free(
    device: &mut impl BlockDevice,
    volume: Volume,
    freed_count: u32,
    first_freed: u32,
) -> Result<(), Error> {
    let Some(mut info) = read_fs_info(device, volume)? else {
        return Ok(());
    };
    if info.free_count != FSINFO_UNKNOWN {
        info.free_count = info
            .free_count
            .saturating_add(freed_count)
            .min(volume.cluster_count);
    }
    if is_allocatable_cluster(volume, first_freed) {
        info.next_free = first_freed;
    }
    write_fs_info(device, volume, info)
}

fn read_fat_entry(
    device: &mut impl BlockDevice,
    volume: Volume,
    cluster: u32,
) -> Result<u32, Error> {
    volume.cluster_lba(cluster)?;
    let fat_offset = (cluster as u64).checked_mul(4).ok_or(Error::CorruptChain)?;
    let fat_sector = volume
        .first_fat_sector
        .checked_add(fat_offset / SECTOR_SIZE as u64)
        .ok_or(Error::CorruptChain)?;
    if fat_sector >= volume.first_data_sector {
        return Err(Error::CorruptChain);
    }
    let mut sector = [0_u8; SECTOR_SIZE];
    device
        .read_sector(fat_sector, &mut sector)
        .map_err(Error::Block)?;
    let offset = (fat_offset % SECTOR_SIZE as u64) as usize;
    Ok(read_u32(&sector, offset) & FAT32_ENTRY_MASK)
}

fn write_fat_entry(
    device: &mut impl BlockDevice,
    volume: Volume,
    cluster: u32,
    value: u32,
) -> Result<(), Error> {
    volume.cluster_lba(cluster)?;
    let fat_offset = (cluster as u64).checked_mul(4).ok_or(Error::CorruptChain)?;
    let sector_index = fat_offset / SECTOR_SIZE as u64;
    let offset = (fat_offset % SECTOR_SIZE as u64) as usize;
    let masked = value & FAT32_ENTRY_MASK;

    for fat in 0..volume.fat_count as u64 {
        let fat_sector = volume
            .first_fat_sector
            .checked_add(fat * volume.fat_size as u64)
            .and_then(|base| base.checked_add(sector_index))
            .ok_or(Error::CorruptChain)?;
        if fat_sector >= volume.first_data_sector {
            return Err(Error::CorruptChain);
        }
        let mut sector = [0_u8; SECTOR_SIZE];
        device
            .read_sector(fat_sector, &mut sector)
            .map_err(Error::Block)?;
        // Preserve high nibble reserved bits if any were set.
        let existing = read_u32(&sector, offset);
        let new_value = (existing & !FAT32_ENTRY_MASK) | masked;
        write_u32(&mut sector, offset, new_value);
        device
            .write_sector(fat_sector, &sector)
            .map_err(Error::Block)?;
    }
    Ok(())
}

fn allocate_cluster(device: &mut impl BlockDevice, volume: Volume) -> Result<u32, Error> {
    let max = volume
        .cluster_count
        .checked_add(2)
        .ok_or(Error::UnsupportedGeometry)?;
    let mut start = fs_info_next_free_hint(device, volume);
    if !is_allocatable_cluster(volume, start) {
        start = 2;
    }

    match allocate_cluster_in_range(device, volume, start, max) {
        Ok(cluster) => Ok(cluster),
        Err(Error::NoSpace) if start > 2 => allocate_cluster_in_range(device, volume, 2, start),
        Err(err) => Err(err),
    }
}

fn allocate_cluster_in_range(
    device: &mut impl BlockDevice,
    volume: Volume,
    start: u32,
    end: u32,
) -> Result<u32, Error> {
    for cluster in start..end {
        if read_fat_entry(device, volume, cluster)? == 0 {
            write_fat_entry(device, volume, cluster, FAT32_END_MIN)?;
            let _ = update_fs_info_after_alloc(device, volume, cluster);
            return Ok(cluster);
        }
    }
    Err(Error::NoSpace)
}

fn allocate_file_chain(
    device: &mut impl BlockDevice,
    volume: Volume,
    needed_clusters: usize,
) -> Result<u32, Error> {
    let mut first_cluster = 0_u32;
    let mut previous_cluster = 0_u32;

    for index in 0..needed_clusters {
        let cluster = match allocate_cluster(device, volume) {
            Ok(cluster) => cluster,
            Err(err) => {
                if first_cluster >= 2 {
                    let _ = free_cluster_chain(device, volume, first_cluster);
                }
                return Err(err);
            }
        };

        if index == 0 {
            first_cluster = cluster;
        } else if let Err(err) = write_fat_entry(device, volume, previous_cluster, cluster) {
            let _ = free_cluster_chain(device, volume, cluster);
            if first_cluster >= 2 {
                let _ = free_cluster_chain(device, volume, first_cluster);
            }
            return Err(err);
        }
        previous_cluster = cluster;
    }

    Ok(first_cluster)
}

fn write_file_data_chain(
    device: &mut impl BlockDevice,
    volume: Volume,
    first_cluster: u32,
    data: &[u8],
) -> Result<(), Error> {
    if data.is_empty() {
        return Ok(());
    }
    if first_cluster < 2 {
        return Err(Error::CorruptChain);
    }

    let mut remaining = data;
    let mut cluster = first_cluster;
    let mut traversed = 0_u32;
    while !remaining.is_empty() {
        traversed = traversed.checked_add(1).ok_or(Error::ChainTooLong)?;
        if traversed > volume.cluster_count {
            return Err(Error::ChainTooLong);
        }

        let cluster_lba = volume.cluster_lba(cluster)?;
        for sector_index in 0..volume.sectors_per_cluster as u64 {
            let mut sector = [0_u8; SECTOR_SIZE];
            let take = core::cmp::min(SECTOR_SIZE, remaining.len());
            if take > 0 {
                sector[..take].copy_from_slice(&remaining[..take]);
                remaining = &remaining[take..];
            }
            device
                .write_sector(cluster_lba + sector_index, &sector)
                .map_err(Error::Block)?;
        }

        if remaining.is_empty() {
            return Ok(());
        }
        cluster = match next_cluster(device, volume, cluster)? {
            ClusterLink::Next(next) => next,
            ClusterLink::End => return Err(Error::TruncatedFile),
        };
    }
    Ok(())
}

fn zero_cluster(device: &mut impl BlockDevice, volume: Volume, cluster: u32) -> Result<(), Error> {
    let base = volume.cluster_lba(cluster)?;
    let zero = [0_u8; SECTOR_SIZE];
    for sector_index in 0..volume.sectors_per_cluster as u64 {
        device
            .write_sector(base + sector_index, &zero)
            .map_err(Error::Block)?;
    }
    Ok(())
}

fn extend_directory_chain(
    device: &mut impl BlockDevice,
    volume: Volume,
    last_cluster: u32,
) -> Result<DirectorySlotReservation, Error> {
    let new_cluster = allocate_cluster(device, volume)?;
    if let Err(err) = zero_cluster(device, volume, new_cluster) {
        let _ = free_cluster_chain(device, volume, new_cluster);
        return Err(err);
    }
    if let Err(err) = write_fat_entry(device, volume, last_cluster, new_cluster) {
        let _ = free_cluster_chain(device, volume, new_cluster);
        return Err(err);
    }
    Ok(DirectorySlotReservation {
        lba: volume.cluster_lba(new_cluster)?,
        offset: 0,
        existing: None,
        extension: Some(DirectoryExtension {
            parent_cluster: last_cluster,
            cluster: new_cluster,
        }),
    })
}

fn rollback_directory_extension(
    device: &mut impl BlockDevice,
    volume: Volume,
    extension: DirectoryExtension,
) -> Result<(), Error> {
    write_fat_entry(device, volume, extension.parent_cluster, FAT32_END_MIN)?;
    free_cluster_chain(device, volume, extension.cluster)
}

fn rollback_reserved_slot(
    device: &mut impl BlockDevice,
    volume: Volume,
    slot: DirectorySlotReservation,
) {
    if let Some(extension) = slot.extension {
        let _ = rollback_directory_extension(device, volume, extension);
    }
}

/// Create or overwrite a file in the volume root directory.
///
/// `name` must be a valid 8.3 short name component (e.g. `"FOO.TXT"` or `"readme"`).
/// Root directories grow by linking a fresh cluster when all slots are occupied.
/// Data larger than one cluster is written as a FAT chain, with rollback if the
/// replacement data cannot be prepared before the directory entry is published.
#[allow(dead_code)]
pub fn create_root_file(
    device: &mut impl BlockDevice,
    volume: Volume,
    name: &str,
    data: &[u8],
) -> Result<(), Error> {
    create_file_in_directory(device, volume, volume.root_cluster, name, data)
}

pub fn create_file_in_directory(
    device: &mut impl BlockDevice,
    volume: Volume,
    dir_cluster: u32,
    name: &str,
    data: &[u8],
) -> Result<(), Error> {
    let short = encode_short_name(name).ok_or(Error::NameTooLong)?;
    if data.len() > u32::MAX as usize {
        return Err(Error::NoSpace);
    }

    let slot = find_directory_slot(device, volume, dir_cluster, &short)?;
    let existing_first = if let Some(entry) = slot.existing {
        if entry.attributes & DIRECTORY_ATTRIBUTE != 0 {
            return Err(Error::WriteFailed);
        }
        if entry.first_cluster >= 2 {
            validate_cluster_chain(device, volume, entry.first_cluster)?;
        }
        Some(entry.first_cluster)
    } else {
        None
    };

    let bytes_per_cluster = volume.sectors_per_cluster as usize * SECTOR_SIZE;
    let needed_clusters = if data.is_empty() {
        0
    } else {
        data.len().div_ceil(bytes_per_cluster)
    };
    let first_cluster = match allocate_file_chain(device, volume, needed_clusters) {
        Ok(first_cluster) => first_cluster,
        Err(err) => {
            rollback_reserved_slot(device, volume, slot);
            return Err(err);
        }
    };

    if let Err(err) = write_file_data_chain(device, volume, first_cluster, data) {
        if first_cluster >= 2 {
            let _ = free_cluster_chain(device, volume, first_cluster);
        }
        rollback_reserved_slot(device, volume, slot);
        return Err(err);
    }

    if let Err(err) = write_directory_entry(
        device,
        slot.lba,
        slot.offset,
        &short,
        0x20,
        first_cluster,
        data.len() as u32,
    ) {
        if first_cluster >= 2 {
            let _ = free_cluster_chain(device, volume, first_cluster);
        }
        rollback_reserved_slot(device, volume, slot);
        return Err(err);
    }

    if let Some(cluster) = existing_first {
        if cluster >= 2 {
            free_cluster_chain(device, volume, cluster)?;
        }
    }
    Ok(())
}

fn resolve_parent_cluster(
    device: &mut impl BlockDevice,
    volume: Volume,
    path: &str,
    create_missing: bool,
) -> Result<(u32, [u8; 13], usize), Error> {
    let path = path.trim_matches('/');
    if path.is_empty() {
        return Err(Error::NameTooLong);
    }
    let mut parts = [""; 16];
    let mut count = 0usize;
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || count == parts.len() {
            return Err(Error::NameTooLong);
        }
        parts[count] = part;
        count += 1;
    }
    if count == 0 {
        return Err(Error::NameTooLong);
    }
    let leaf = parts[count - 1];
    if encode_short_name(leaf).is_none() {
        return Err(Error::NameTooLong);
    }
    let mut leaf_buf = [0u8; 13];
    leaf_buf[..leaf.len()].copy_from_slice(leaf.as_bytes());

    let mut cluster = volume.root_cluster;
    for component in &parts[..count - 1] {
        let short = encode_short_name(component).ok_or(Error::NameTooLong)?;
        match find_in_directory(device, volume, cluster, &short) {
            Ok(entry) => {
                if entry.attributes & DIRECTORY_ATTRIBUTE == 0 || entry.first_cluster < 2 {
                    return Err(Error::NotFound);
                }
                cluster = entry.first_cluster;
            }
            Err(Error::NotFound) if create_missing => {
                cluster = create_directory_in(device, volume, cluster, component)?;
            }
            Err(err) => return Err(err),
        }
    }
    Ok((cluster, leaf_buf, leaf.len()))
}

fn find_directory_slot(
    device: &mut impl BlockDevice,
    volume: Volume,
    dir_cluster: u32,
    short: &[u8; 11],
) -> Result<DirectorySlotReservation, Error> {
    let mut cluster = dir_cluster;
    let mut visited = [0u32; MAX_DIRECTORY_CLUSTERS];
    let mut visited_count = 0usize;
    let mut deleted: Option<(u64, usize)> = None;
    let mut sector = [0u8; SECTOR_SIZE];
    loop {
        if visited_count == visited.len() {
            return Err(Error::ChainTooLong);
        }
        if visited[..visited_count].contains(&cluster) {
            return Err(Error::ChainLoop);
        }
        visited[visited_count] = cluster;
        visited_count += 1;
        let base = volume.cluster_lba(cluster)?;
        for si in 0..volume.sectors_per_cluster as u64 {
            let lba = base + si;
            device.read_sector(lba, &mut sector).map_err(Error::Block)?;
            for i in 0..DIRECTORY_ENTRIES_PER_SECTOR {
                let off = i * DIRECTORY_ENTRY_SIZE;
                let first = sector[off];
                if first == 0 {
                    let (lba, offset) = deleted.unwrap_or((lba, off));
                    return Ok(DirectorySlotReservation {
                        lba,
                        offset,
                        existing: None,
                        extension: None,
                    });
                }
                if first == 0xe5 {
                    if deleted.is_none() {
                        deleted = Some((lba, off));
                    }
                    continue;
                }
                let attr = sector[off + 11];
                if attr == LONG_NAME_ATTRIBUTE || attr & VOLUME_ID_ATTRIBUTE != 0 {
                    continue;
                }
                let mut found = [0u8; 11];
                found.copy_from_slice(&sector[off..off + 11]);
                if &found == short {
                    let first_cluster = ((read_u16(&sector, off + 20) as u32) << 16)
                        | read_u16(&sector, off + 26) as u32;
                    return Ok(DirectorySlotReservation {
                        lba,
                        offset: off,
                        existing: Some(DirectoryEntry {
                            short_name: found,
                            first_cluster,
                            size: read_u32(&sector, off + 28),
                            attributes: attr,
                        }),
                        extension: None,
                    });
                }
            }
        }
        cluster = match next_cluster(device, volume, cluster)? {
            ClusterLink::Next(n) => n,
            ClusterLink::End => {
                if let Some((lba, offset)) = deleted {
                    return Ok(DirectorySlotReservation {
                        lba,
                        offset,
                        existing: None,
                        extension: None,
                    });
                }
                return extend_directory_chain(device, volume, cluster);
            }
        };
    }
}

fn directory_entry_from_sector(
    sector: &[u8; SECTOR_SIZE],
    offset: usize,
) -> Result<DirectoryEntry, Error> {
    let mut short_name = [0_u8; 11];
    short_name.copy_from_slice(&sector[offset..offset + 11]);
    let first_cluster =
        ((read_u16(sector, offset + 20) as u32) << 16) | read_u16(sector, offset + 26) as u32;
    let size = read_u32(sector, offset + 28);
    let attributes = sector[offset + 11];
    if first_cluster < 2 && (size != 0 || attributes & DIRECTORY_ATTRIBUTE != 0) {
        return Err(Error::CorruptDirectory);
    }
    Ok(DirectoryEntry {
        short_name,
        first_cluster,
        size,
        attributes,
    })
}

fn find_existing_directory_slot(
    device: &mut impl BlockDevice,
    volume: Volume,
    dir_cluster: u32,
    short: &[u8; 11],
) -> Result<DirectorySlot, Error> {
    let mut cluster = dir_cluster;
    let mut visited = [0u32; MAX_DIRECTORY_CLUSTERS];
    let mut visited_count = 0usize;
    let mut sector = [0u8; SECTOR_SIZE];
    loop {
        if visited_count == visited.len() {
            return Err(Error::ChainTooLong);
        }
        if visited[..visited_count].contains(&cluster) {
            return Err(Error::ChainLoop);
        }
        visited[visited_count] = cluster;
        visited_count += 1;

        let base = volume.cluster_lba(cluster)?;
        for sector_index in 0..volume.sectors_per_cluster as u64 {
            let lba = base + sector_index;
            device.read_sector(lba, &mut sector).map_err(Error::Block)?;
            for index in 0..DIRECTORY_ENTRIES_PER_SECTOR {
                let offset = index * DIRECTORY_ENTRY_SIZE;
                let first = sector[offset];
                if first == 0 {
                    return Err(Error::NotFound);
                }
                if first == 0xe5 {
                    continue;
                }
                let attributes = sector[offset + 11];
                if attributes == LONG_NAME_ATTRIBUTE || attributes & VOLUME_ID_ATTRIBUTE != 0 {
                    continue;
                }
                let mut found = [0u8; 11];
                found.copy_from_slice(&sector[offset..offset + 11]);
                if &found == short {
                    let entry = directory_entry_from_sector(&sector, offset)?;
                    let mut raw = [0u8; DIRECTORY_ENTRY_SIZE];
                    raw.copy_from_slice(&sector[offset..offset + DIRECTORY_ENTRY_SIZE]);
                    return Ok(DirectorySlot {
                        lba,
                        offset,
                        entry,
                        raw,
                    });
                }
            }
        }
        cluster = match next_cluster(device, volume, cluster)? {
            ClusterLink::Next(next) => next,
            ClusterLink::End => return Err(Error::NotFound),
        };
    }
}

fn mark_directory_entry_deleted(
    device: &mut impl BlockDevice,
    lba: u64,
    offset: usize,
) -> Result<(), Error> {
    let mut sector = [0u8; SECTOR_SIZE];
    device.read_sector(lba, &mut sector).map_err(Error::Block)?;
    sector[offset] = 0xe5;
    device.write_sector(lba, &sector).map_err(Error::Block)
}

fn write_short_name(
    device: &mut impl BlockDevice,
    lba: u64,
    offset: usize,
    short: &[u8; 11],
) -> Result<(), Error> {
    let mut sector = [0u8; SECTOR_SIZE];
    device.read_sector(lba, &mut sector).map_err(Error::Block)?;
    sector[offset..offset + 11].copy_from_slice(short);
    device.write_sector(lba, &sector).map_err(Error::Block)
}

fn write_raw_directory_slot(
    device: &mut impl BlockDevice,
    lba: u64,
    offset: usize,
    raw: &[u8; DIRECTORY_ENTRY_SIZE],
) -> Result<(), Error> {
    let mut sector = [0u8; SECTOR_SIZE];
    device.read_sector(lba, &mut sector).map_err(Error::Block)?;
    sector[offset..offset + DIRECTORY_ENTRY_SIZE].copy_from_slice(raw);
    device.write_sector(lba, &sector).map_err(Error::Block)
}

fn directory_is_empty(
    device: &mut impl BlockDevice,
    volume: Volume,
    dir_cluster: u32,
) -> Result<bool, Error> {
    let mut cluster = dir_cluster;
    let mut visited = [0u32; MAX_DIRECTORY_CLUSTERS];
    let mut visited_count = 0usize;
    let mut sector = [0u8; SECTOR_SIZE];
    loop {
        if visited_count == visited.len() {
            return Err(Error::ChainTooLong);
        }
        if visited[..visited_count].contains(&cluster) {
            return Err(Error::ChainLoop);
        }
        visited[visited_count] = cluster;
        visited_count += 1;

        let base = volume.cluster_lba(cluster)?;
        for sector_index in 0..volume.sectors_per_cluster as u64 {
            device
                .read_sector(base + sector_index, &mut sector)
                .map_err(Error::Block)?;
            for index in 0..DIRECTORY_ENTRIES_PER_SECTOR {
                let offset = index * DIRECTORY_ENTRY_SIZE;
                let first = sector[offset];
                if first == 0 {
                    return Ok(true);
                }
                if first == 0xe5 {
                    continue;
                }
                let attributes = sector[offset + 11];
                if attributes == LONG_NAME_ATTRIBUTE || attributes & VOLUME_ID_ATTRIBUTE != 0 {
                    continue;
                }
                let name = &sector[offset..offset + 11];
                if name == b".          " || name == b"..         " {
                    continue;
                }
                return Ok(false);
            }
        }
        cluster = match next_cluster(device, volume, cluster)? {
            ClusterLink::Next(next) => next,
            ClusterLink::End => return Ok(true),
        };
    }
}

fn update_dotdot(
    device: &mut impl BlockDevice,
    volume: Volume,
    dir_cluster: u32,
    parent_cluster: u32,
) -> Result<(), Error> {
    let base = volume.cluster_lba(dir_cluster)?;
    let mut sector = [0u8; SECTOR_SIZE];
    device
        .read_sector(base, &mut sector)
        .map_err(Error::Block)?;
    for index in 0..DIRECTORY_ENTRIES_PER_SECTOR {
        let offset = index * DIRECTORY_ENTRY_SIZE;
        let first = sector[offset];
        if first == 0 {
            break;
        }
        if first == 0xe5 {
            continue;
        }
        if &sector[offset..offset + 11] == b"..         " {
            write_u16(&mut sector, offset + 20, (parent_cluster >> 16) as u16);
            write_u16(&mut sector, offset + 26, parent_cluster as u16);
            device.write_sector(base, &sector).map_err(Error::Block)?;
            return Ok(());
        }
    }
    Err(Error::CorruptDirectory)
}

fn path_is_descendant(parent: &str, child: &str) -> bool {
    let parent = parent.trim_matches('/');
    let child = child.trim_matches('/');
    !parent.is_empty()
        && child.len() > parent.len()
        && child.as_bytes().starts_with(parent.as_bytes())
        && child.as_bytes()[parent.len()] == b'/'
}

fn write_directory_entry(
    device: &mut impl BlockDevice,
    lba: u64,
    offset: usize,
    short: &[u8; 11],
    attributes: u8,
    first_cluster: u32,
    size: u32,
) -> Result<(), Error> {
    let mut sector = [0u8; SECTOR_SIZE];
    device.read_sector(lba, &mut sector).map_err(Error::Block)?;
    for b in &mut sector[offset..offset + DIRECTORY_ENTRY_SIZE] {
        *b = 0;
    }
    sector[offset..offset + 11].copy_from_slice(short);
    sector[offset + 11] = attributes;
    write_u16(&mut sector, offset + 20, (first_cluster >> 16) as u16);
    write_u16(&mut sector, offset + 26, first_cluster as u16);
    write_u32(&mut sector, offset + 28, size);
    device.write_sector(lba, &sector).map_err(Error::Block)
}

fn create_directory_in(
    device: &mut impl BlockDevice,
    volume: Volume,
    parent_cluster: u32,
    name: &str,
) -> Result<u32, Error> {
    let short = encode_short_name(name).ok_or(Error::NameTooLong)?;
    let slot = find_directory_slot(device, volume, parent_cluster, &short)?;
    if let Some(entry) = slot.existing {
        if entry.attributes & DIRECTORY_ATTRIBUTE != 0 && entry.first_cluster >= 2 {
            return Ok(entry.first_cluster);
        }
        return Err(Error::WriteFailed);
    }

    let cluster = match allocate_cluster(device, volume) {
        Ok(cluster) => cluster,
        Err(err) => {
            rollback_reserved_slot(device, volume, slot);
            return Err(err);
        }
    };
    if let Err(err) = zero_cluster(device, volume, cluster) {
        let _ = free_cluster_chain(device, volume, cluster);
        rollback_reserved_slot(device, volume, slot);
        return Err(err);
    }

    let base = match volume.cluster_lba(cluster) {
        Ok(base) => base,
        Err(err) => {
            let _ = free_cluster_chain(device, volume, cluster);
            rollback_reserved_slot(device, volume, slot);
            return Err(err);
        }
    };

    let mut first = [0u8; SECTOR_SIZE];
    first[0..11].copy_from_slice(b".          ");
    first[11] = DIRECTORY_ATTRIBUTE;
    write_u16(&mut first, 20, (cluster >> 16) as u16);
    write_u16(&mut first, 26, cluster as u16);
    first[32..43].copy_from_slice(b"..         ");
    first[43] = DIRECTORY_ATTRIBUTE;
    let dotdot = if parent_cluster == volume.root_cluster {
        volume.root_cluster
    } else {
        parent_cluster
    };
    write_u16(&mut first, 32 + 20, (dotdot >> 16) as u16);
    write_u16(&mut first, 32 + 26, dotdot as u16);
    if let Err(err) = device.write_sector(base, &first).map_err(Error::Block) {
        let _ = free_cluster_chain(device, volume, cluster);
        rollback_reserved_slot(device, volume, slot);
        return Err(err);
    }
    if let Err(err) = write_directory_entry(
        device,
        slot.lba,
        slot.offset,
        &short,
        DIRECTORY_ATTRIBUTE,
        cluster,
        0,
    ) {
        let _ = free_cluster_chain(device, volume, cluster);
        rollback_reserved_slot(device, volume, slot);
        return Err(err);
    }
    Ok(cluster)
}

/// Create a FAT32 directory path, creating any missing 8.3 components.
pub fn mkdir_path(device: &mut impl BlockDevice, volume: Volume, path: &str) -> Result<(), Error> {
    let path = path.trim_matches('/');
    if path.is_empty() {
        return Ok(());
    }
    let mut cluster = volume.root_cluster;
    for component in path.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            return Err(Error::NameTooLong);
        }
        let short = encode_short_name(component).ok_or(Error::NameTooLong)?;
        cluster = match find_in_directory(device, volume, cluster, &short) {
            Ok(entry)
                if entry.attributes & DIRECTORY_ATTRIBUTE != 0 && entry.first_cluster >= 2 =>
            {
                entry.first_cluster
            }
            Ok(_) => return Err(Error::WriteFailed),
            Err(Error::NotFound) => create_directory_in(device, volume, cluster, component)?,
            Err(err) => return Err(err),
        };
    }
    Ok(())
}

/// Create or overwrite a file at a multi-component FAT32 path. Parent directories
/// are created automatically. Components currently use FAT 8.3 names.
pub fn create_path_file(
    device: &mut impl BlockDevice,
    volume: Volume,
    path: &str,
    data: &[u8],
) -> Result<(), Error> {
    let (parent, leaf, leaf_len) = resolve_parent_cluster(device, volume, path, true)?;
    let leaf = core::str::from_utf8(&leaf[..leaf_len]).map_err(|_| Error::NameTooLong)?;
    create_file_in_directory(device, volume, parent, leaf, data)
}

/// Delete a file or empty directory at a multi-component FAT32 path.
pub fn delete_path(device: &mut impl BlockDevice, volume: Volume, path: &str) -> Result<(), Error> {
    let path = path.trim_matches('/');
    if path.is_empty() {
        return Err(Error::InvalidPath);
    }
    let (parent, leaf, leaf_len) = resolve_parent_cluster(device, volume, path, false)?;
    let leaf = core::str::from_utf8(&leaf[..leaf_len]).map_err(|_| Error::NameTooLong)?;
    let short = encode_short_name(leaf).ok_or(Error::NameTooLong)?;
    let slot = find_existing_directory_slot(device, volume, parent, &short)?;
    if slot.entry.attributes & READ_ONLY_ATTRIBUTE != 0 {
        return Err(Error::ReadOnly);
    }
    let is_directory = slot.entry.attributes & DIRECTORY_ATTRIBUTE != 0;
    if is_directory {
        if !directory_is_empty(device, volume, slot.entry.first_cluster)? {
            return Err(Error::DirectoryNotEmpty);
        }
        validate_cluster_chain(device, volume, slot.entry.first_cluster)?;
    } else if slot.entry.first_cluster >= 2 {
        validate_cluster_chain(device, volume, slot.entry.first_cluster)?;
    }

    mark_directory_entry_deleted(device, slot.lba, slot.offset)?;
    if slot.entry.first_cluster >= 2 {
        free_cluster_chain(device, volume, slot.entry.first_cluster)?;
    }
    Ok(())
}

/// Rename a file or directory at a multi-component FAT32 path.
pub fn rename_path(
    device: &mut impl BlockDevice,
    volume: Volume,
    old: &str,
    new: &str,
) -> Result<(), Error> {
    let old_path = old.trim_matches('/');
    let new_path = new.trim_matches('/');
    if old_path.is_empty() || new_path.is_empty() {
        return Err(Error::InvalidPath);
    }

    let (old_parent, old_leaf, old_leaf_len) =
        resolve_parent_cluster(device, volume, old_path, false)?;
    let old_leaf =
        core::str::from_utf8(&old_leaf[..old_leaf_len]).map_err(|_| Error::NameTooLong)?;
    let old_short = encode_short_name(old_leaf).ok_or(Error::NameTooLong)?;
    let old_slot = find_existing_directory_slot(device, volume, old_parent, &old_short)?;
    if old_slot.entry.attributes & READ_ONLY_ATTRIBUTE != 0 {
        return Err(Error::ReadOnly);
    }
    let is_directory = old_slot.entry.attributes & DIRECTORY_ATTRIBUTE != 0;
    if is_directory && path_is_descendant(old_path, new_path) {
        return Err(Error::InvalidPath);
    }
    if is_directory && old_slot.entry.first_cluster < 2 {
        return Err(Error::CorruptDirectory);
    }

    let (new_parent, new_leaf, new_leaf_len) =
        resolve_parent_cluster(device, volume, new_path, false)?;
    let new_leaf =
        core::str::from_utf8(&new_leaf[..new_leaf_len]).map_err(|_| Error::NameTooLong)?;
    let new_short = encode_short_name(new_leaf).ok_or(Error::NameTooLong)?;

    if old_parent == new_parent && old_short == new_short {
        return Ok(());
    }

    match find_existing_directory_slot(device, volume, new_parent, &new_short) {
        Ok(_) => return Err(Error::AlreadyExists),
        Err(Error::NotFound) => {}
        Err(err) => return Err(err),
    }

    if old_parent == new_parent {
        return write_short_name(device, old_slot.lba, old_slot.offset, &new_short);
    }

    let new_slot = find_directory_slot(device, volume, new_parent, &new_short)?;
    if new_slot.existing.is_some() {
        return Err(Error::AlreadyExists);
    }
    let mut raw = old_slot.raw;
    raw[..11].copy_from_slice(&new_short);
    if let Err(err) = write_raw_directory_slot(device, new_slot.lba, new_slot.offset, &raw) {
        rollback_reserved_slot(device, volume, new_slot);
        return Err(err);
    }
    if is_directory {
        if let Err(err) = update_dotdot(device, volume, old_slot.entry.first_cluster, new_parent) {
            let _ = mark_directory_entry_deleted(device, new_slot.lba, new_slot.offset);
            rollback_reserved_slot(device, volume, new_slot);
            return Err(err);
        }
    }
    if let Err(err) = mark_directory_entry_deleted(device, old_slot.lba, old_slot.offset) {
        let _ = mark_directory_entry_deleted(device, new_slot.lba, new_slot.offset);
        if is_directory {
            let _ = update_dotdot(device, volume, old_slot.entry.first_cluster, old_parent);
        }
        rollback_reserved_slot(device, volume, new_slot);
        return Err(err);
    }
    Ok(())
}

struct TestDisk {
    valid_signature: bool,
    cyclic_chain: bool,
}

impl TestDisk {
    const TOTAL_SECTORS: u64 = 70_000;
    const FAT_LBA: u64 = 32;
    const ROOT_LBA: u64 = 1_232;
    const FILE_LBA: u64 = 1_233;
}

impl BlockDevice for TestDisk {
    fn sector_count(&self) -> u64 {
        Self::TOTAL_SECTORS
    }

    fn read_sector(&mut self, lba: u64, sector: &mut [u8]) -> Result<(), BlockError> {
        if sector.len() != SECTOR_SIZE || lba >= Self::TOTAL_SECTORS {
            return Err(BlockError::OutOfBounds);
        }
        sector.fill(0);
        if lba == 0 {
            sector[11..13].copy_from_slice(&(SECTOR_SIZE as u16).to_le_bytes());
            sector[13] = 1;
            sector[14..16].copy_from_slice(&32_u16.to_le_bytes());
            sector[16] = 2;
            sector[32..36].copy_from_slice(&(Self::TOTAL_SECTORS as u32).to_le_bytes());
            sector[36..40].copy_from_slice(&600_u32.to_le_bytes());
            sector[44..48].copy_from_slice(&2_u32.to_le_bytes());
            if self.valid_signature {
                sector[510] = 0x55;
                sector[511] = 0xaa;
            }
        } else if lba == Self::FAT_LBA {
            sector[8..12].copy_from_slice(&5_u32.to_le_bytes());
            if self.cyclic_chain {
                sector[12..16].copy_from_slice(&3_u32.to_le_bytes());
            } else {
                sector[12..16].copy_from_slice(&4_u32.to_le_bytes());
                sector[16..20].copy_from_slice(&FAT32_END_MIN.to_le_bytes());
            }
            sector[20..24].copy_from_slice(&FAT32_END_MIN.to_le_bytes());
        } else if lba == Self::ROOT_LBA {
            for index in 0..DIRECTORY_ENTRIES_PER_SECTOR {
                sector[index * DIRECTORY_ENTRY_SIZE] = 0xe5;
            }
        } else if lba == Self::ROOT_LBA + 3 {
            sector[..11].copy_from_slice(b"KERNEL  BIN");
            sector[11] = 0x20;
            sector[26..28].copy_from_slice(&3_u16.to_le_bytes());
            let size = if self.cyclic_chain { 1024_u32 } else { 600_u32 };
            sector[28..32].copy_from_slice(&size.to_le_bytes());
        } else if lba == Self::FILE_LBA {
            sector.fill(b'A');
        } else if lba == Self::FILE_LBA + 1 {
            sector.fill(b'B');
        }
        Ok(())
    }

    fn write_sector(&mut self, _lba: u64, _sector: &[u8]) -> Result<(), BlockError> {
        Err(BlockError::ReadOnly)
    }
}

struct RootCycleDisk(TestDisk);

impl BlockDevice for RootCycleDisk {
    fn sector_count(&self) -> u64 {
        self.0.sector_count()
    }

    fn read_sector(&mut self, lba: u64, sector: &mut [u8]) -> Result<(), BlockError> {
        self.0.read_sector(lba, sector)?;
        if lba == TestDisk::FAT_LBA {
            sector[8..12].copy_from_slice(&2_u32.to_le_bytes());
        }
        Ok(())
    }

    fn write_sector(&mut self, lba: u64, sector: &[u8]) -> Result<(), BlockError> {
        self.0.write_sector(lba, sector)
    }
}

pub fn self_test() -> bool {
    let mut disk = TestDisk {
        valid_signature: true,
        cyclic_chain: false,
    };
    let Ok(volume) = mount(&mut disk) else {
        return false;
    };
    let Ok(entry) = find_root(&mut disk, volume, b"KERNEL  BIN") else {
        return false;
    };
    let mut entries = [None; 2];
    let listed = list_root(&mut disk, volume, &mut entries) == Ok(1)
        && entries[0].is_some_and(|listed| listed.short_name == entry.short_name);
    let listing_capacity_enforced =
        list_root(&mut disk, volume, &mut []) == Err(Error::DirectoryFull);
    let mut payload = [0_u8; 600];
    let valid = listed
        && listing_capacity_enforced
        && volume.total_sectors == TestDisk::TOTAL_SECTORS as u32
        && volume.fat_count == 2
        && volume.fat_size == 600
        && volume.cluster_lba(volume.root_cluster) == Ok(TestDisk::ROOT_LBA)
        && next_cluster(&mut disk, volume, volume.root_cluster) == Ok(ClusterLink::Next(5))
        && entry.short_name == *b"KERNEL  BIN"
        && entry.first_cluster == 3
        && entry.size == 600
        && entry.attributes == 0x20
        && next_cluster(&mut disk, volume, 3) == Ok(ClusterLink::Next(4))
        && next_cluster(&mut disk, volume, 4) == Ok(ClusterLink::End)
        && read_file(&mut disk, volume, entry, &mut payload) == Ok(payload.len())
        && payload[..SECTOR_SIZE].iter().all(|byte| *byte == b'A')
        && payload[SECTOR_SIZE..].iter().all(|byte| *byte == b'B')
        && find_root(&mut disk, volume, b"MISSING TXT") == Err(Error::NotFound);

    let mut invalid = TestDisk {
        valid_signature: false,
        cyclic_chain: false,
    };
    let invalid_rejected = mount(&mut invalid) == Err(Error::InvalidBootSector);

    let mut cyclic = TestDisk {
        valid_signature: true,
        cyclic_chain: true,
    };
    let Ok(cyclic_volume) = mount(&mut cyclic) else {
        return false;
    };
    let Ok(cyclic_entry) = find_root(&mut cyclic, cyclic_volume, b"KERNEL  BIN") else {
        return false;
    };
    let mut oversized = [0_u8; 1024];
    let cycle_rejected = read_file(&mut cyclic, cyclic_volume, cyclic_entry, &mut oversized)
        == Err(Error::ChainLoop);

    let mut root_cycle = RootCycleDisk(TestDisk {
        valid_signature: true,
        cyclic_chain: false,
    });
    let root_cycle_rejected = mount(&mut root_cycle).is_ok_and(|volume| {
        find_root(&mut root_cycle, volume, b"MISSING TXT") == Err(Error::ChainLoop)
    });

    valid
        && invalid_rejected
        && cycle_rejected
        && root_cycle_rejected
        && range_self_test()
        && streaming_read_self_test()
        && mutation_self_test()
        && directory_growth_self_test()
        && overwrite_rollback_self_test()
        && directory_extension_rollback_self_test()
}

fn range_self_test() -> bool {
    let mut disk = TestDisk {
        valid_signature: true,
        cyclic_chain: false,
    };
    let Ok(volume) = mount(&mut disk) else {
        return false;
    };
    let Ok(entry) = find_root(&mut disk, volume, b"KERNEL  BIN") else {
        return false;
    };
    let mut data = [0; 32];
    if read_file_at(&mut disk, volume, entry, 500, &mut data) != Ok(32)
        || data[..12] != [b'A'; 12]
        || data[12..] != [b'B'; 20]
    {
        return false;
    }
    data.fill(0);
    read_file_at(&mut disk, volume, entry, 590, &mut data) == Ok(10)
        && data[..10] == [b'B'; 10]
        && data[10..] == [0; 22]
        && read_file_at(&mut disk, volume, entry, 600, &mut data) == Ok(0)
        && read_file_at(&mut disk, volume, entry, usize::MAX, &mut data) == Ok(0)
}

struct StreamingReadDisk;

impl BlockDevice for StreamingReadDisk {
    fn sector_count(&self) -> u64 {
        TestDisk::TOTAL_SECTORS
    }

    fn read_sector(&mut self, lba: u64, sector: &mut [u8]) -> Result<(), BlockError> {
        if sector.len() != SECTOR_SIZE || lba >= TestDisk::TOTAL_SECTORS {
            return Err(BlockError::OutOfBounds);
        }
        sector.fill(0);
        if lba == 0 {
            sector[11..13].copy_from_slice(&(SECTOR_SIZE as u16).to_le_bytes());
            sector[13] = 1;
            sector[14..16].copy_from_slice(&32_u16.to_le_bytes());
            sector[16] = 2;
            sector[32..36].copy_from_slice(&(TestDisk::TOTAL_SECTORS as u32).to_le_bytes());
            sector[36..40].copy_from_slice(&600_u32.to_le_bytes());
            sector[44..48].copy_from_slice(&2_u32.to_le_bytes());
            sector[510] = 0x55;
            sector[511] = 0xaa;
        } else if (TestDisk::FAT_LBA..TestDisk::FAT_LBA + 600).contains(&lba) {
            for index in 0..128 {
                let cluster = ((lba - TestDisk::FAT_LBA) * 128 + index) as u32;
                let next = if (3..260).contains(&cluster) {
                    cluster + 1
                } else {
                    FAT32_END_MIN
                };
                let offset = index as usize * 4;
                sector[offset..offset + 4].copy_from_slice(&next.to_le_bytes());
            }
        } else if (TestDisk::FILE_LBA..TestDisk::FILE_LBA + 258).contains(&lba) {
            for (index, byte) in sector.iter_mut().enumerate() {
                *byte = (((lba - TestDisk::FILE_LBA) as usize * SECTOR_SIZE + index) % 251) as u8;
            }
        } else {
            return Err(BlockError::OutOfBounds);
        }
        Ok(())
    }

    fn write_sector(&mut self, _lba: u64, _sector: &[u8]) -> Result<(), BlockError> {
        Err(BlockError::ReadOnly)
    }
}

fn streaming_read_self_test() -> bool {
    let mut disk = StreamingReadDisk;
    let Ok(volume) = mount(&mut disk) else {
        return false;
    };
    let entry = DirectoryEntry {
        short_name: *b"STREAM  BIN",
        first_cluster: 3,
        size: 260 * SECTOR_SIZE as u32,
        attributes: 0x20,
    };
    let offset = 140 * SECTOR_SIZE + 11;
    let mut data = [0_u8; 73];
    if read_file_at(&mut disk, volume, entry, offset, &mut data) != Ok(data.len()) {
        return false;
    }
    data.iter()
        .enumerate()
        .all(|(index, byte)| *byte == ((offset + index) % 251) as u8)
}

struct MutableFatDisk {
    boot: [u8; SECTOR_SIZE],
    fs_info: [u8; SECTOR_SIZE],
    backup_fs_info: [u8; SECTOR_SIZE],
    fat0: [u8; SECTOR_SIZE],
    fat1: [u8; SECTOR_SIZE],
    root: [u8; SECTOR_SIZE],
    data: [[u8; SECTOR_SIZE]; 128],
}

impl MutableFatDisk {
    fn new() -> Self {
        let mut disk = Self {
            boot: [0; SECTOR_SIZE],
            fs_info: [0; SECTOR_SIZE],
            backup_fs_info: [0; SECTOR_SIZE],
            fat0: [0; SECTOR_SIZE],
            fat1: [0; SECTOR_SIZE],
            root: [0; SECTOR_SIZE],
            data: [[0; SECTOR_SIZE]; 128],
        };
        disk.boot[11..13].copy_from_slice(&(SECTOR_SIZE as u16).to_le_bytes());
        disk.boot[13] = 1;
        disk.boot[14..16].copy_from_slice(&32_u16.to_le_bytes());
        disk.boot[16] = 2;
        disk.boot[32..36].copy_from_slice(&(TestDisk::TOTAL_SECTORS as u32).to_le_bytes());
        disk.boot[36..40].copy_from_slice(&600_u32.to_le_bytes());
        disk.boot[44..48].copy_from_slice(&2_u32.to_le_bytes());
        disk.boot[48..50].copy_from_slice(&1_u16.to_le_bytes());
        disk.boot[50..52].copy_from_slice(&6_u16.to_le_bytes());
        disk.boot[510] = 0x55;
        disk.boot[511] = 0xaa;
        write_u32(&mut disk.fs_info, 0, FSINFO_LEAD_SIGNATURE);
        write_u32(&mut disk.fs_info, 484, FSINFO_STRUCT_SIGNATURE);
        write_u32(
            &mut disk.fs_info,
            FSINFO_FREE_COUNT_OFFSET,
            TestDisk::TOTAL_SECTORS as u32 - TestDisk::ROOT_LBA as u32 - 1,
        );
        write_u32(&mut disk.fs_info, FSINFO_NEXT_FREE_OFFSET, 3);
        write_u32(&mut disk.fs_info, 508, FSINFO_TRAIL_SIGNATURE);
        disk.backup_fs_info.copy_from_slice(&disk.fs_info);
        write_u32(&mut disk.fat0, 8, FAT32_END_MIN);
        write_u32(&mut disk.fat1, 8, FAT32_END_MIN);
        disk
    }

    fn data_slot(lba: u64) -> Option<usize> {
        let start = TestDisk::FILE_LBA;
        if (start..start + 128).contains(&lba) {
            Some((lba - start) as usize)
        } else {
            None
        }
    }
}

impl BlockDevice for MutableFatDisk {
    fn sector_count(&self) -> u64 {
        TestDisk::TOTAL_SECTORS
    }

    fn read_sector(&mut self, lba: u64, sector: &mut [u8]) -> Result<(), BlockError> {
        if sector.len() != SECTOR_SIZE || lba >= TestDisk::TOTAL_SECTORS {
            return Err(BlockError::OutOfBounds);
        }
        sector.fill(0);
        if lba == 0 {
            sector.copy_from_slice(&self.boot);
        } else if lba == 1 {
            sector.copy_from_slice(&self.fs_info);
        } else if lba == 7 {
            sector.copy_from_slice(&self.backup_fs_info);
        } else if lba == TestDisk::FAT_LBA {
            sector.copy_from_slice(&self.fat0);
        } else if lba == TestDisk::FAT_LBA + 600 {
            sector.copy_from_slice(&self.fat1);
        } else if lba == TestDisk::ROOT_LBA {
            sector.copy_from_slice(&self.root);
        } else if let Some(index) = Self::data_slot(lba) {
            sector.copy_from_slice(&self.data[index]);
        }
        Ok(())
    }

    fn write_sector(&mut self, lba: u64, sector: &[u8]) -> Result<(), BlockError> {
        if sector.len() != SECTOR_SIZE || lba >= TestDisk::TOTAL_SECTORS {
            return Err(BlockError::OutOfBounds);
        }
        if lba == 0 {
            self.boot.copy_from_slice(sector);
        } else if lba == 1 {
            self.fs_info.copy_from_slice(sector);
        } else if lba == 7 {
            self.backup_fs_info.copy_from_slice(sector);
        } else if lba == TestDisk::FAT_LBA {
            self.fat0.copy_from_slice(sector);
        } else if lba == TestDisk::FAT_LBA + 600 {
            self.fat1.copy_from_slice(sector);
        } else if lba == TestDisk::ROOT_LBA {
            self.root.copy_from_slice(sector);
        } else if let Some(index) = Self::data_slot(lba) {
            self.data[index].copy_from_slice(sector);
        } else {
            return Err(BlockError::OutOfBounds);
        }
        Ok(())
    }
}

fn read_path_bytes(
    disk: &mut MutableFatDisk,
    volume: Volume,
    path: &str,
    out: &mut [u8],
) -> Result<usize, Error> {
    let entry = resolve_path(disk, volume, path)?;
    read_file(disk, volume, entry, out)
}

fn mutation_self_test() -> bool {
    let mut disk = MutableFatDisk::new();
    let Ok(volume) = mount(&mut disk) else {
        return false;
    };

    if create_path_file(&mut disk, volume, "a.txt", b"one").is_err()
        || create_path_file(&mut disk, volume, "docs/a.txt", b"two").is_err()
        || mkdir_path(&mut disk, volume, "other").is_err()
    {
        return false;
    }

    let mut bytes = [0u8; 8];
    let file_rename = rename_path(&mut disk, volume, "a.txt", "b.txt").is_ok()
        && resolve_path(&mut disk, volume, "a.txt") == Err(Error::NotFound)
        && read_path_bytes(&mut disk, volume, "b.txt", &mut bytes) == Ok(3)
        && &bytes[..3] == b"one";

    let collision_rejected =
        rename_path(&mut disk, volume, "docs/a.txt", "b.txt") == Err(Error::AlreadyExists);

    let same_parent_dir_rename = rename_path(&mut disk, volume, "docs", "archive").is_ok()
        && resolve_path(&mut disk, volume, "docs/a.txt") == Err(Error::NotFound)
        && read_path_bytes(&mut disk, volume, "archive/a.txt", &mut bytes) == Ok(3)
        && &bytes[..3] == b"two";

    let dir_move = rename_path(&mut disk, volume, "archive", "other/docs").is_ok()
        && resolve_path(&mut disk, volume, "archive/a.txt") == Err(Error::NotFound)
        && read_path_bytes(&mut disk, volume, "other/docs/a.txt", &mut bytes) == Ok(3)
        && &bytes[..3] == b"two";

    let non_empty_rejected =
        delete_path(&mut disk, volume, "other/docs") == Err(Error::DirectoryNotEmpty);
    let file_deleted = delete_path(&mut disk, volume, "other/docs/a.txt").is_ok()
        && resolve_path(&mut disk, volume, "other/docs/a.txt") == Err(Error::NotFound);
    let file_cluster_freed = read_fat_entry(&mut disk, volume, 5) == Ok(0);
    let dir_deleted = delete_path(&mut disk, volume, "other/docs").is_ok()
        && resolve_path(&mut disk, volume, "other/docs") == Err(Error::NotFound);
    let dir_cluster_freed = read_fat_entry(&mut disk, volume, 4) == Ok(0);

    file_rename
        && collision_rejected
        && same_parent_dir_rename
        && dir_move
        && non_empty_rejected
        && file_deleted
        && file_cluster_freed
        && dir_deleted
        && dir_cluster_freed
}

fn fs_info_free_count_for_test(disk: &mut MutableFatDisk, volume: Volume) -> Option<u32> {
    read_fs_info(disk, volume)
        .ok()
        .flatten()
        .map(|info| info.free_count)
}

fn numbered_leaf(prefix: u8, index: usize, out: &mut [u8; 7]) -> Option<&str> {
    if index >= 100 {
        return None;
    }
    out[0] = prefix;
    out[1] = b'0' + (index / 10) as u8;
    out[2] = b'0' + (index % 10) as u8;
    out[3] = b'.';
    out[4] = b't';
    out[5] = b'x';
    out[6] = b't';
    core::str::from_utf8(out).ok()
}

fn numbered_child_path<'a>(
    dir: &str,
    prefix: u8,
    index: usize,
    out: &'a mut [u8; 24],
) -> Option<&'a str> {
    let mut leaf = [0_u8; 7];
    let leaf = numbered_leaf(prefix, index, &mut leaf)?;
    let dir_bytes = dir.as_bytes();
    let total = dir_bytes.len().checked_add(1)?.checked_add(leaf.len())?;
    if total > out.len() {
        return None;
    }
    out[..dir_bytes.len()].copy_from_slice(dir_bytes);
    out[dir_bytes.len()] = b'/';
    out[dir_bytes.len() + 1..total].copy_from_slice(leaf.as_bytes());
    core::str::from_utf8(&out[..total]).ok()
}

fn directory_growth_self_test() -> bool {
    let mut disk = MutableFatDisk::new();
    let Ok(volume) = mount(&mut disk) else {
        return false;
    };
    let Some(initial_free) = fs_info_free_count_for_test(&mut disk, volume) else {
        return false;
    };

    for index in 0..18 {
        let mut name = [0_u8; 7];
        let Some(name) = numbered_leaf(b'f', index, &mut name) else {
            return false;
        };
        if create_path_file(&mut disk, volume, name, b"x").is_err() {
            return false;
        }
    }
    let root_extended = next_cluster(&mut disk, volume, volume.root_cluster)
        .is_ok_and(|link| matches!(link, ClusterLink::Next(_)));
    let mut byte = [0_u8; 1];
    if !root_extended
        || read_path_bytes(&mut disk, volume, "f17.txt", &mut byte) != Ok(1)
        || byte[0] != b'x'
    {
        return false;
    }

    if mkdir_path(&mut disk, volume, "big").is_err() {
        return false;
    }
    for index in 0..15 {
        let mut path = [0_u8; 24];
        let Some(path) = numbered_child_path("big", b'g', index, &mut path) else {
            return false;
        };
        if create_path_file(&mut disk, volume, path, b"y").is_err() {
            return false;
        }
    }
    let Ok(big) = resolve_path(&mut disk, volume, "big") else {
        return false;
    };
    let big_extended = next_cluster(&mut disk, volume, big.first_cluster)
        .is_ok_and(|link| matches!(link, ClusterLink::Next(_)));
    if !big_extended
        || read_path_bytes(&mut disk, volume, "big/g14.txt", &mut byte) != Ok(1)
        || byte[0] != b'y'
    {
        return false;
    }

    if mkdir_path(&mut disk, volume, "dst").is_err() {
        return false;
    }
    for index in 0..14 {
        let mut path = [0_u8; 24];
        let Some(path) = numbered_child_path("dst", b'd', index, &mut path) else {
            return false;
        };
        if create_path_file(&mut disk, volume, path, b"q").is_err() {
            return false;
        }
    }
    if create_path_file(&mut disk, volume, "move.txt", b"z").is_err()
        || rename_path(&mut disk, volume, "move.txt", "dst/move.txt").is_err()
        || resolve_path(&mut disk, volume, "move.txt") != Err(Error::NotFound)
        || read_path_bytes(&mut disk, volume, "dst/move.txt", &mut byte) != Ok(1)
        || byte[0] != b'z'
    {
        return false;
    }
    let Ok(dst) = resolve_path(&mut disk, volume, "dst") else {
        return false;
    };
    let dst_extended = next_cluster(&mut disk, volume, dst.first_cluster)
        .is_ok_and(|link| matches!(link, ClusterLink::Next(_)));
    if !dst_extended {
        return false;
    }

    let Some(after_growth) = fs_info_free_count_for_test(&mut disk, volume) else {
        return false;
    };
    if after_growth != initial_free.saturating_sub(53) {
        return false;
    }
    delete_path(&mut disk, volume, "dst/move.txt").is_ok()
        && fs_info_free_count_for_test(&mut disk, volume) == Some(after_growth + 1)
}

struct FailingWriteDisk {
    inner: MutableFatDisk,
    fail_lba: u64,
    failed: bool,
}

impl BlockDevice for FailingWriteDisk {
    fn sector_count(&self) -> u64 {
        self.inner.sector_count()
    }

    fn read_sector(&mut self, lba: u64, sector: &mut [u8]) -> Result<(), BlockError> {
        self.inner.read_sector(lba, sector)
    }

    fn write_sector(&mut self, lba: u64, sector: &[u8]) -> Result<(), BlockError> {
        if lba == self.fail_lba && !self.failed {
            self.failed = true;
            return Err(BlockError::DeviceFault);
        }
        self.inner.write_sector(lba, sector)
    }
}

fn overwrite_rollback_self_test() -> bool {
    let mut disk = MutableFatDisk::new();
    let Ok(volume) = mount(&mut disk) else {
        return false;
    };
    if create_path_file(&mut disk, volume, "keep.txt", b"old").is_err() {
        return false;
    }
    let fail_lba = match volume.cluster_lba(4) {
        Ok(lba) => lba,
        Err(_) => return false,
    };
    let mut disk = FailingWriteDisk {
        inner: disk,
        fail_lba,
        failed: false,
    };
    if create_path_file(&mut disk, volume, "keep.txt", b"new")
        != Err(Error::Block(BlockError::DeviceFault))
    {
        return false;
    }
    let mut bytes = [0_u8; 4];
    read_path_bytes(&mut disk.inner, volume, "keep.txt", &mut bytes) == Ok(3)
        && &bytes[..3] == b"old"
        && read_fat_entry(&mut disk.inner, volume, 4) == Ok(0)
}

fn directory_extension_rollback_self_test() -> bool {
    let mut disk = MutableFatDisk::new();
    let Ok(volume) = mount(&mut disk) else {
        return false;
    };

    for index in 0..16 {
        let mut name = [0_u8; 7];
        let Some(name) = numbered_leaf(b'r', index, &mut name) else {
            return false;
        };
        if create_path_file(&mut disk, volume, name, b"").is_err() {
            return false;
        }
    }

    if next_cluster(&mut disk, volume, volume.root_cluster) != Ok(ClusterLink::End) {
        return false;
    }

    let fail_lba = match volume.cluster_lba(4) {
        Ok(lba) => lba,
        Err(_) => return false,
    };
    let mut disk = FailingWriteDisk {
        inner: disk,
        fail_lba,
        failed: false,
    };

    if create_path_file(&mut disk, volume, "boom.txt", b"new")
        != Err(Error::Block(BlockError::DeviceFault))
    {
        return false;
    }

    resolve_path(&mut disk.inner, volume, "boom.txt") == Err(Error::NotFound)
        && next_cluster(&mut disk.inner, volume, volume.root_cluster) == Ok(ClusterLink::End)
        && read_fat_entry(&mut disk.inner, volume, 3) == Ok(0)
        && read_fat_entry(&mut disk.inner, volume, 4) == Ok(0)
}

#[cfg(test)]
mod range_tests {
    #[test]
    fn unaligned_range_and_eof() {
        assert!(super::range_self_test());
    }
    #[test]
    fn existing_fat32_regressions() {
        assert!(super::self_test());
    }
}

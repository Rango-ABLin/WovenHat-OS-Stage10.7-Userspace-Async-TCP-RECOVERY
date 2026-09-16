use crate::{
    block::{BlockDevice, Error as BlockError, RamDisk, SECTOR_SIZE},
    partition::Partition,
};

const GPT_HEADER_MIN_SIZE: usize = 92;
const MAX_PARTITION_ENTRIES: usize = 128;
const MAX_PARTITION_ENTRY_SIZE: usize = 256;
const EFI_SYSTEM_GUID: [u8; 16] = [
    0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b,
];
const BASIC_DATA_GUID: [u8; 16] = [
    0xa2, 0xa0, 0xd0, 0xeb, 0xe5, 0xb9, 0x33, 0x44, 0x87, 0xc0, 0x68, 0xb6, 0xb7, 0x26, 0x99, 0xc7,
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Block(BlockError),
    MissingProtectiveMbr,
    InvalidHeader,
    InvalidChecksum,
    Unsupported,
    OutOfBounds,
}

pub fn find_fat_partition(device: &mut impl BlockDevice) -> Result<Option<Partition>, Error> {
    validate_protective_mbr(device)?;
    let mut header = [0_u8; SECTOR_SIZE];
    device.read_sector(1, &mut header).map_err(Error::Block)?;
    if &header[..8] != b"EFI PART" || read_u32(&header, 8) != 0x0001_0000 {
        return Err(Error::InvalidHeader);
    }
    let header_size = read_u32(&header, 12) as usize;
    if !(GPT_HEADER_MIN_SIZE..=SECTOR_SIZE).contains(&header_size) {
        return Err(Error::InvalidHeader);
    }
    let expected_header_crc = read_u32(&header, 16);
    header[16..20].fill(0);
    if crc32(&header[..header_size]) != expected_header_crc {
        return Err(Error::InvalidChecksum);
    }

    let current_lba = read_u64(&header, 24);
    let backup_lba = read_u64(&header, 32);
    let first_usable = read_u64(&header, 40);
    let last_usable = read_u64(&header, 48);
    let entries_lba = read_u64(&header, 72);
    let entry_count = read_u32(&header, 80) as usize;
    let entry_size = read_u32(&header, 84) as usize;
    let expected_entries_crc = read_u32(&header, 88);
    if current_lba != 1
        || backup_lba >= device.sector_count()
        || first_usable > last_usable
        || last_usable >= device.sector_count()
        || entry_count == 0
        || entry_count > MAX_PARTITION_ENTRIES
        || !(128..=MAX_PARTITION_ENTRY_SIZE).contains(&entry_size)
        || !entry_size.is_multiple_of(8)
    {
        return Err(Error::Unsupported);
    }
    let entries_bytes = entry_count
        .checked_mul(entry_size)
        .ok_or(Error::OutOfBounds)?;
    let entries_start = entries_lba
        .checked_mul(SECTOR_SIZE as u64)
        .ok_or(Error::OutOfBounds)?;
    let disk_bytes = device
        .sector_count()
        .checked_mul(SECTOR_SIZE as u64)
        .ok_or(Error::OutOfBounds)?;
    if entries_start
        .checked_add(entries_bytes as u64)
        .is_none_or(|end| end > disk_bytes)
    {
        return Err(Error::OutOfBounds);
    }
    if entries_crc(device, entries_start, entries_bytes)? != expected_entries_crc {
        return Err(Error::InvalidChecksum);
    }

    let mut selected = None;
    let mut entry = [0_u8; MAX_PARTITION_ENTRY_SIZE];
    for index in 0..entry_count {
        read_bytes(
            device,
            entries_start + (index * entry_size) as u64,
            &mut entry[..entry_size],
        )?;
        let kind = &entry[..16];
        if kind.iter().all(|byte| *byte == 0) {
            continue;
        }
        let first_lba = read_u64(&entry, 32);
        let last_lba = read_u64(&entry, 40);
        if first_lba < first_usable || last_lba > last_usable || first_lba > last_lba {
            return Err(Error::OutOfBounds);
        }
        if selected.is_none() && (kind == EFI_SYSTEM_GUID || kind == BASIC_DATA_GUID) {
            selected = Some(Partition {
                start_lba: first_lba,
                sectors: last_lba - first_lba + 1,
                kind: 0xee,
            });
        }
    }
    Ok(selected)
}

fn validate_protective_mbr(device: &mut impl BlockDevice) -> Result<(), Error> {
    let mut sector = [0_u8; SECTOR_SIZE];
    device.read_sector(0, &mut sector).map_err(Error::Block)?;
    if sector[510] != 0x55 || sector[511] != 0xaa {
        return Err(Error::MissingProtectiveMbr);
    }
    for index in 0..4 {
        let offset = 446 + index * 16;
        if sector[offset + 4] == 0xee && read_u32(&sector, offset + 8) == 1 {
            return Ok(());
        }
    }
    Err(Error::MissingProtectiveMbr)
}

fn entries_crc(device: &mut impl BlockDevice, start: u64, length: usize) -> Result<u32, Error> {
    let mut state = u32::MAX;
    let mut sector = [0_u8; SECTOR_SIZE];
    let mut consumed = 0;
    while consumed < length {
        let byte_offset = start + consumed as u64;
        let lba = byte_offset / SECTOR_SIZE as u64;
        let offset = (byte_offset % SECTOR_SIZE as u64) as usize;
        device.read_sector(lba, &mut sector).map_err(Error::Block)?;
        let count = core::cmp::min(SECTOR_SIZE - offset, length - consumed);
        state = crc32_extend(state, &sector[offset..offset + count]);
        consumed += count;
    }
    Ok(!state)
}

fn read_bytes(device: &mut impl BlockDevice, start: u64, output: &mut [u8]) -> Result<(), Error> {
    let mut sector = [0_u8; SECTOR_SIZE];
    let mut copied = 0;
    while copied < output.len() {
        let byte_offset = start + copied as u64;
        let lba = byte_offset / SECTOR_SIZE as u64;
        let offset = (byte_offset % SECTOR_SIZE as u64) as usize;
        device.read_sector(lba, &mut sector).map_err(Error::Block)?;
        let count = core::cmp::min(SECTOR_SIZE - offset, output.len() - copied);
        output[copied..copied + count].copy_from_slice(&sector[offset..offset + count]);
        copied += count;
    }
    Ok(())
}

fn crc32(bytes: &[u8]) -> u32 {
    !crc32_extend(u32::MAX, bytes)
}

fn crc32_extend(mut state: u32, bytes: &[u8]) -> u32 {
    for byte in bytes {
        state ^= u32::from(*byte);
        for _ in 0..8 {
            state = (state >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(state & 1));
        }
    }
    state
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap_or([0; 4]))
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap_or([0; 8]))
}

pub fn self_test() -> bool {
    let mut disk = RamDisk::<16>::new();
    let mut mbr = [0_u8; SECTOR_SIZE];
    mbr[446 + 4] = 0xee;
    mbr[446 + 8..446 + 12].copy_from_slice(&1_u32.to_le_bytes());
    mbr[446 + 12..446 + 16].copy_from_slice(&15_u32.to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xaa;
    if disk.write_sector(0, &mbr).is_err() {
        return false;
    }

    let mut entries = [0_u8; SECTOR_SIZE];
    entries[..16].copy_from_slice(&EFI_SYSTEM_GUID);
    entries[16] = 1;
    entries[32..40].copy_from_slice(&3_u64.to_le_bytes());
    entries[40..48].copy_from_slice(&7_u64.to_le_bytes());
    let entries_crc = crc32(&entries[..128]);
    if disk.write_sector(2, &entries).is_err() {
        return false;
    }

    let mut header = [0_u8; SECTOR_SIZE];
    header[..8].copy_from_slice(b"EFI PART");
    header[8..12].copy_from_slice(&0x0001_0000_u32.to_le_bytes());
    header[12..16].copy_from_slice(&(GPT_HEADER_MIN_SIZE as u32).to_le_bytes());
    header[24..32].copy_from_slice(&1_u64.to_le_bytes());
    header[32..40].copy_from_slice(&15_u64.to_le_bytes());
    header[40..48].copy_from_slice(&3_u64.to_le_bytes());
    header[48..56].copy_from_slice(&14_u64.to_le_bytes());
    header[72..80].copy_from_slice(&2_u64.to_le_bytes());
    header[80..84].copy_from_slice(&1_u32.to_le_bytes());
    header[84..88].copy_from_slice(&128_u32.to_le_bytes());
    header[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    let header_crc = crc32(&header[..GPT_HEADER_MIN_SIZE]);
    header[16..20].copy_from_slice(&header_crc.to_le_bytes());
    if disk.write_sector(1, &header).is_err() {
        return false;
    }
    let valid = find_fat_partition(&mut disk).is_ok_and(|partition| {
        partition.is_some_and(|partition| partition.start_lba == 3 && partition.sectors == 5)
    });
    header[20] ^= 1;
    if disk.write_sector(1, &header).is_err() {
        return false;
    }
    let header_corruption_rejected = find_fat_partition(&mut disk) == Err(Error::InvalidChecksum);
    header[20] ^= 1;
    entries[20] ^= 1;
    if disk.write_sector(1, &header).is_err() || disk.write_sector(2, &entries).is_err() {
        return false;
    }
    let entries_corruption_rejected = find_fat_partition(&mut disk) == Err(Error::InvalidChecksum);
    valid && header_corruption_rejected && entries_corruption_rejected
}

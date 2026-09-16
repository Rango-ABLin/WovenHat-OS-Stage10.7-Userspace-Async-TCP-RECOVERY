use crate::block::{BlockDevice, Error as BlockError, RamDisk, SECTOR_SIZE};

const PARTITION_TABLE_OFFSET: usize = 446;
const PARTITION_ENTRY_SIZE: usize = 16;
const PARTITION_COUNT: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Partition {
    pub start_lba: u64,
    pub sectors: u64,
    pub kind: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Block(BlockError),
    InvalidTable,
    OutOfBounds,
}

pub fn find_fat32(device: &mut impl BlockDevice) -> Result<Option<Partition>, Error> {
    let mut sector = [0_u8; SECTOR_SIZE];
    device.read_sector(0, &mut sector).map_err(Error::Block)?;
    if sector[510] != 0x55 || sector[511] != 0xaa {
        return Ok(None);
    }

    let mut result = None;
    for index in 0..PARTITION_COUNT {
        let offset = PARTITION_TABLE_OFFSET + index * PARTITION_ENTRY_SIZE;
        let boot = sector[offset];
        if !matches!(boot, 0 | 0x80) {
            return Err(Error::InvalidTable);
        }
        let kind = sector[offset + 4];
        let start_lba = u64::from(read_u32(&sector, offset + 8));
        let sectors = u64::from(read_u32(&sector, offset + 12));
        if kind == 0 && start_lba == 0 && sectors == 0 {
            continue;
        }
        let end = start_lba.checked_add(sectors).ok_or(Error::OutOfBounds)?;
        if start_lba == 0 || sectors == 0 || end > device.sector_count() {
            return Err(Error::OutOfBounds);
        }
        if result.is_none() && matches!(kind, 0x0b | 0x0c) {
            result = Some(Partition {
                start_lba,
                sectors,
                kind,
            });
        }
    }
    Ok(result)
}

pub struct PartitionDevice<'a, D> {
    device: &'a mut D,
    partition: Partition,
}

impl<'a, D: BlockDevice> PartitionDevice<'a, D> {
    pub fn new(device: &'a mut D, partition: Partition) -> Result<Self, Error> {
        let end = partition
            .start_lba
            .checked_add(partition.sectors)
            .ok_or(Error::OutOfBounds)?;
        if partition.start_lba == 0 || partition.sectors == 0 || end > device.sector_count() {
            return Err(Error::OutOfBounds);
        }
        Ok(Self { device, partition })
    }

    fn absolute_lba(&self, lba: u64) -> Result<u64, BlockError> {
        if lba >= self.partition.sectors {
            return Err(BlockError::OutOfBounds);
        }
        self.partition
            .start_lba
            .checked_add(lba)
            .ok_or(BlockError::OutOfBounds)
    }
}

impl<D: BlockDevice> BlockDevice for PartitionDevice<'_, D> {
    fn is_read_only(&self) -> bool {
        self.device.is_read_only()
    }
    fn flush(&mut self) -> Result<(), BlockError> {
        self.device.flush()
    }
    fn sector_count(&self) -> u64 {
        self.partition.sectors
    }

    fn read_sector(&mut self, lba: u64, sector: &mut [u8]) -> Result<(), BlockError> {
        self.device.read_sector(self.absolute_lba(lba)?, sector)
    }

    fn write_sector(&mut self, lba: u64, sector: &[u8]) -> Result<(), BlockError> {
        self.device.write_sector(self.absolute_lba(lba)?, sector)
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

pub fn self_test() -> bool {
    let mut disk = RamDisk::<8>::new();
    let mut mbr = [0_u8; SECTOR_SIZE];
    mbr[510] = 0x55;
    mbr[511] = 0xaa;
    mbr[PARTITION_TABLE_OFFSET] = 0x80;
    mbr[PARTITION_TABLE_OFFSET + 4] = 0x0c;
    mbr[PARTITION_TABLE_OFFSET + 8..PARTITION_TABLE_OFFSET + 12]
        .copy_from_slice(&2_u32.to_le_bytes());
    mbr[PARTITION_TABLE_OFFSET + 12..PARTITION_TABLE_OFFSET + 16]
        .copy_from_slice(&4_u32.to_le_bytes());
    if disk.write_sector(0, &mbr).is_err() {
        return false;
    }
    let Ok(Some(partition)) = find_fat32(&mut disk) else {
        return false;
    };
    let mut payload = [0_u8; SECTOR_SIZE];
    payload[..9].copy_from_slice(b"partition");
    {
        let Ok(mut view) = PartitionDevice::new(&mut disk, partition) else {
            return false;
        };
        if view.write_sector(1, &payload).is_err() || view.read_sector(4, &mut payload).is_ok() {
            return false;
        }
    }
    let mut physical = [0_u8; SECTOR_SIZE];
    disk.read_sector(3, &mut physical).is_ok()
        && physical[..9] == *b"partition"
        && partition.start_lba == 2
        && partition.sectors == 4
}

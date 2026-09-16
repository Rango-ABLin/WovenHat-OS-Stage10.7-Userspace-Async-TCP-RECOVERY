pub const SECTOR_SIZE: usize = 512;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Error {
    OutOfBounds,
    InvalidBuffer,
    ReadOnly,
    DeviceFault,
}

pub trait BlockDevice {
    fn sector_count(&self) -> u64;
    fn is_read_only(&self) -> bool {
        false
    }
    /// Drain software buffers to the device. Hardware durability is device-specific.
    fn flush(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn read_sector(&mut self, lba: u64, sector: &mut [u8]) -> Result<(), Error>;
    fn write_sector(&mut self, lba: u64, sector: &[u8]) -> Result<(), Error>;
}

impl<D: BlockDevice + ?Sized> BlockDevice for &mut D {
    fn sector_count(&self) -> u64 {
        (**self).sector_count()
    }
    fn is_read_only(&self) -> bool {
        (**self).is_read_only()
    }
    fn flush(&mut self) -> Result<(), Error> {
        (**self).flush()
    }
    fn read_sector(&mut self, lba: u64, sector: &mut [u8]) -> Result<(), Error> {
        (**self).read_sector(lba, sector)
    }
    fn write_sector(&mut self, lba: u64, sector: &[u8]) -> Result<(), Error> {
        (**self).write_sector(lba, sector)
    }
}

pub struct RamDisk<const SECTORS: usize> {
    sectors: [[u8; SECTOR_SIZE]; SECTORS],
    read_only: bool,
}

impl<const SECTORS: usize> RamDisk<SECTORS> {
    pub const fn new() -> Self {
        Self {
            sectors: [[0; SECTOR_SIZE]; SECTORS],
            read_only: false,
        }
    }

    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }
}

impl<const SECTORS: usize> BlockDevice for RamDisk<SECTORS> {
    fn is_read_only(&self) -> bool {
        self.read_only
    }
    fn sector_count(&self) -> u64 {
        SECTORS as u64
    }

    fn read_sector(&mut self, lba: u64, sector: &mut [u8]) -> Result<(), Error> {
        if sector.len() != SECTOR_SIZE {
            return Err(Error::InvalidBuffer);
        }
        let index = usize::try_from(lba).map_err(|_| Error::OutOfBounds)?;
        let source = self.sectors.get(index).ok_or(Error::OutOfBounds)?;
        sector.copy_from_slice(source);
        Ok(())
    }

    fn write_sector(&mut self, lba: u64, sector: &[u8]) -> Result<(), Error> {
        if sector.len() != SECTOR_SIZE {
            return Err(Error::InvalidBuffer);
        }
        if self.read_only {
            return Err(Error::ReadOnly);
        }
        let index = usize::try_from(lba).map_err(|_| Error::OutOfBounds)?;
        let destination = self.sectors.get_mut(index).ok_or(Error::OutOfBounds)?;
        destination.copy_from_slice(sector);
        Ok(())
    }
}

pub fn self_test() -> bool {
    let mut disk = RamDisk::<2>::new();
    let mut written = [0_u8; SECTOR_SIZE];
    written[..14].copy_from_slice(b"wovenhat-block");
    if disk.write_sector(1, &written).is_err() {
        return false;
    }

    let mut read = [0_u8; SECTOR_SIZE];
    let round_trip = disk.read_sector(1, &mut read).is_ok() && read == written;
    let bounds = disk.read_sector(2, &mut read) == Err(Error::OutOfBounds);
    let size = disk.read_sector(0, &mut read[..SECTOR_SIZE - 1]) == Err(Error::InvalidBuffer);
    disk.set_read_only(true);
    let protection = disk.write_sector(0, &written) == Err(Error::ReadOnly);
    round_trip && bounds && size && protection
}

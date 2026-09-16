//! Small write-back sector cache used between filesystems and block devices.
//!
//! This deliberately stays allocation-free so it can be used during early boot.
//! Least-recently-used entries are replaced after successful dirty writeback.
//! Callers must explicitly flush: drop cannot report I/O failures.

use crate::block::{BlockDevice, Error, SECTOR_SIZE};

#[derive(Clone, Copy)]
pub struct Stats {
    pub hits: u64,
    pub misses: u64,
    pub writebacks: u64,
    pub evictions: u64,
    pub capacity: usize,
    pub resident: usize,
    pub dirty: usize,
}

#[derive(Clone, Copy)]
struct CacheEntry {
    lba: u64,
    data: [u8; SECTOR_SIZE],
    valid: bool,
    dirty: bool,
    age: u64,
}

impl CacheEntry {
    const fn empty() -> Self {
        Self {
            lba: 0,
            data: [0; SECTOR_SIZE],
            valid: false,
            dirty: false,
            age: 0,
        }
    }
}

pub struct CachedDevice<D: BlockDevice, const N: usize = 16> {
    inner: D,
    entries: [CacheEntry; N],
    writebacks: u64,
    evictions: u64,
    hits: u64,
    misses: u64,
}

impl<D: BlockDevice, const N: usize> CachedDevice<D, N> {
    pub fn new(inner: D) -> Self {
        Self {
            inner,
            entries: [CacheEntry::empty(); N],
            writebacks: 0,
            evictions: 0,
            hits: 0,
            misses: 0,
        }
    }

    #[allow(dead_code)]
    pub fn hits(&self) -> u64 {
        self.hits
    }
    #[allow(dead_code)]
    pub fn misses(&self) -> u64 {
        self.misses
    }

    fn touch(&mut self, index: usize) {
        for entry in self.entries.iter_mut().filter(|e| e.valid) {
            entry.age = entry.age.saturating_add(1);
        }
        self.entries[index].age = 0;
    }

    fn find(&self, lba: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.valid && e.lba == lba)
    }

    fn victim(&self) -> usize {
        if let Some(index) = self.entries.iter().position(|e| !e.valid) {
            return index;
        }
        self.entries
            .iter()
            .enumerate()
            .max_by_key(|(_, e)| e.age)
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    fn write_back(&mut self, index: usize) -> Result<(), Error> {
        if self.entries[index].valid && self.entries[index].dirty {
            let lba = self.entries[index].lba;
            let data = self.entries[index].data;
            self.inner.write_sector(lba, &data)?;
            self.entries[index].dirty = false;
            self.writebacks = self.writebacks.saturating_add(1);
        }
        Ok(())
    }

    fn load(&mut self, lba: u64) -> Result<usize, Error> {
        if let Some(index) = self.find(lba) {
            self.hits = self.hits.saturating_add(1);
            self.touch(index);
            return Ok(index);
        }
        self.misses = self.misses.saturating_add(1);
        let index = self.victim();
        self.write_back(index)?;
        let mut data = [0u8; SECTOR_SIZE];
        self.inner.read_sector(lba, &mut data)?;
        if self.entries[index].valid {
            self.evictions = self.evictions.saturating_add(1);
        }
        self.entries[index] = CacheEntry {
            lba,
            data,
            valid: true,
            dirty: false,
            age: 0,
        };
        self.touch(index);
        Ok(index)
    }

    pub fn flush(&mut self) -> Result<(), Error> {
        for index in 0..N {
            self.write_back(index)?;
        }
        self.inner.flush()
    }

    pub fn stats(&self) -> Stats {
        Stats {
            hits: self.hits,
            misses: self.misses,
            writebacks: self.writebacks,
            evictions: self.evictions,
            capacity: N,
            resident: self.entries.iter().filter(|e| e.valid).count(),
            dirty: self.entries.iter().filter(|e| e.valid && e.dirty).count(),
        }
    }
}

impl<D: BlockDevice, const N: usize> BlockDevice for CachedDevice<D, N> {
    fn sector_count(&self) -> u64 {
        self.inner.sector_count()
    }
    fn is_read_only(&self) -> bool {
        self.inner.is_read_only()
    }
    fn flush(&mut self) -> Result<(), Error> {
        CachedDevice::flush(self)
    }

    fn read_sector(&mut self, lba: u64, sector: &mut [u8]) -> Result<(), Error> {
        if sector.len() != SECTOR_SIZE {
            return Err(Error::InvalidBuffer);
        }
        if lba >= self.sector_count() {
            return Err(Error::OutOfBounds);
        }
        if N == 0 {
            return self.inner.read_sector(lba, sector);
        }
        let index = self.load(lba)?;
        sector.copy_from_slice(&self.entries[index].data);
        Ok(())
    }

    fn write_sector(&mut self, lba: u64, sector: &[u8]) -> Result<(), Error> {
        if sector.len() != SECTOR_SIZE {
            return Err(Error::InvalidBuffer);
        }
        if lba >= self.sector_count() {
            return Err(Error::OutOfBounds);
        }
        if self.is_read_only() {
            return Err(Error::ReadOnly);
        }
        if N == 0 {
            return self.inner.write_sector(lba, sector);
        }
        let index = if let Some(index) = self.find(lba) {
            self.hits = self.hits.saturating_add(1);
            index
        } else {
            self.misses = self.misses.saturating_add(1);
            let index = self.victim();
            self.write_back(index)?;
            if self.entries[index].valid {
                self.evictions = self.evictions.saturating_add(1);
            }
            self.entries[index] = CacheEntry {
                lba,
                data: [0; SECTOR_SIZE],
                valid: true,
                dirty: false,
                age: 0,
            };
            index
        };
        self.entries[index].data.copy_from_slice(sector);
        self.entries[index].dirty = true;
        self.touch(index);
        Ok(())
    }
}

// Deterministic device probe shared by host and QEMU self-tests.
struct Probe {
    disk: crate::block::RamDisk<4>,
    reads: usize,
    writes: usize,
    flushes: usize,
    fail_read: bool,
    fail_write: bool,
    fail_flush: bool,
}

impl Probe {
    fn new() -> Self {
        Self {
            disk: crate::block::RamDisk::new(),
            reads: 0,
            writes: 0,
            flushes: 0,
            fail_read: false,
            fail_write: false,
            fail_flush: false,
        }
    }
}

impl BlockDevice for Probe {
    fn sector_count(&self) -> u64 {
        self.disk.sector_count()
    }
    fn is_read_only(&self) -> bool {
        self.disk.is_read_only()
    }
    fn read_sector(&mut self, lba: u64, out: &mut [u8]) -> Result<(), Error> {
        self.reads += 1;
        if self.fail_read {
            return Err(Error::DeviceFault);
        }
        self.disk.read_sector(lba, out)
    }
    fn write_sector(&mut self, lba: u64, data: &[u8]) -> Result<(), Error> {
        self.writes += 1;
        if self.fail_write {
            return Err(Error::DeviceFault);
        }
        self.disk.write_sector(lba, data)
    }
    fn flush(&mut self) -> Result<(), Error> {
        self.flushes += 1;
        if self.fail_flush {
            Err(Error::DeviceFault)
        } else {
            Ok(())
        }
    }
}

fn reuse_and_eviction_test() -> bool {
    let mut cache = CachedDevice::<_, 2>::new(Probe::new());
    let mut out = [0; SECTOR_SIZE];
    let a = [1; SECTOR_SIZE];
    let b = [2; SECTOR_SIZE];
    if cache.read_sector(0, &mut out).is_err()
        || cache.read_sector(0, &mut out).is_err()
        || cache.inner.reads != 1
        || cache.write_sector(1, &a).is_err()
        || cache.write_sector(1, &b).is_err()
        || cache.inner.reads != 1
        || cache.inner.writes != 0
        || cache.read_sector(1, &mut out).is_err()
        || out != b
        || cache.stats().dirty != 1
    {
        return false;
    }
    // Touch 0 last: dirty sector 1 must be the LRU victim.
    if cache.read_sector(0, &mut out).is_err()
        || cache.read_sector(2, &mut out).is_err()
        || cache.inner.writes != 1
        || cache.stats().writebacks != 1
        || cache.stats().evictions != 1
        || cache.stats().resident != 2
        || cache.inner.disk.read_sector(1, &mut out).is_err()
        || out != b
    {
        return false;
    }
    // Repeated flush retains clean cached data and performs no extra writes.
    cache.flush().is_ok()
        && cache.flush().is_ok()
        && cache.inner.writes == 1
        && cache.inner.flushes == 2
        && cache.stats().dirty == 0
        && cache.read_sector(0, &mut out).is_ok()
        && cache.inner.reads == 2
        && cache.hits() == 5
        && cache.misses() == 3
}

fn failure_recovery_test() -> bool {
    let mut cache = CachedDevice::<_, 1>::new(Probe::new());
    let data = [7; SECTOR_SIZE];
    let mut out = [0; SECTOR_SIZE];
    if cache.write_sector(0, &data).is_err() {
        return false;
    }
    cache.inner.fail_write = true;
    if cache.flush() != Err(Error::DeviceFault)
        || cache.read_sector(1, &mut out) != Err(Error::DeviceFault)
        || cache.write_sector(1, &[9; SECTOR_SIZE]) != Err(Error::DeviceFault)
        || cache.stats().dirty != 1
        || cache.stats().evictions != 0
        || cache.read_sector(0, &mut out).is_err()
        || out != data
    {
        return false;
    }
    cache.inner.fail_write = false;
    if cache.flush().is_err()
        || cache.stats().dirty != 0
        || cache.inner.disk.read_sector(0, &mut out).is_err()
        || out != data
    {
        return false;
    }
    cache.inner.fail_read = true;
    out.fill(3);
    if cache.read_sector(1, &mut out) != Err(Error::DeviceFault)
        || out != [3; SECTOR_SIZE]
        || cache.read_sector(0, &mut out).is_err()
        || out != data
    {
        return false;
    }
    cache.inner.fail_read = false;
    cache.inner.fail_flush = true;
    if cache.flush() != Err(Error::DeviceFault) {
        return false;
    }
    cache.inner.fail_flush = false;
    cache.flush().is_ok() && cache.read_sector(1, &mut out).is_ok() && out == [0; SECTOR_SIZE]
}

fn validation_and_bypass_test() -> bool {
    let mut cache = CachedDevice::<_, 1>::new(Probe::new());
    let mut out = [0; SECTOR_SIZE];
    if cache.read_sector(4, &mut out) != Err(Error::OutOfBounds)
        || cache.write_sector(4, &out) != Err(Error::OutOfBounds)
        || cache.read_sector(0, &mut out[..1]) != Err(Error::InvalidBuffer)
        || cache.write_sector(0, &out[..1]) != Err(Error::InvalidBuffer)
        || cache.inner.reads != 0
        || cache.inner.writes != 0
    {
        return false;
    }
    cache.inner.disk.set_read_only(true);
    if cache.write_sector(0, &out) != Err(Error::ReadOnly) || cache.stats().dirty != 0 {
        return false;
    }
    let mut bypass = CachedDevice::<_, 0>::new(Probe::new());
    let data = [8; SECTOR_SIZE];
    bypass.write_sector(2, &data).is_ok()
        && bypass.read_sector(2, &mut out).is_ok()
        && out == data
        && bypass.inner.writes == 1
        && bypass.inner.reads == 1
        && bypass.stats().resident == 0
        && bypass.flush().is_ok()
        && bypass.inner.flushes == 1
}

pub fn self_test() -> bool {
    reuse_and_eviction_test() && failure_recovery_test() && validation_and_bypass_test()
}

#[cfg(test)]
mod tests {
    #[test]
    fn reuses_reads_coalesces_writes_and_evicts_lru() {
        assert!(super::reuse_and_eviction_test());
    }
    #[test]
    fn retains_data_on_io_failure_and_retries() {
        assert!(super::failure_recovery_test());
    }
    #[test]
    fn validates_and_supports_zero_capacity() {
        assert!(super::validation_and_bypass_test());
    }
}

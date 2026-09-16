use crate::{
    block::{BlockDevice, Error as BlockError, RamDisk, SECTOR_SIZE},
    config::MAX_SWAP_SLOTS,
};

use spin::Mutex;

pub const PAGE_SIZE: usize = 4096;
const SECTORS_PER_PAGE: usize = PAGE_SIZE / SECTOR_SIZE;
const _: () = assert!(PAGE_SIZE.is_multiple_of(SECTOR_SIZE));

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Handle {
    index: u16,
    generation: u16,
}

impl Handle {
    fn index(self) -> usize {
        self.index as usize
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Backing {
    Ram,
    Disk,
}

#[derive(Clone, Copy)]
struct Slot {
    data: [u8; PAGE_SIZE],
    refcount: u16,
    generation: u16,
    occupied: bool,
    backing: Backing,
}

impl Slot {
    const fn empty() -> Self {
        Self {
            data: [0; PAGE_SIZE],
            refcount: 0,
            generation: 0,
            occupied: false,
            backing: Backing::Ram,
        }
    }

    fn clear(&mut self) {
        let generation = self.generation;
        *self = Self::empty();
        self.generation = generation;
    }
}

#[derive(Clone, Copy)]
struct DiskBacking {
    start_lba: u64,
    slots: usize,
}

#[derive(Clone, Copy)]
struct State {
    slots: [Slot; MAX_SWAP_SLOTS],
    disk: Option<DiskBacking>,
}

impl State {
    const fn new() -> Self {
        Self {
            slots: [const { Slot::empty() }; MAX_SWAP_SLOTS],
            disk: None,
        }
    }

    fn configure_disk(&mut self, start_lba: u64, sectors: u64) -> Option<usize> {
        if self.used() != 0 || sectors < SECTORS_PER_PAGE as u64 {
            return None;
        }
        let slots = core::cmp::min(
            MAX_SWAP_SLOTS,
            usize::try_from(sectors / SECTORS_PER_PAGE as u64).unwrap_or(MAX_SWAP_SLOTS),
        );
        if slots == 0 {
            return None;
        }
        self.disk = Some(DiskBacking { start_lba, slots });
        Some(slots)
    }

    fn used(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.occupied && slot.refcount > 0)
            .count()
    }

    fn free_slot(&self) -> Option<usize> {
        self.slots.iter().position(|slot| !slot.occupied)
    }

    fn disk_lba(&self, index: usize) -> Option<u64> {
        let disk = self.disk?;
        if index >= disk.slots {
            return None;
        }
        disk.start_lba
            .checked_add((index as u64).checked_mul(SECTORS_PER_PAGE as u64)?)
    }

    fn next_generation(&self, index: usize) -> u16 {
        let mut generation = self.slots[index].generation.wrapping_add(1);
        if generation == 0 {
            generation = 1;
        }
        generation
    }

    fn occupy(
        &mut self,
        index: usize,
        backing: Backing,
        bytes: Option<&[u8; PAGE_SIZE]>,
    ) -> Handle {
        let generation = self.next_generation(index);
        self.slots[index].clear();
        if let Some(bytes) = bytes {
            self.slots[index].data.copy_from_slice(bytes);
        }
        self.slots[index].generation = generation;
        self.slots[index].refcount = 1;
        self.slots[index].occupied = true;
        self.slots[index].backing = backing;
        Handle {
            index: index as u16,
            generation,
        }
    }

    fn reserve_disk(&mut self) -> Option<(usize, u16, u64)> {
        let index = self.free_slot()?;
        let lba = self.disk_lba(index)?;
        let generation = self.next_generation(index);
        self.slots[index].clear();
        self.slots[index].generation = generation;
        self.slots[index].occupied = true;
        self.slots[index].refcount = 0;
        self.slots[index].backing = Backing::Disk;
        Some((index, generation, lba))
    }

    fn publish_reserved_disk(&mut self, index: usize, generation: u16) -> Option<Handle> {
        let slot = self.slots.get_mut(index)?;
        if !slot.occupied || slot.refcount != 0 || slot.generation != generation {
            return None;
        }
        slot.refcount = 1;
        slot.backing = Backing::Disk;
        Some(Handle {
            index: index as u16,
            generation,
        })
    }

    fn cancel_reserved(&mut self, index: usize, generation: u16) {
        let Some(slot) = self.slots.get_mut(index) else {
            return;
        };
        if slot.occupied && slot.refcount == 0 && slot.generation == generation {
            slot.clear();
        }
    }

    fn allocate_ram(&mut self, bytes: &[u8; PAGE_SIZE]) -> Option<Handle> {
        let index = self.free_slot()?;
        Some(self.occupy(index, Backing::Ram, Some(bytes)))
    }

    fn allocate_disk(
        &mut self,
        device: &mut impl BlockDevice,
        bytes: &[u8; PAGE_SIZE],
    ) -> Option<Handle> {
        let (index, generation, lba) = self.reserve_disk()?;
        if write_page_to_device(device, lba, bytes).is_err() || device.flush().is_err() {
            self.cancel_reserved(index, generation);
            return None;
        }
        self.publish_reserved_disk(index, generation)
    }

    fn get(&self, handle: Handle) -> Option<&Slot> {
        let slot = self.slots.get(handle.index())?;
        (slot.occupied && slot.refcount > 0 && slot.generation == handle.generation).then_some(slot)
    }

    fn get_mut(&mut self, handle: Handle) -> Option<&mut Slot> {
        let slot = self.slots.get_mut(handle.index())?;
        (slot.occupied && slot.refcount > 0 && slot.generation == handle.generation).then_some(slot)
    }

    fn read_ram(&self, handle: Handle, bytes: &mut [u8; PAGE_SIZE]) -> bool {
        let Some(slot) = self.get(handle) else {
            return false;
        };
        if slot.backing != Backing::Ram {
            return false;
        }
        bytes.copy_from_slice(&slot.data);
        true
    }

    fn disk_read_lba(&self, handle: Handle) -> Option<u64> {
        let slot = self.get(handle)?;
        if slot.backing != Backing::Disk {
            return None;
        }
        self.disk_lba(handle.index())
    }

    fn read_disk(
        &self,
        device: &mut impl BlockDevice,
        handle: Handle,
        bytes: &mut [u8; PAGE_SIZE],
    ) -> bool {
        let Some(lba) = self.disk_read_lba(handle) else {
            return false;
        };
        read_page_from_device(device, lba, bytes).is_ok()
    }

    fn share(&mut self, handle: Handle) -> bool {
        let Some(slot) = self.get_mut(handle) else {
            return false;
        };
        let Some(refcount) = slot.refcount.checked_add(1) else {
            return false;
        };
        slot.refcount = refcount;
        true
    }

    fn release(&mut self, handle: Handle) -> bool {
        let Some(slot) = self.get_mut(handle) else {
            return false;
        };
        slot.refcount = slot.refcount.saturating_sub(1);
        if slot.refcount == 0 {
            slot.clear();
        }
        true
    }

    fn stats(&self) -> Stats {
        let mut stats = Stats {
            used: 0,
            ram_used: 0,
            disk_used: 0,
            disk_slots: self.disk.map(|disk| disk.slots).unwrap_or(0),
        };
        for slot in self
            .slots
            .iter()
            .filter(|slot| slot.occupied && slot.refcount > 0)
        {
            stats.used += 1;
            match slot.backing {
                Backing::Ram => stats.ram_used += 1,
                Backing::Disk => stats.disk_used += 1,
            }
        }
        stats
    }
}

static SWAP: Mutex<State> = Mutex::new(State::new());

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub used: usize,
    pub ram_used: usize,
    pub disk_used: usize,
    pub disk_slots: usize,
}

pub fn configure_ata_backing(start_lba: u64, sectors: u64) -> Option<usize> {
    SWAP.lock().configure_disk(start_lba, sectors)
}

pub fn allocate_page(bytes: &[u8; PAGE_SIZE]) -> Option<Handle> {
    let reservation = SWAP.lock().reserve_disk();
    if let Some((index, generation, lba)) = reservation {
        let wrote = if crate::block_io::primary_ata_present() {
            let mut disk = crate::block_io::primary_ata();
            write_page_to_device(&mut disk, lba, bytes).is_ok() && disk.flush().is_ok()
        } else {
            false
        };
        let mut state = SWAP.lock();
        if wrote {
            if let Some(handle) = state.publish_reserved_disk(index, generation) {
                return Some(handle);
            }
        }
        state.cancel_reserved(index, generation);
    }
    SWAP.lock().allocate_ram(bytes)
}

pub fn read_page(handle: Handle, bytes: &mut [u8; PAGE_SIZE]) -> bool {
    let disk_lba = {
        let state = SWAP.lock();
        if state.read_ram(handle, bytes) {
            return true;
        }
        state.disk_read_lba(handle)
    };
    let Some(lba) = disk_lba else {
        return false;
    };
    if !crate::block_io::primary_ata_present() {
        return false;
    }
    let mut disk = crate::block_io::primary_ata();
    read_page_from_device(&mut disk, lba, bytes).is_ok()
}

pub fn share_page(handle: Handle) -> bool {
    SWAP.lock().share(handle)
}

pub fn release_page(handle: Handle) -> bool {
    SWAP.lock().release(handle)
}

pub fn stats() -> Stats {
    SWAP.lock().stats()
}

fn write_page_to_device(
    device: &mut impl BlockDevice,
    start_lba: u64,
    bytes: &[u8; PAGE_SIZE],
) -> Result<(), BlockError> {
    for sector in 0..SECTORS_PER_PAGE {
        let start = sector * SECTOR_SIZE;
        let lba = start_lba
            .checked_add(sector as u64)
            .ok_or(BlockError::OutOfBounds)?;
        device.write_sector(lba, &bytes[start..start + SECTOR_SIZE])?;
    }
    Ok(())
}

fn read_page_from_device(
    device: &mut impl BlockDevice,
    start_lba: u64,
    bytes: &mut [u8; PAGE_SIZE],
) -> Result<(), BlockError> {
    for sector in 0..SECTORS_PER_PAGE {
        let start = sector * SECTOR_SIZE;
        let lba = start_lba
            .checked_add(sector as u64)
            .ok_or(BlockError::OutOfBounds)?;
        device.read_sector(lba, &mut bytes[start..start + SECTOR_SIZE])?;
    }
    Ok(())
}

pub fn self_test() -> bool {
    let mut first = [0_u8; PAGE_SIZE];
    let mut second = [0_u8; PAGE_SIZE];
    for index in 0..PAGE_SIZE {
        first[index] = (index as u8).wrapping_mul(3).wrapping_add(7);
        second[index] = 255_u8.wrapping_sub(index as u8);
    }

    let mut disk = RamDisk::<32>::new();
    let mut state = State::new();
    if state.configure_disk(8, (SECTORS_PER_PAGE * 2) as u64) != Some(2) {
        return false;
    }

    let Some(first_handle) = state.allocate_disk(&mut disk, &first) else {
        return false;
    };
    let Some(second_handle) = state.allocate_disk(&mut disk, &second) else {
        return false;
    };
    if state.allocate_disk(&mut disk, &first).is_some() {
        return false;
    }

    let mut out = [0_u8; PAGE_SIZE];
    if !state.read_disk(&mut disk, first_handle, &mut out) || out != first {
        return false;
    }
    if !state.share(first_handle) || !state.release(first_handle) {
        return false;
    }
    out.fill(0);
    if !state.read_disk(&mut disk, first_handle, &mut out) || out != first {
        return false;
    }
    if !state.release(first_handle) || state.read_disk(&mut disk, first_handle, &mut out) {
        return false;
    }
    let Some(reused) = state.allocate_disk(&mut disk, &second) else {
        return false;
    };
    if reused == first_handle || state.read_disk(&mut disk, first_handle, &mut out) {
        return false;
    }

    if !state.release(second_handle) || !state.release(reused) || state.used() != 0 {
        return false;
    }

    let mut ram_state = State::new();
    let Some(ram_handle) = ram_state.allocate_ram(&first) else {
        return false;
    };
    out.fill(0);
    if !ram_state.read_ram(ram_handle, &mut out) || out != first || !ram_state.release(ram_handle) {
        return false;
    }

    disk.set_read_only(true);
    state.allocate_disk(&mut disk, &first).is_none() && state.used() == 0
}

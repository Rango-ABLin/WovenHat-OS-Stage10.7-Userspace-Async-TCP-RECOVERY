use bootloader_api::info::{MemoryRegion, MemoryRegionKind};
#[cfg(feature = "qemu-test")]
use alloc::vec::Vec;
use x86_64::{
    structures::paging::{FrameAllocator, PageSize, PhysFrame, Size4KiB},
    PhysAddr,
};

use crate::irq_lock::{IrqMutex, IrqMutexGuard};

const FRAME_SIZE: u64 = Size4KiB::SIZE;
const MAX_USABLE_REGIONS: usize = 128;
// Hot cache for returns; the per-region bitmap keeps returns beyond this
// capacity reusable without imposing a total deallocation limit.
const MAX_RECLAIMED_FRAMES: usize = 4096;

// Lock order: PAGING (10) -> COW_TABLE (30) -> ALLOCATOR (40).
static ALLOCATOR: IrqMutex<PhysicalFrameAllocator> =
    IrqMutex::with_rank(PhysicalFrameAllocator::empty(), 40);

#[derive(Clone, Copy)]
struct FrameRange {
    start: u64,
    next: u64,
    end: u64,
    domain: u32,
    bitmap: u64,
    overflow_hint: u64,
}

impl FrameRange {
    const fn empty() -> Self {
        Self {
            start: 0,
            next: 0,
            end: 0,
            domain: 0,
            bitmap: 0,
            overflow_hint: u64::MAX,
        }
    }
}

pub struct Stats {
    pub usable_regions: usize,
    pub total_frames: u64,
    pub allocated_frames: u64,
    pub remaining_frames: u64,
    pub numa_domains: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InitError {
    AlreadyInitialized,
    MissingPhysicalMemoryMapping,
    TooManyUsableRegions,
    AddressOverflow,
    NoUsableFrames,
}

pub(crate) struct PhysicalFrameAllocator {
    ranges: [FrameRange; MAX_USABLE_REGIONS],
    range_count: usize,
    current_range: usize,
    total_frames: u64,
    allocated_frames: u64,
    reclaimed_frames: [u64; MAX_RECLAIMED_FRAMES],
    reclaimed_domains: [u32; MAX_RECLAIMED_FRAMES],
    reclaimed_count: usize,
    reclaimed_untracked: u64,
    numa_enabled: bool,
    initialized: bool,
}

impl PhysicalFrameAllocator {
    const fn empty() -> Self {
        Self {
            ranges: [FrameRange::empty(); MAX_USABLE_REGIONS],
            range_count: 0,
            current_range: 0,
            total_frames: 0,
            allocated_frames: 0,
            reclaimed_frames: [0; MAX_RECLAIMED_FRAMES],
            reclaimed_domains: [0; MAX_RECLAIMED_FRAMES],
            reclaimed_count: 0,
            reclaimed_untracked: 0,
            numa_enabled: false,
            initialized: false,
        }
    }

    fn initialize(
        &mut self,
        regions: &[MemoryRegion],
        affinities: &[crate::hal::acpi::MemoryAffinity],
        physical_memory_offset: u64,
    ) -> Result<(), InitError> {
        if self.initialized {
            return Err(InitError::AlreadyInitialized);
        }

        for region in regions {
            if region.kind != MemoryRegionKind::Usable {
                continue;
            }

            let start = align_up(region.start.max(0x10_0000), FRAME_SIZE).ok_or(InitError::AddressOverflow)?;
            let end = align_down(region.end, FRAME_SIZE);
            if start >= end {
                continue;
            }

            if self.range_count == self.ranges.len() {
                return Err(InitError::TooManyUsableRegions);
            }

            // Keep one allocation bit per usable frame in pages carved from
            // this same physical range. The bitmap is reachable through the
            // bootloader's direct map and never enters the free pool.
            let frames = (end - start) / FRAME_SIZE;
            let bitmap_bytes = frames.div_ceil(8);
            let bitmap_pages = bitmap_bytes.div_ceil(FRAME_SIZE);
            let bitmap_size = bitmap_pages
                .checked_mul(FRAME_SIZE)
                .ok_or(InitError::AddressOverflow)?;
            let alloc_start = start.checked_add(bitmap_size).ok_or(InitError::AddressOverflow)?;
            if alloc_start >= end {
                continue;
            }
            let bitmap_address = physical_memory_offset
                .checked_add(start)
                .ok_or(InitError::AddressOverflow)?;
            let bitmap_size = usize::try_from(bitmap_size).map_err(|_| InitError::AddressOverflow)?;
            // SAFETY: Bootloader direct-maps each usable physical range;
            // bitmap pages have been removed from the allocatable portion.
            unsafe { core::ptr::write_bytes(bitmap_address as *mut u8, 0, bitmap_size) };
            let frames = (end - alloc_start) / FRAME_SIZE;
            self.total_frames = self
                .total_frames
                .checked_add(frames)
                .ok_or(InitError::AddressOverflow)?;
            self.ranges[self.range_count] = FrameRange {
                start: alloc_start,
                next: alloc_start,
                end,
                domain: memory_domain(start, end, affinities),
                bitmap: bitmap_address,
                overflow_hint: u64::MAX,
            };
            self.range_count += 1;
        }

        if self.total_frames == 0 {
            return Err(InitError::NoUsableFrames);
        }

        self.initialized = true;
        self.numa_enabled = !affinities.is_empty();
        Ok(())
    }

    fn stats(&self) -> Stats {
        Stats {
            usable_regions: self.range_count,
            total_frames: self.total_frames,
            allocated_frames: self.allocated_frames,
            remaining_frames: self.total_frames.saturating_sub(self.allocated_frames),
            numa_domains: count_domains(&self.ranges[..self.range_count]),
        }
    }

    fn contains(&self, frame: PhysFrame<Size4KiB>) -> bool {
        let address = frame.start_address().as_u64();
        self.ranges[..self.range_count]
            .iter()
            .any(|range| address >= range.start && address < range.end)
    }

    pub(crate) fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) -> bool {
        let address = frame.start_address().as_u64();
        let Some(range) = self.ranges[..self.range_count]
            .iter_mut()
            .find(|range| address >= range.start && address < range.next)
        else {
            return false;
        };
        if !bitmap_get(range, address) || self.allocated_frames == 0 {
            return false;
        }
        bitmap_set(range, address, false);
        if self.reclaimed_count < self.reclaimed_frames.len() {
            self.reclaimed_frames[self.reclaimed_count] = address;
            self.reclaimed_domains[self.reclaimed_count] = range.domain;
            self.reclaimed_count += 1;
        } else {
            self.reclaimed_untracked += 1;
            range.overflow_hint = range.overflow_hint.min(address);
        }
        self.allocated_frames -= 1;
        true
    }
}

impl PhysicalFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let preferred = self
            .numa_enabled
            .then(|| crate::smp::cpu_domain(crate::smp::cpu_index()));
        self.allocate_frame_for_domain(preferred)
    }

    /// Reserve a physically contiguous run for a device DMA arena. Reclaimed
    /// frames are intentionally excluded: they are individually reusable but
    /// do not carry a contiguity guarantee. A run is carved from one original
    /// usable range, preserving the allocator's bounded NUMA preference.
    fn allocate_contiguous_frames(&mut self, count: usize) -> Option<PhysFrame<Size4KiB>> {
        if !self.initialized || count == 0 {
            return None;
        }
        let bytes = FRAME_SIZE.checked_mul(count as u64)?;
        let preferred = self
            .numa_enabled
            .then(|| crate::smp::cpu_domain(crate::smp::cpu_index()));
        for pass in 0..2 {
            for offset in 0..self.range_count {
                let index = (self.current_range + offset) % self.range_count;
                if pass == 0
                    && preferred.is_some_and(|domain| self.ranges[index].domain != domain)
                {
                    continue;
                }
                let range = &mut self.ranges[index];
                let available = range.end.saturating_sub(range.next);
                if available < bytes {
                    continue;
                }
                let address = range.next;
                range.next = range.next.checked_add(bytes)?;
                for frame in 0..count {
                    bitmap_set(range, address + frame as u64 * FRAME_SIZE, true);
                }
                self.current_range = if range.next >= range.end {
                    (index + 1) % self.range_count
                } else {
                    index
                };
                self.allocated_frames = self
                    .allocated_frames
                    .checked_add(count as u64)?;
                return PhysFrame::from_start_address(PhysAddr::new(address)).ok();
            }
        }
        None
    }

    fn allocate_frame_for_domain(
        &mut self,
        preferred: Option<u32>,
    ) -> Option<PhysFrame<Size4KiB>> {
        if !self.initialized {
            return None;
        }

        if self.reclaimed_count != 0 {
            let index = preferred
                .and_then(|domain| {
                    (0..self.reclaimed_count)
                        .rev()
                        .find(|index| self.reclaimed_domains[*index] == domain)
                })
                .unwrap_or(self.reclaimed_count - 1);
            let last = self.reclaimed_count - 1;
            let address = self.reclaimed_frames[index];
            if index != last {
                self.reclaimed_frames[index] = self.reclaimed_frames[last];
                self.reclaimed_domains[index] = self.reclaimed_domains[last];
            }
            self.reclaimed_count = last;
            let range = self.ranges[..self.range_count]
                .iter()
                .find(|range| address >= range.start && address < range.end)?;
            bitmap_set(range, address, true);
            self.allocated_frames += 1;
            return PhysFrame::from_start_address(PhysAddr::new(address)).ok();
        }

        if self.reclaimed_untracked != 0 {
            for pass in 0..2 {
                for offset in 0..self.range_count {
                    let index = (self.current_range + offset) % self.range_count;
                    let range = &mut self.ranges[index];
                    if pass == 0
                        && preferred.is_some_and(|domain| range.domain != domain)
                    {
                        continue;
                    }
                    let mut address = range.overflow_hint;
                    while address < range.next {
                        if !bitmap_get(range, address) {
                            bitmap_set(range, address, true);
                            range.overflow_hint = address + FRAME_SIZE;
                            self.reclaimed_untracked -= 1;
                            self.allocated_frames += 1;
                            return PhysFrame::from_start_address(PhysAddr::new(address)).ok();
                        }
                        address += FRAME_SIZE;
                    }
                    range.overflow_hint = u64::MAX;
                }
            }
        }

        for pass in 0..2 {
            for offset in 0..self.range_count {
                let index = (self.current_range + offset) % self.range_count;
                if pass == 0
                    && preferred.is_some_and(|domain| self.ranges[index].domain != domain)
                {
                    continue;
                }
                let range = &mut self.ranges[index];
                if range.next < range.end {
                    let address = range.next;
                    range.next += FRAME_SIZE;
                    bitmap_set(range, address, true);
                    self.current_range = if range.next >= range.end {
                        (index + 1) % self.range_count
                    } else {
                        index
                    };
                    self.allocated_frames += 1;
                    return PhysFrame::from_start_address(PhysAddr::new(address)).ok();
                }
            }
        }

        None
    }
}

fn bitmap_get(range: &FrameRange, address: u64) -> bool {
    let index = ((address - range.start) / FRAME_SIZE) as usize;
    // SAFETY: The bitmap is reserved direct-mapped RAM, and the allocator
    // mutex serializes every access to this range's allocation bits.
    let byte = unsafe { (range.bitmap as *const u8).add(index / 8).read() };
    byte & (1 << (index % 8)) != 0
}

fn bitmap_set(range: &FrameRange, address: u64, allocated: bool) {
    let index = ((address - range.start) / FRAME_SIZE) as usize;
    // SAFETY: See bitmap_get. The bitmap has one bit for every frame in the
    // allocatable subrange and remains permanently outside that subrange.
    let pointer = unsafe { (range.bitmap as *mut u8).add(index / 8) };
    let previous = unsafe { pointer.read() };
    let mask = 1 << (index % 8);
    unsafe { pointer.write(if allocated { previous | mask } else { previous & !mask }) };
}

// SAFETY: The allocator returns each fully usable 4 KiB frame at most once.
// Its only instance is protected by `ALLOCATOR`, so cursors cannot race or be
// cloned/reset while frames are live.
unsafe impl FrameAllocator<Size4KiB> for PhysicalFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        PhysicalFrameAllocator::allocate_frame(self)
    }
}

pub fn init(regions: &[MemoryRegion], physical_memory_offset: u64) -> Result<(), InitError> {
    ALLOCATOR.lock().initialize(regions, &[], physical_memory_offset)
}

pub fn init_with_topology(
    regions: &[MemoryRegion],
    affinities: &[crate::hal::acpi::MemoryAffinity],
    physical_memory_offset: u64,
) -> Result<(), InitError> {
    ALLOCATOR.lock().initialize(regions, affinities, physical_memory_offset)
}

pub fn allocate_frame() -> Option<PhysFrame<Size4KiB>> {
    ALLOCATOR.lock().allocate_frame()
}

pub(crate) fn allocate_contiguous_frames(count: usize) -> Option<PhysFrame<Size4KiB>> {
    ALLOCATOR.lock().allocate_contiguous_frames(count)
}

pub fn deallocate_frame(frame: PhysFrame<Size4KiB>) -> bool {
    ALLOCATOR.lock().deallocate_frame(frame)
}

pub(crate) fn allocator() -> IrqMutexGuard<'static, PhysicalFrameAllocator> {
    ALLOCATOR.lock()
}

pub fn stats() -> Stats {
    ALLOCATOR.lock().stats()
}

pub fn self_test() -> bool {
    let first = allocate_frame();
    let second = allocate_frame();
    let third = allocate_frame();

    let (Some(first), Some(second), Some(third)) = (first, second, third) else {
        return false;
    };

    let allocator = ALLOCATOR.lock();
    let affinity = [crate::hal::acpi::MemoryAffinity {
        domain: 7,
        base: 0x20_0000,
        length: 0x10_0000,
    }];
    let topology_mapping_valid = memory_domain(0x20_0000, 0x30_0000, &affinity) == 7
        && memory_domain(0x10_0000, 0x20_0000, &affinity) == 0;
    first != second
        && second != third
        && first != third
        && allocator.contains(first)
        && allocator.contains(second)
        && allocator.contains(third)
        && topology_mapping_valid
}

/// Exercise returns beyond the small hot cache and verify that every frame
/// remains reachable through the per-region allocation bitmap.
#[cfg(feature = "qemu-test")]
pub fn reclaimed_overflow_self_test() -> bool {
    const COUNT: usize = MAX_RECLAIMED_FRAMES + 256;
    if stats().remaining_frames < COUNT as u64 + 32 {
        return false;
    }
    let before = stats().allocated_frames;
    let mut first = Vec::with_capacity(COUNT);
    for _ in 0..COUNT {
        let Some(frame) = allocate_frame() else {
            for frame in first {
                let _ = deallocate_frame(frame);
            }
            return false;
        };
        first.push(frame);
    }
    let mut expected: Vec<u64> = first
        .iter()
        .map(|frame| frame.start_address().as_u64())
        .collect();
    expected.sort_unstable();
    for frame in first {
        if !deallocate_frame(frame) {
            return false;
        }
    }
    let overflow_reached = ALLOCATOR.lock().reclaimed_untracked != 0;
    if !overflow_reached || stats().allocated_frames != before {
        return false;
    }
    let mut second = Vec::with_capacity(COUNT);
    for _ in 0..COUNT {
        let Some(frame) = allocate_frame() else {
            return false;
        };
        second.push(frame);
    }
    let mut actual: Vec<u64> = second
        .iter()
        .map(|frame| frame.start_address().as_u64())
        .collect();
    actual.sort_unstable();
    let same_frames = expected == actual;
    let duplicate_rejected = second
        .first()
        .copied()
        .is_some_and(|frame| deallocate_frame(frame) && !deallocate_frame(frame));
    for frame in second.into_iter().skip(1) {
        if !deallocate_frame(frame) {
            return false;
        }
    }
    same_frames && duplicate_rejected && stats().allocated_frames == before
}

fn align_up(address: u64, alignment: u64) -> Option<u64> {
    address
        .checked_add(alignment - 1)
        .map(|value| align_down(value, alignment))
}

fn memory_domain(
    start: u64,
    end: u64,
    affinities: &[crate::hal::acpi::MemoryAffinity],
) -> u32 {
    affinities
        .iter()
        .find(|affinity| {
            affinity.base <= start
                && affinity
                    .base
                    .checked_add(affinity.length)
                    .is_some_and(|affinity_end| end <= affinity_end)
        })
        .map_or(0, |affinity| affinity.domain)
}

fn count_domains(ranges: &[FrameRange]) -> usize {
    let mut seen = [u32::MAX; MAX_USABLE_REGIONS];
    let mut count = 0;
    for range in ranges {
        if !seen[..count].contains(&range.domain) {
            seen[count] = range.domain;
            count += 1;
        }
    }
    count
}

const fn align_down(address: u64, alignment: u64) -> u64 {
    address & !(alignment - 1)
}

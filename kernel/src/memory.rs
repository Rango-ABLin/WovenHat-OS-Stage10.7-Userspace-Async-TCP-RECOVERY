use bootloader_api::info::{MemoryRegion, MemoryRegionKind};
use x86_64::{
    structures::paging::{FrameAllocator, PageSize, PhysFrame, Size4KiB},
    PhysAddr,
};

use crate::irq_lock::{IrqMutex, IrqMutexGuard};

const FRAME_SIZE: u64 = Size4KiB::SIZE;
const MAX_USABLE_REGIONS: usize = 128;
// Covers a full rollback of the maximum 8 MiB boot heap mapping (2,048 data
// frames), plus page-table and earlier reclaimed frames during initialization.
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
}

impl FrameRange {
    const fn empty() -> Self {
        Self {
            start: 0,
            next: 0,
            end: 0,
            domain: 0,
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
            numa_enabled: false,
            initialized: false,
        }
    }

    fn initialize(
        &mut self,
        regions: &[MemoryRegion],
        affinities: &[crate::hal::acpi::MemoryAffinity],
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

            let frames = (end - start) / FRAME_SIZE;
            self.total_frames = self
                .total_frames
                .checked_add(frames)
                .ok_or(InitError::AddressOverflow)?;
            self.ranges[self.range_count] = FrameRange {
                start,
                next: start,
                end,
                domain: memory_domain(start, end, affinities),
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
        if !self.contains(frame)
            || self.allocated_frames == 0
            || self.reclaimed_count == self.reclaimed_frames.len()
            || self.reclaimed_frames[..self.reclaimed_count].contains(&address)
        {
            return false;
        }

        self.reclaimed_frames[self.reclaimed_count] = address;
        self.reclaimed_domains[self.reclaimed_count] = self
            .ranges[..self.range_count]
            .iter()
            .find(|range| address >= range.start && address < range.end)
            .map_or(0, |range| range.domain);
        self.reclaimed_count += 1;
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
            self.allocated_frames += 1;
            return PhysFrame::from_start_address(PhysAddr::new(address)).ok();
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

// SAFETY: The allocator returns each fully usable 4 KiB frame at most once.
// Its only instance is protected by `ALLOCATOR`, so cursors cannot race or be
// cloned/reset while frames are live.
unsafe impl FrameAllocator<Size4KiB> for PhysicalFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        PhysicalFrameAllocator::allocate_frame(self)
    }
}

pub fn init(regions: &[MemoryRegion]) -> Result<(), InitError> {
    ALLOCATOR.lock().initialize(regions, &[])
}

pub fn init_with_topology(
    regions: &[MemoryRegion],
    affinities: &[crate::hal::acpi::MemoryAffinity],
) -> Result<(), InitError> {
    ALLOCATOR.lock().initialize(regions, affinities)
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

use bootloader_api::info::{MemoryRegion, MemoryRegionKind};
use spin::{Mutex, MutexGuard};
use x86_64::{
    structures::paging::{FrameAllocator, PageSize, PhysFrame, Size4KiB},
    PhysAddr,
};

const FRAME_SIZE: u64 = Size4KiB::SIZE;
const MAX_USABLE_REGIONS: usize = 128;
const MAX_RECLAIMED_FRAMES: usize = 1024;

static ALLOCATOR: Mutex<PhysicalFrameAllocator> = Mutex::new(PhysicalFrameAllocator::empty());

#[derive(Clone, Copy)]
struct FrameRange {
    start: u64,
    next: u64,
    end: u64,
}

impl FrameRange {
    const fn empty() -> Self {
        Self {
            start: 0,
            next: 0,
            end: 0,
        }
    }
}

pub struct Stats {
    pub usable_regions: usize,
    pub total_frames: u64,
    pub allocated_frames: u64,
    pub remaining_frames: u64,
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
    reclaimed_count: usize,
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
            reclaimed_count: 0,
            initialized: false,
        }
    }

    fn initialize(&mut self, regions: &[MemoryRegion]) -> Result<(), InitError> {
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
            };
            self.range_count += 1;
        }

        if self.total_frames == 0 {
            return Err(InitError::NoUsableFrames);
        }

        self.initialized = true;
        Ok(())
    }

    fn stats(&self) -> Stats {
        Stats {
            usable_regions: self.range_count,
            total_frames: self.total_frames,
            allocated_frames: self.allocated_frames,
            remaining_frames: self.total_frames.saturating_sub(self.allocated_frames),
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
        self.reclaimed_count += 1;
        self.allocated_frames -= 1;
        true
    }
}

// SAFETY: The allocator returns each fully usable 4 KiB frame at most once.
// Its only instance is protected by `ALLOCATOR`, so cursors cannot race or be
// cloned/reset while frames are live.
unsafe impl FrameAllocator<Size4KiB> for PhysicalFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        if !self.initialized {
            return None;
        }

        if self.reclaimed_count != 0 {
            self.reclaimed_count -= 1;
            let address = self.reclaimed_frames[self.reclaimed_count];
            self.allocated_frames += 1;
            return PhysFrame::from_start_address(PhysAddr::new(address)).ok();
        }

        while self.current_range < self.range_count {
            let range = &mut self.ranges[self.current_range];
            if range.next < range.end {
                let address = range.next;
                range.next += FRAME_SIZE;
                self.allocated_frames += 1;
                return PhysFrame::from_start_address(PhysAddr::new(address)).ok();
            }

            self.current_range += 1;
        }

        None
    }
}

pub fn init(regions: &[MemoryRegion]) -> Result<(), InitError> {
    ALLOCATOR.lock().initialize(regions)
}

pub fn allocate_frame() -> Option<PhysFrame<Size4KiB>> {
    ALLOCATOR.lock().allocate_frame()
}

pub fn deallocate_frame(frame: PhysFrame<Size4KiB>) -> bool {
    ALLOCATOR.lock().deallocate_frame(frame)
}

pub(crate) fn allocator() -> MutexGuard<'static, PhysicalFrameAllocator> {
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
    first != second
        && second != third
        && first != third
        && allocator.contains(first)
        && allocator.contains(second)
        && allocator.contains(third)
}

fn align_up(address: u64, alignment: u64) -> Option<u64> {
    address
        .checked_add(alignment - 1)
        .map(|value| align_down(value, alignment))
}

const fn align_down(address: u64, alignment: u64) -> u64 {
    address & !(alignment - 1)
}

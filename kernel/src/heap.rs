use core::{
    alloc::{GlobalAlloc, Layout},
    ptr::null_mut,
};

use alloc::{boxed::Box, vec::Vec};
use crate::irq_lock::IrqMutex as Mutex;

use crate::config::MAX_HEAP_ALLOCATIONS as MAX_ALLOCATIONS;
use crate::{memory, paging};

pub const START: u64 = 0x4444_5000_0000;
pub const MIN_SIZE: usize = 256 * 1024;
pub const MAX_SIZE: usize = 8 * 1024 * 1024;
const PAGE_SIZE: usize = 4096;

#[cfg_attr(not(test), global_allocator)]
static ALLOCATOR: TrackedAllocator = TrackedAllocator;
// Allocation can occur beneath paging, frame, process, or audit guards. The
// heap metadata never acquires another ranked lock during runtime allocation.
static HEAP: Mutex<HeapState> = Mutex::with_rank(HeapState::empty(), 50);

#[derive(Clone, Copy)]
struct FreeBlock {
    ptr: usize,
    size: usize,
}

// A coalesced contiguous arena with at most N live allocations has at most
// N + 1 free intervals. Reserve that many slots so deallocation cannot leak
// memory merely because the metadata table filled.
const MAX_FREE_BLOCKS: usize = MAX_ALLOCATIONS + 1;

struct HeapState {
    start: usize,
    end: usize,
    next: usize,
    total_allocations: usize,
    total_allocated_bytes: usize,
    live_allocations: [Option<LiveAllocation>; MAX_ALLOCATIONS],
    free_list: [Option<FreeBlock>; MAX_FREE_BLOCKS],
    free_count: usize,
}

#[derive(Clone, Copy)]
struct LiveAllocation {
    ptr: usize,
    size: usize,
    block_start: usize,
    block_size: usize,
}

impl HeapState {
    const fn empty() -> Self {
        Self {
            start: 0,
            end: 0,
            next: 0,
            total_allocations: 0,
            total_allocated_bytes: 0,
            live_allocations: [None; MAX_ALLOCATIONS],
            free_list: [const { None }; MAX_FREE_BLOCKS],
            free_count: 0,
        }
    }

    fn init(&mut self, size: usize) -> Result<(), InitError> {
        if self.start != 0 {
            return Err(InitError::AlreadyInitialized);
        }
        if !(MIN_SIZE..=MAX_SIZE).contains(&size) || !size.is_multiple_of(PAGE_SIZE) {
            return Err(InitError::AddressOverflow);
        }

        let start = usize::try_from(START).map_err(|_| InitError::AddressOverflow)?;
        let end = start.checked_add(size).ok_or(InitError::AddressOverflow)?;
        self.start = start;
        self.end = end;
        self.next = start;
        Ok(())
    }

    fn remove_free_block(&mut self, index: usize) -> FreeBlock {
        let block = self.free_list[index].take().expect("occupied free block");
        self.free_count -= 1;
        self.free_list[index] = self.free_list[self.free_count].take();
        block
    }

    fn find_free(&mut self, layout: Layout) -> Option<LiveAllocation> {
        let needed = layout.size().max(1);
        let align = layout.align();

        let mut best_idx = None;
        let mut best_size = usize::MAX;

        for i in 0..self.free_count {
            if let Some(block) = self.free_list[i] {
                let aligned_ptr = align_up(block.ptr, align)?;
                let waste = aligned_ptr.saturating_sub(block.ptr);
                if waste.checked_add(needed).is_some_and(|required| required <= block.size)
                    && block.size < best_size
                {
                    best_size = block.size;
                    best_idx = Some(i);
                }
            }
        }

        let idx = best_idx?;
        let block = self.remove_free_block(idx);

        let aligned_ptr = align_up(block.ptr, align)?;
        let waste = aligned_ptr.saturating_sub(block.ptr);
        let reserved = waste + needed;
        let remaining = block.size - reserved;

        if remaining > 0 {
            let rem_ptr = block.ptr + reserved;
            self.insert_free_block(FreeBlock {
                ptr: rem_ptr,
                size: remaining,
            });
        }

        // Keep the alignment prefix with this allocation. It is returned to
        // the free list on deallocation instead of silently disappearing.
        Some(LiveAllocation {
            ptr: aligned_ptr,
            size: layout.size(),
            block_start: block.ptr,
            block_size: reserved,
        })
    }

    fn insert_free_block(&mut self, mut block: FreeBlock) {
        if block.size == 0 {
            return;
        }
        let mut index = 0;
        while index < self.free_count {
            let existing = self.free_list[index].expect("occupied free block");
            if existing.ptr + existing.size == block.ptr {
                block.ptr = existing.ptr;
                block.size += existing.size;
                self.remove_free_block(index);
            } else if block.ptr + block.size == existing.ptr {
                block.size += existing.size;
                self.remove_free_block(index);
            } else {
                index += 1;
            }
        }
        if block.ptr + block.size == self.next {
            self.next = block.ptr;
        } else {
            assert!(self.free_count < MAX_FREE_BLOCKS, "heap free metadata exhausted");
            self.free_list[self.free_count] = Some(block);
            self.free_count += 1;
        }
    }

    fn alloc(&mut self, layout: Layout) -> *mut u8 {
        if self.start == 0 {
            return null_mut();
        }
        let slot = match self.live_allocations.iter().position(Option::is_none) {
            Some(slot) => slot,
            None => return null_mut(),
        };

        if let Some(allocation) = self.find_free(layout) {
            self.live_allocations[slot] = Some(allocation);
            self.total_allocations += 1;
            self.total_allocated_bytes += layout.size();
            return allocation.ptr as *mut u8;
        }

        let aligned = match align_up(self.next, layout.align()) {
            Some(value) => value,
            None => return null_mut(),
        };

        let end = match aligned.checked_add(layout.size().max(1)) {
            Some(value) => value,
            None => return null_mut(),
        };

        if end > self.end {
            return null_mut();
        }

        self.live_allocations[slot] = Some(LiveAllocation {
            ptr: aligned,
            size: layout.size(),
            block_start: self.next,
            block_size: end - self.next,
        });
        self.next = end;
        self.total_allocations += 1;
        self.total_allocated_bytes += layout.size();
        aligned as *mut u8
    }

    fn dealloc(&mut self, pointer: *mut u8, _layout: Layout) {
        let ptr = pointer as usize;
        let Some(slot) = self
            .live_allocations
            .iter_mut()
            .position(|s| matches!(s, Some(a) if a.ptr == ptr))
        else {
            return;
        };

        let Some(allocation) = self.live_allocations[slot].take() else {
            return;
        };

        self.total_allocations = self.total_allocations.saturating_sub(1);
        self.total_allocated_bytes = self.total_allocated_bytes.saturating_sub(allocation.size);

        self.insert_free_block(FreeBlock {
            ptr: allocation.block_start,
            size: allocation.block_size,
        });
    }

    fn stats(&self) -> Stats {
        let mut free_bytes = self.end.saturating_sub(self.next);
        for i in 0..self.free_count {
            if let Some(block) = self.free_list[i] {
                free_bytes += block.size;
            }
        }
        Stats {
            start: START,
            size: self.end.saturating_sub(self.start),
            allocated_bytes: self.total_allocated_bytes,
            free_bytes,
            allocations: self.total_allocations,
        }
    }
}

struct TrackedAllocator;

pub struct Stats {
    pub start: u64,
    pub size: usize,
    pub allocated_bytes: usize,
    pub free_bytes: usize,
    pub allocations: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InitError {
    Paging,
    AlreadyInitialized,
    AddressOverflow,
}

unsafe impl GlobalAlloc for TrackedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        HEAP.lock().alloc(layout)
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        HEAP.lock().dealloc(pointer, layout)
    }
}

pub fn init() -> Result<(), InitError> {
    if HEAP.lock().start != 0 {
        return Err(InitError::AlreadyInitialized);
    }
    // Reserve at most one sixteenth of the remaining physical frames at boot.
    // Eager mapping keeps allocation independent of paging's lower-ranked lock,
    // including allocations made from within other kernel critical sections.
    let frames = usize::try_from(memory::stats().remaining_frames).unwrap_or(usize::MAX);
    let budget = frames.saturating_mul(PAGE_SIZE) / 16;
    let size = budget.clamp(MIN_SIZE, MAX_SIZE) & !(PAGE_SIZE - 1);
    // Map pages before taking the rank-50 heap guard. Paging is rank 10.
    paging::map_range(START, size).map_err(|_| InitError::Paging)?;
    HEAP.lock().init(size)
}

pub fn self_test() -> bool {
    let boxed = Box::new(0x574F_5645_4E48_4154_u64);
    let value = *boxed;
    drop(boxed);

    let mut values = Vec::with_capacity(64);
    for value in 0..64_u64 {
        values.push(value * value);
    }

    let large_passed = if stats().size > 512 * 1024 {
        let mut large = Vec::new();
        large.resize(300 * 1024, 0x5a_u8);
        let passed = large[0] == 0x5a && large[large.len() - 1] == 0x5a;
        drop(large);
        passed
    } else {
        true
    };

    let result = large_passed
        && value == 0x574F_5645_4E48_4154
        && values.len() == 64
        && values[0] == 0
        && values[7] == 49
        && values[63] == 3969;

    drop(values);
    result
}

pub fn stats() -> Stats {
    HEAP.lock().stats()
}

fn align_up(value: usize, alignment: usize) -> Option<usize> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return None;
    }

    value
        .checked_add(alignment - 1)
        .map(|address| address & !(alignment - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> HeapState {
        let mut state = HeapState::empty();
        assert!(state.init(MIN_SIZE).is_ok());
        state
    }

    #[test]
    fn aligned_reuse_returns_all_reserved_bytes() {
        let mut heap = state();
        let byte = Layout::from_size_align(1, 1).unwrap();
        let aligned = Layout::from_size_align(8, 64).unwrap();
        let first = heap.alloc(byte);
        let middle = heap.alloc(Layout::from_size_align(127, 1).unwrap());
        let last = heap.alloc(byte);
        heap.dealloc(middle, byte);
        let reused = heap.alloc(aligned);
        assert_eq!((reused as usize) % 64, 0);
        heap.dealloc(reused, aligned);
        heap.dealloc(last, byte);
        heap.dealloc(first, byte);
        assert_eq!(heap.next, heap.start);
        assert_eq!(heap.free_count, 0);
        assert_eq!(heap.stats().free_bytes, MIN_SIZE);
    }

    #[test]
    fn full_live_table_preserves_free_space_and_allows_reuse() {
        let mut heap = state();
        let layout = Layout::from_size_align(16, 1).unwrap();
        let small = Layout::from_size_align(1, 1).unwrap();
        let mut pointers = [null_mut(); MAX_ALLOCATIONS];
        for pointer in &mut pointers {
            *pointer = heap.alloc(layout);
            assert!(!pointer.is_null());
        }
        heap.dealloc(pointers[1], layout);
        let reused = heap.alloc(small);
        assert_eq!(reused, pointers[1]);
        let free_before = heap.stats().free_bytes;
        assert!(heap.alloc(small).is_null());
        assert_eq!(heap.stats().free_bytes, free_before);
        heap.dealloc(reused, small);
        for (index, pointer) in pointers.into_iter().enumerate() {
            if index != 1 {
                heap.dealloc(pointer, layout);
            }
        }
        assert_eq!(heap.stats().free_bytes, MIN_SIZE);
    }

    #[test]
    fn fragmented_frees_coalesce_without_metadata_loss() {
        let mut heap = state();
        let layout = Layout::from_size_align(16, 8).unwrap();
        let mut pointers = [null_mut(); MAX_ALLOCATIONS];
        for pointer in &mut pointers {
            *pointer = heap.alloc(layout);
            assert!(!pointer.is_null());
        }
        for pointer in pointers.iter().step_by(2) {
            heap.dealloc(*pointer, layout);
        }
        assert_eq!(heap.free_count, MAX_ALLOCATIONS / 2);
        for pointer in pointers.iter().skip(1).step_by(2) {
            heap.dealloc(*pointer, layout);
        }
        assert_eq!(heap.next, heap.start);
        assert_eq!(heap.free_count, 0);
        assert_eq!(heap.stats().free_bytes, MIN_SIZE);
    }
}

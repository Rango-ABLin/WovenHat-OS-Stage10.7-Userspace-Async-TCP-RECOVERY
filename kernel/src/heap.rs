use core::{
    alloc::{GlobalAlloc, Layout},
    ptr::null_mut,
};

use alloc::{boxed::Box, vec::Vec};
use spin::Mutex;

use crate::config::MAX_HEAP_ALLOCATIONS as MAX_ALLOCATIONS;
use crate::paging;

pub const START: u64 = 0x4444_5000_0000;
pub const SIZE: usize = 256 * 1024;

#[global_allocator]
static ALLOCATOR: TrackedAllocator = TrackedAllocator;
static HEAP: Mutex<HeapState> = Mutex::new(HeapState::empty());

#[derive(Clone, Copy)]
struct FreeBlock {
    ptr: usize,
    size: usize,
}

const MAX_FREE_BLOCKS: usize = 128;

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

    fn init(&mut self) -> Result<(), InitError> {
        if self.start != 0 {
            return Err(InitError::AlreadyInitialized);
        }

        paging::map_range(START, SIZE).map_err(|_| InitError::Paging)?;

        let start = usize::try_from(START).map_err(|_| InitError::AddressOverflow)?;
        let end = start.checked_add(SIZE).ok_or(InitError::AddressOverflow)?;
        self.start = start;
        self.end = end;
        self.next = start;
        Ok(())
    }

    fn find_free(&mut self, layout: Layout) -> Option<usize> {
        let needed = layout.size();
        let align = layout.align();

        let mut best_idx = None;
        let mut best_size = usize::MAX;

        for i in 0..self.free_count {
            if let Some(block) = self.free_list[i] {
                let aligned_ptr = align_up(block.ptr, align)?;
                let waste = aligned_ptr.saturating_sub(block.ptr);
                if waste + needed <= block.size && block.size < best_size {
                    best_size = block.size;
                    best_idx = Some(i);
                }
            }
        }

        let idx = best_idx?;
        let block = self.free_list[idx].take()?;

        let aligned_ptr = align_up(block.ptr, align)?;
        let waste = aligned_ptr.saturating_sub(block.ptr);
        let remaining = block.size.saturating_sub(waste + needed);

        if remaining > 0 {
            let rem_ptr = aligned_ptr + needed;
            self.insert_free_block(FreeBlock {
                ptr: rem_ptr,
                size: remaining,
            });
        }

        Some(aligned_ptr)
    }

    fn insert_free_block(&mut self, block: FreeBlock) {
        if block.size == 0 {
            return;
        }
        if self.free_count < MAX_FREE_BLOCKS {
            self.free_list[self.free_count] = Some(block);
            self.free_count += 1;
        }
    }

    fn alloc(&mut self, layout: Layout) -> *mut u8 {
        if self.start == 0 {
            return null_mut();
        }

        if let Some(ptr) = self.find_free(layout) {
            let slot = match self.live_allocations.iter_mut().position(|s| s.is_none()) {
                Some(i) => i,
                None => return null_mut(),
            };
            self.live_allocations[slot] = Some(LiveAllocation {
                ptr,
                size: layout.size(),
            });
            self.total_allocations += 1;
            self.total_allocated_bytes += layout.size();
            return ptr as *mut u8;
        }

        let aligned = match align_up(self.next, layout.align()) {
            Some(value) => value,
            None => return null_mut(),
        };

        let end = match aligned.checked_add(layout.size()) {
            Some(value) => value,
            None => return null_mut(),
        };

        if end > self.end {
            return null_mut();
        }

        let slot = match self.live_allocations.iter_mut().position(|s| s.is_none()) {
            Some(i) => i,
            None => return null_mut(),
        };

        self.live_allocations[slot] = Some(LiveAllocation {
            ptr: aligned,
            size: layout.size(),
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
            ptr: allocation.ptr,
            size: allocation.size,
        });
    }

    fn stats(&self) -> Stats {
        let mut free_bytes = 0usize;
        for i in 0..self.free_count {
            if let Some(block) = self.free_list[i] {
                free_bytes += block.size;
            }
        }
        Stats {
            start: START,
            size: SIZE,
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
    HEAP.lock().init()
}

pub fn self_test() -> bool {
    let boxed = Box::new(0x574F_5645_4E48_4154_u64);
    let value = *boxed;
    drop(boxed);

    let mut values = Vec::with_capacity(64);
    for value in 0..64_u64 {
        values.push(value * value);
    }

    let result = value == 0x574F_5645_4E48_4154
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

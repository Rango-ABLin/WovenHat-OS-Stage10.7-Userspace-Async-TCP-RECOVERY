use core::{
    alloc::{GlobalAlloc, Layout},
    ptr::null_mut,
};

use alloc::{boxed::Box, vec::Vec};
use crate::irq_lock::IrqMutex as Mutex;

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

const HEADER_MAGIC: u64 = 0x5748_4845_4150_4C49;
const METADATA_ALIGN: usize = core::mem::align_of::<AllocHeader>();
const HEADER_SIZE: usize = core::mem::size_of::<AllocHeader>();
const FREE_NODE_SIZE: usize = core::mem::size_of::<FreeNode>();

#[repr(C)]
#[derive(Clone, Copy)]
struct AllocHeader {
    magic: u64,
    block_start: usize,
    block_size: usize,
    requested_size: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FreeNode {
    size: usize,
    next: usize,
}

#[derive(Clone, Copy)]
struct Placement {
    payload: usize,
    end: usize,
}

struct HeapState {
    start: usize,
    end: usize,
    next: usize,
    total_allocations: usize,
    total_allocated_bytes: usize,
    free_head: usize,
}

impl HeapState {
    const fn empty() -> Self {
        Self {
            start: 0,
            end: 0,
            next: 0,
            total_allocations: 0,
            total_allocated_bytes: 0,
            free_head: 0,
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
        self.init_at(start, size)
    }

    fn init_at(&mut self, start: usize, size: usize) -> Result<(), InitError> {
        if self.start != 0 {
            return Err(InitError::AlreadyInitialized);
        }
        if !(MIN_SIZE..=MAX_SIZE).contains(&size)
            || !size.is_multiple_of(PAGE_SIZE)
            || !start.is_multiple_of(PAGE_SIZE)
        {
            return Err(InitError::AddressOverflow);
        }
        let end = start.checked_add(size).ok_or(InitError::AddressOverflow)?;
        self.start = start;
        self.end = end;
        self.next = start;
        Ok(())
    }

    fn place(start: usize, layout: Layout) -> Option<Placement> {
        let payload = align_up(start.checked_add(HEADER_SIZE)?, layout.align().max(METADATA_ALIGN))?;
        let end = align_up(payload.checked_add(layout.size().max(1))?, METADATA_ALIGN)?;
        Some(Placement { payload, end })
    }

    fn read_free(address: usize) -> FreeNode {
        // SAFETY: Free nodes occupy aligned, mapped spans owned by HEAP.
        unsafe { (address as *const FreeNode).read() }
    }

    fn write_free(address: usize, node: FreeNode) {
        // SAFETY: The caller gives an aligned span of at least FREE_NODE_SIZE
        // bytes inside the heap, and HEAP owns the only metadata writer.
        unsafe { (address as *mut FreeNode).write(node) };
    }

    fn link_after(&mut self, previous: usize, next: usize) {
        if previous == 0 {
            self.free_head = next;
        } else {
            let mut node = Self::read_free(previous);
            node.next = next;
            Self::write_free(previous, node);
        }
    }

    fn record_allocation(&mut self, block_start: usize, end: usize, payload: usize, layout: Layout) -> *mut u8 {
        let header = AllocHeader {
            magic: HEADER_MAGIC,
            block_start,
            block_size: end - block_start,
            requested_size: layout.size(),
        };
        // SAFETY: place() aligned the payload and left HEADER_SIZE mapped
        // bytes immediately before it, exclusively owned by this allocation.
        unsafe { ((payload - HEADER_SIZE) as *mut AllocHeader).write(header) };
        self.total_allocations += 1;
        self.total_allocated_bytes += layout.size();
        payload as *mut u8
    }

    fn alloc_from_free(&mut self, layout: Layout) -> Option<*mut u8> {
        let mut previous = 0;
        let mut current = self.free_head;
        while current != 0 {
            let node = Self::read_free(current);
            let block_end = current.checked_add(node.size)?;
            if let Some(placement) = Self::place(current, layout) {
                if placement.end <= block_end {
                    let remaining = block_end - placement.end;
                    let end = if remaining < FREE_NODE_SIZE { block_end } else { placement.end };
                    if end < block_end {
                        Self::write_free(end, FreeNode {
                            size: block_end - end,
                            next: node.next,
                        });
                        self.link_after(previous, end);
                    } else {
                        self.link_after(previous, node.next);
                    }
                    return Some(self.record_allocation(current, end, placement.payload, layout));
                }
            }
            previous = current;
            current = node.next;
        }
        None
    }

    fn alloc(&mut self, layout: Layout) -> *mut u8 {
        if self.start == 0 {
            return null_mut();
        }
        if let Some(pointer) = self.alloc_from_free(layout) {
            return pointer;
        }
        let Some(placement) = Self::place(self.next, layout) else {
            return null_mut();
        };
        if placement.end > self.end {
            return null_mut();
        }
        let block_start = self.next;
        self.next = placement.end;
        self.record_allocation(block_start, placement.end, placement.payload, layout)
    }

    fn insert_free(&mut self, start: usize, size: usize) {
        let end = start + size;
        let mut before_previous = 0;
        let mut previous = 0;
        let mut current = self.free_head;
        while current != 0 && current < start {
            before_previous = previous;
            previous = current;
            current = Self::read_free(current).next;
        }
        let mut merged_start = start;
        let mut merged_size = size;
        let mut merged_next = current;
        let mut merged_previous = previous;
        if current != 0 && end == current {
            let node = Self::read_free(current);
            merged_size += node.size;
            merged_next = node.next;
        }
        if previous != 0 {
            let node = Self::read_free(previous);
            if previous + node.size == start {
                merged_start = previous;
                merged_size += node.size;
                merged_previous = before_previous;
            }
        }
        assert!(merged_size >= FREE_NODE_SIZE, "free span too small");
        if merged_start + merged_size == self.next {
            self.next = merged_start;
            self.link_after(merged_previous, merged_next);
        } else {
            Self::write_free(merged_start, FreeNode {
                size: merged_size,
                next: merged_next,
            });
            self.link_after(merged_previous, merged_start);
        }
    }

    fn dealloc(&mut self, pointer: *mut u8, layout: Layout) {
        let ptr = pointer as usize;
        if ptr < self.start.saturating_add(HEADER_SIZE) || ptr >= self.next {
            return;
        }
        // SAFETY: The pointer is within the mapped heap. A valid allocation
        // has its aligned header immediately before the returned pointer.
        let header = unsafe { ((ptr - HEADER_SIZE) as *const AllocHeader).read() };
        if header.magic != HEADER_MAGIC
            || header.requested_size != layout.size()
            || header.block_start < self.start
            || header.block_start >= self.next
            || header.block_size > self.next - header.block_start
        {
            return;
        }
        // Clear the tag before linking this span into the free list, including
        // the case where alignment left the header after the span's start.
        unsafe { ((ptr - HEADER_SIZE) as *mut u64).write(0) };
        self.total_allocations = self.total_allocations.saturating_sub(1);
        self.total_allocated_bytes = self.total_allocated_bytes.saturating_sub(header.requested_size);
        self.insert_free(header.block_start, header.block_size);
    }

    fn stats(&self) -> Stats {
        let mut free_bytes = self.end.saturating_sub(self.next);
        let mut current = self.free_head;
        while current != 0 {
            let node = Self::read_free(current);
            free_bytes += node.size;
            current = node.next;
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

#[cfg(feature = "qemu-test")]
pub fn live_metadata_self_test() -> bool {
    let before = stats();
    let mut boxes = Vec::with_capacity(4096);
    for index in 0..4096_u64 {
        boxes.push(Box::new(index ^ 0x5748_4845_4150_4C49));
    }
    let live = stats();
    let valid = live.allocations >= before.allocations + 4097
        && boxes.iter().enumerate().all(|(index, value)| {
            **value == index as u64 ^ 0x5748_4845_4150_4C49
        });
    drop(boxes);
    let after = stats();
    valid
        && after.allocations == before.allocations
        && after.allocated_bytes == before.allocated_bytes
        && after.free_bytes == before.free_bytes
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
    use core::{ops::{Deref, DerefMut}, ptr::NonNull};
    use std::alloc::{alloc_zeroed, dealloc};

    struct TestHeap {
        state: HeapState,
        arena: NonNull<u8>,
        layout: Layout,
    }

    impl TestHeap {
        fn new() -> Self {
            let layout = Layout::from_size_align(MIN_SIZE, PAGE_SIZE).unwrap();
            let arena = NonNull::new(unsafe { alloc_zeroed(layout) }).unwrap();
            let mut state = HeapState::empty();
            assert!(state.init_at(arena.as_ptr() as usize, MIN_SIZE).is_ok());
            Self { state, arena, layout }
        }
    }

    impl Deref for TestHeap {
        type Target = HeapState;
        fn deref(&self) -> &HeapState { &self.state }
    }

    impl DerefMut for TestHeap {
        fn deref_mut(&mut self) -> &mut HeapState { &mut self.state }
    }

    impl Drop for TestHeap {
        fn drop(&mut self) {
            unsafe { dealloc(self.arena.as_ptr(), self.layout) };
        }
    }

    #[test]
    fn aligned_reuse_returns_all_reserved_bytes() {
        let mut heap = TestHeap::new();
        let byte = Layout::from_size_align(1, 1).unwrap();
        let middle_layout = Layout::from_size_align(127, 1).unwrap();
        let aligned = Layout::from_size_align(8, 64).unwrap();
        let first = heap.alloc(byte);
        let middle = heap.alloc(middle_layout);
        let last = heap.alloc(byte);
        heap.dealloc(middle, middle_layout);
        let reused = heap.alloc(aligned);
        assert_eq!((reused as usize) % 64, 0);
        heap.dealloc(reused, aligned);
        heap.dealloc(last, byte);
        heap.dealloc(first, byte);
        assert_eq!(heap.next, heap.start);
        assert_eq!(heap.free_head, 0);
        assert_eq!(heap.stats().free_bytes, MIN_SIZE);
    }

    #[test]
    fn live_objects_exceed_the_former_fixed_table() {
        let mut heap = TestHeap::new();
        let layout = Layout::from_size_align(16, 1).unwrap();
        let mut pointers = Vec::new();
        for _ in 0..4096 {
            let pointer = heap.alloc(layout);
            assert!(!pointer.is_null());
            unsafe { pointer.write(0x5a) };
            pointers.push(pointer);
        }
        assert_eq!(heap.stats().allocations, 4096);
        let small = Layout::from_size_align(1, 1).unwrap();
        heap.dealloc(pointers[1], layout);
        let reused = heap.alloc(small);
        assert_eq!(reused, pointers[1]);
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
        let mut heap = TestHeap::new();
        let layout = Layout::from_size_align(16, 8).unwrap();
        let mut pointers = Vec::new();
        for _ in 0..4096 {
            let pointer = heap.alloc(layout);
            assert!(!pointer.is_null());
            pointers.push(pointer);
        }
        for pointer in pointers.iter().step_by(2) {
            heap.dealloc(*pointer, layout);
        }
        assert_ne!(heap.free_head, 0);
        for pointer in pointers.iter().skip(1).step_by(2) {
            heap.dealloc(*pointer, layout);
        }
        assert_eq!(heap.next, heap.start);
        assert_eq!(heap.free_head, 0);
        assert_eq!(heap.stats().free_bytes, MIN_SIZE);
    }

    #[test]
    fn mixed_alignment_fragmentation_preserves_live_payloads() {
        let mut heap = TestHeap::new();
        let mut live: Vec<(*mut u8, Layout, u8)> = Vec::new();
        let mut seed = 0x5748_4154_4845_4150_u64;
        for _ in 0..6000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            if !live.is_empty() && seed & 3 == 0 {
                let index = (seed as usize) % live.len();
                let (pointer, layout, pattern) = live.swap_remove(index);
                let bytes = unsafe { core::slice::from_raw_parts(pointer, layout.size()) };
                assert!(bytes.iter().all(|byte| *byte == pattern));
                heap.dealloc(pointer, layout);
            } else {
                let size = ((seed >> 16) as usize % 257) + 1;
                let align = 1usize << ((seed >> 32) as usize % 8);
                let layout = Layout::from_size_align(size, align).unwrap();
                let pointer = heap.alloc(layout);
                if !pointer.is_null() {
                    let pattern = (seed >> 48) as u8;
                    unsafe { pointer.write_bytes(pattern, size) };
                    live.push((pointer, layout, pattern));
                }
            }
        }
        for (pointer, layout, pattern) in live {
            let bytes = unsafe { core::slice::from_raw_parts(pointer, layout.size()) };
            assert!(bytes.iter().all(|byte| *byte == pattern));
            heap.dealloc(pointer, layout);
        }
        assert_eq!(heap.stats().allocations, 0);
        assert_eq!(heap.stats().free_bytes, MIN_SIZE);
        assert_eq!(heap.free_head, 0);
        assert_eq!(heap.next, heap.start);
    }
}

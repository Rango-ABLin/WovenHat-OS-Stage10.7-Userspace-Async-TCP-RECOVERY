use x86_64::{
    registers::control::{Cr3, Cr3Flags},
    registers::model_specific::{Efer, EferFlags},
    structures::paging::{
        mapper::{MapperFlush, TranslateResult},
        FrameAllocator, Mapper, OffsetPageTable, Page, PageSize, PageTable, PageTableFlags,
        PhysFrame, Size4KiB, Translate,
    },
    VirtAddr,
};

use crate::{irq_lock::IrqMutex, memory};

const TEST_PAGE_ADDRESS: u64 = 0x4444_4444_0000;
const TEST_VALUE: u64 = 0x574F_5645_4E48_4154;
#[cfg(feature = "qemu-test")]
const TEST_TABLE_FAILURE_ADDRESS: u64 = 0x5555_0000_0000;

/// Maximum number of physical frames currently participating in COW sharing.
const MAX_COW_FRAMES: usize = 1024;

// Lock order: PAGING (10) -> COW_TABLE (30) -> ALLOCATOR (40).
static PAGING: IrqMutex<PagingState> = IrqMutex::with_rank(PagingState::empty(), 10);
static COW_TABLE: IrqMutex<CowTable> = IrqMutex::with_rank(CowTable::empty(), 30);

/// Tracks reference counts for frames shared across address spaces after fork.
struct CowEntry {
    frame: u64,
    refcount: u32,
    occupied: bool,
}

impl CowEntry {
    const fn empty() -> Self {
        Self {
            frame: 0,
            refcount: 0,
            occupied: false,
        }
    }
}

struct CowTable {
    entries: [CowEntry; MAX_COW_FRAMES],
}

impl CowTable {
    const fn empty() -> Self {
        Self {
            entries: [const { CowEntry::empty() }; MAX_COW_FRAMES],
        }
    }

    fn find_slot(&self, frame: u64) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.occupied && entry.frame == frame)
    }

    /// Register a newly shared frame. First share sets refcount to 2 (parent + child).
    fn share(&mut self, frame: u64) -> Result<(), MapRangeError> {
        if let Some(slot) = self.find_slot(frame) {
            self.entries[slot].refcount = self.entries[slot]
                .refcount
                .checked_add(1)
                .ok_or(MapRangeError::OutOfFrames)?;
            return Ok(());
        }
        let slot = self
            .entries
            .iter()
            .position(|entry| !entry.occupied)
            .ok_or(MapRangeError::OutOfFrames)?;
        self.entries[slot] = CowEntry {
            frame,
            refcount: 2,
            occupied: true,
        };
        Ok(())
    }

    /// Drop one reference. Returns true if the frame should be freed.
    fn release(&mut self, frame: u64) -> bool {
        let Some(slot) = self.find_slot(frame) else {
            // Not shared — caller should free normally.
            return true;
        };
        let entry = &mut self.entries[slot];
        entry.refcount = entry.refcount.saturating_sub(1);
        if entry.refcount == 0 {
            *entry = CowEntry::empty();
            return true;
        }
        false
    }

    fn refcount(&self, frame: u64) -> u32 {
        self.find_slot(frame)
            .map(|slot| self.entries[slot].refcount)
            .unwrap_or(1)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AddressSpace {
    level_4_frame: PhysFrame<Size4KiB>,
}

impl AddressSpace {
    pub const fn root_address(self) -> u64 {
        self.level_4_frame.start_address().as_u64()
    }
}
pub struct Stats {
    pub physical_memory_offset: u64,
    pub level_4_frame: u64,
    pub successful_translations: usize,
    pub tested_translations: usize,
    pub mapping_test_passed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InitError {
    MissingPhysicalMemoryMapping,
    AlreadyInitialized,
    AddressOverflow,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MapRangeError {
    NotInitialized,
    InvalidRange,
    AlreadyMapped,
    OutOfFrames,
    MappingFailed,
    NotMapped,
}

/// The x86_64 mapper can allocate a P3, P2, and P1 table before reporting a
/// later failure. Record those frames so a failed map cannot strand them.
struct TableFrameRecorder<'a> {
    allocator: &'a mut memory::PhysicalFrameAllocator,
    frames: [Option<PhysFrame<Size4KiB>>; 3],
    count: usize,
    limit: usize,
}

impl<'a> TableFrameRecorder<'a> {
    fn new(allocator: &'a mut memory::PhysicalFrameAllocator, limit: usize) -> Self {
        Self {
            allocator,
            frames: [None; 3],
            count: 0,
            limit,
        }
    }
}

// SAFETY: Every frame is obtained from the uniquely borrowed physical
// allocator. The recorder only limits and remembers table-frame allocations.
unsafe impl FrameAllocator<Size4KiB> for TableFrameRecorder<'_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        if self.count >= self.limit || self.count >= self.frames.len() {
            return None;
        }
        let frame = self.allocator.allocate_frame()?;
        self.frames[self.count] = Some(frame);
        self.count += 1;
        Some(frame)
    }
}

fn reclaim_failed_tables(
    mapper: &mut OffsetPageTable<'_>,
    page: Page<Size4KiB>,
    frames: [Option<PhysFrame<Size4KiB>>; 3],
    allocator: &mut memory::PhysicalFrameAllocator,
) -> bool {
    let offset = mapper.phys_offset().as_u64();
    for frame in frames.into_iter().rev().flatten() {
        let address = frame.start_address().as_u64();
        let root = mapper.level_4_table_mut();
        let p4 = &mut root[page.p4_index()];
        if p4.addr().as_u64() == address {
            p4.set_unused();
        } else {
            if !p4.flags().contains(PageTableFlags::PRESENT)
                || p4.flags().contains(PageTableFlags::HUGE_PAGE)
            {
                return false;
            }
            let Some(p3) = page_table_at_mut(offset, p4.addr().as_u64()) else {
                return false;
            };
            let p3_entry = &mut p3[page.p3_index()];
            if p3_entry.addr().as_u64() == address {
                p3_entry.set_unused();
            } else {
                if !p3_entry.flags().contains(PageTableFlags::PRESENT)
                    || p3_entry.flags().contains(PageTableFlags::HUGE_PAGE)
                {
                    return false;
                }
                let Some(p2) = page_table_at_mut(offset, p3_entry.addr().as_u64()) else {
                    return false;
                };
                let p2_entry = &mut p2[page.p2_index()];
                if p2_entry.addr().as_u64() != address {
                    return false;
                }
                p2_entry.set_unused();
            }
        }
        if !allocator.deallocate_frame(frame) {
            return false;
        }
    }
    true
}

fn map_to_reclaim_on_failure(
    mapper: &mut OffsetPageTable<'_>,
    page: Page<Size4KiB>,
    frame: PhysFrame<Size4KiB>,
    flags: PageTableFlags,
    allocator: &mut memory::PhysicalFrameAllocator,
    table_frame_limit: usize,
) -> Result<MapperFlush<Size4KiB>, MapRangeError> {
    let (result, frames) = {
        let mut recorder = TableFrameRecorder::new(allocator, table_frame_limit);
        // SAFETY: The caller supplied a unique data frame and the recorder
        // forwards unique table frames from the physical allocator.
        let result = unsafe { mapper.map_to(page, frame, flags, &mut recorder) };
        (result, recorder.frames)
    };
    match result {
        Ok(flush) => Ok(flush),
        Err(_) => {
            assert!(
                reclaim_failed_tables(mapper, page, frames, allocator),
                "failed page-table mapping could not reclaim its tables"
            );
            Err(MapRangeError::MappingFailed)
        }
    }
}

struct PagingState {
    mapper: Option<OffsetPageTable<'static>>,
    physical_memory_offset: u64,
    level_4_frame: u64,
    successful_translations: usize,
    tested_translations: usize,
    mapping_test_passed: bool,
}

impl PagingState {
    const fn empty() -> Self {
        Self {
            mapper: None,
            physical_memory_offset: 0,
            level_4_frame: 0,
            successful_translations: 0,
            tested_translations: 0,
            mapping_test_passed: false,
        }
    }

    fn stats(&self) -> Stats {
        Stats {
            physical_memory_offset: self.physical_memory_offset,
            level_4_frame: self.level_4_frame,
            successful_translations: self.successful_translations,
            tested_translations: self.tested_translations,
            mapping_test_passed: self.mapping_test_passed,
        }
    }
}

pub fn init(physical_memory_offset: u64) -> Result<(), InitError> {
    // SAFETY: NXE is enabled before any no-execute mappings are created.
    unsafe { Efer::update(|flags| *flags |= EferFlags::NO_EXECUTE_ENABLE) };

    let mut paging = PAGING.lock();
    if paging.mapper.is_some() {
        return Err(InitError::AlreadyInitialized);
    }

    let offset = VirtAddr::new(physical_memory_offset);
    let (level_4_frame, _) = Cr3::read();
    let table_address = physical_memory_offset
        .checked_add(level_4_frame.start_address().as_u64())
        .ok_or(InitError::AddressOverflow)?;

    // SAFETY: The bootloader maps all physical memory at `offset`. CR3 names
    // the active level-4 table, and PAGING creates the only mutable Rust view
    // of that table for the remainder of kernel execution.
    let level_4_table = unsafe { &mut *(table_address as *mut PageTable) };

    // SAFETY: `level_4_table` is the uniquely borrowed active table and
    // `offset` is the bootloader-provided physical-memory mapping base.
    let mapper = unsafe { OffsetPageTable::new(level_4_table, offset) };

    paging.level_4_frame = level_4_frame.start_address().as_u64();
    paging.physical_memory_offset = physical_memory_offset;
    paging.mapper = Some(mapper);
    Ok(())
}

/// Return the bootloader-provided direct-map base used to access physical
/// frames from kernel code and DMA arenas.
pub fn physical_memory_offset() -> Option<u64> {
    let paging = PAGING.lock();
    paging
        .mapper
        .as_ref()
        .map(|_| paging.physical_memory_offset)
}

pub fn self_test(addresses: &[u64]) -> bool {
    let mut paging = PAGING.lock();
    let Some(mapper) = paging.mapper.as_ref() else {
        return false;
    };

    let successful = addresses
        .iter()
        .filter(|address| mapper.translate_addr(VirtAddr::new(**address)).is_some())
        .count();

    paging.successful_translations = successful;
    paging.tested_translations = addresses.len();
    successful == addresses.len()
}

pub fn mapping_self_test() -> bool {
    let mut paging = PAGING.lock();
    let Some(mapper) = paging.mapper.as_mut() else {
        return false;
    };

    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(TEST_PAGE_ADDRESS));
    if mapper.translate_addr(page.start_address()).is_some() {
        return false;
    }

    let mut allocator = memory::allocator();
    let Some(frame) = allocator.allocate_frame() else {
        return false;
    };
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

    // SAFETY: `frame` was freshly allocated and `page` was verified unmapped.
    // The paging mutex provides exclusive access to the active page tables.
    let mapping = unsafe { mapper.map_to(page, frame, flags, &mut *allocator) };
    let Ok(flush) = mapping else {
        return false;
    };
    flush.flush();
    crate::smp::shootdown();

    let pointer = page.start_address().as_mut_ptr::<u64>();
    // SAFETY: The page is present and writable for this test, and the pointer
    // is naturally aligned within that mapping.
    unsafe { pointer.write_volatile(TEST_VALUE) };
    // SAFETY: The same live mapping and aligned location are read back before
    // the page is unmapped.
    let value = unsafe { pointer.read_volatile() };

    let Ok((frame, flush)) = mapper.unmap(page) else {
        return false;
    };
    flush.flush();
    crate::smp::shootdown();

    let released = allocator.deallocate_frame(frame);
    let passed =
        released && value == TEST_VALUE && mapper.translate_addr(page.start_address()).is_none();
    paging.mapping_test_passed = passed;
    passed
}

/// A failure halfway through a kernel mapping must leave every newly mapped
/// page absent and preserve a page that was already present in the range.
pub fn map_range_rollback_self_test() -> bool {
    let second = TEST_PAGE_ADDRESS + Size4KiB::SIZE;
    if map_range(second, Size4KiB::SIZE as usize).is_err() {
        return false;
    }
    let rejected = matches!(
        map_range(TEST_PAGE_ADDRESS, 2 * Size4KiB::SIZE as usize),
        Err(MapRangeError::AlreadyMapped)
    );
    let (first_absent, second_present, frame) = {
        let mut paging = PAGING.lock();
        let Some(mapper) = paging.mapper.as_mut() else {
            return false;
        };
        let first_page = Page::<Size4KiB>::containing_address(VirtAddr::new(TEST_PAGE_ADDRESS));
        let second_page = Page::<Size4KiB>::containing_address(VirtAddr::new(second));
        let first_absent = mapper.translate_addr(first_page.start_address()).is_none();
        let second_present = mapper.translate_addr(second_page.start_address()).is_some();
        let Ok((frame, flush)) = mapper.unmap(second_page) else {
            return false;
        };
        flush.flush();
        (first_absent, second_present, frame)
    };
    crate::smp::shootdown();
    let released = memory::deallocate_frame(frame);
    rejected && first_absent && second_present && released
}

#[cfg(feature = "qemu-test")]
pub fn table_allocation_rollback_self_test() -> bool {
    let before = memory::stats().allocated_frames;
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(TEST_TABLE_FAILURE_ADDRESS));
    let passed = {
        let mut paging = PAGING.lock();
        let Some(mapper) = paging.mapper.as_mut() else {
            return false;
        };
        if !mapper.level_4_table()[page.p4_index()].is_unused()
            || mapper.translate_addr(page.start_address()).is_some()
        {
            return false;
        }
        let mut allocator = memory::allocator();
        let mut passed = true;
        for limit in [1, 2] {
            let Some(frame) = allocator.allocate_frame() else {
                return false;
            };
            let rejected = map_to_reclaim_on_failure(
                mapper,
                page,
                frame,
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
                &mut allocator,
                limit,
            )
            .is_err();
            passed &= rejected
                && mapper.level_4_table()[page.p4_index()].is_unused()
                && mapper.translate_addr(page.start_address()).is_none()
                && allocator.deallocate_frame(frame);
        }
        passed
    };
    passed && memory::stats().allocated_frames == before
}

pub fn map_range(start: u64, size: usize) -> Result<(), MapRangeError> {
    map_range_with_flags(
        start,
        size,
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
    )
}

fn user_flags(writable: bool, executable: bool) -> PageTableFlags {
    let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if writable {
        flags |= PageTableFlags::WRITABLE;
    }
    if !executable {
        flags |= PageTableFlags::NO_EXECUTE;
    }
    flags
}

fn page_range(start: u64, size: usize) -> Result<(Page<Size4KiB>, Page<Size4KiB>), MapRangeError> {
    if size == 0
        || !start.is_multiple_of(Size4KiB::SIZE)
        || !size.is_multiple_of(Size4KiB::SIZE as usize)
    {
        return Err(MapRangeError::InvalidRange);
    }
    let size = u64::try_from(size).map_err(|_| MapRangeError::InvalidRange)?;
    let last = start
        .checked_add(size - 1)
        .ok_or(MapRangeError::InvalidRange)?;
    Ok((
        Page::containing_address(VirtAddr::new(start)),
        Page::containing_address(VirtAddr::new(last)),
    ))
}

fn map_range_with_flags(
    start: u64,
    size: usize,
    flags: PageTableFlags,
) -> Result<(), MapRangeError> {
    let (start_page, end_page) = page_range(start, size)?;

    let mut paging = PAGING.lock();
    let Some(mapper) = paging.mapper.as_mut() else {
        return Err(MapRangeError::NotInitialized);
    };
    let mut allocator = memory::allocator();

    let mut mapped_pages = 0;
    for page in Page::range_inclusive(start_page, end_page) {
        let failure = if mapper.translate_addr(page.start_address()).is_some() {
            Some(MapRangeError::AlreadyMapped)
        } else if let Some(frame) = allocator.allocate_frame() {
            // SAFETY: Each page is checked to be unmapped and each frame comes
            // uniquely from the physical allocator. Both allocators are locked.
            match map_to_reclaim_on_failure(mapper, page, frame, flags, &mut allocator, 3) {
                Ok(flush) => {
                    flush.flush();
                    crate::smp::shootdown();
                    mapped_pages += 1;
                    None
                }
                Err(_) => {
                    let _ = allocator.deallocate_frame(frame);
                    Some(MapRangeError::MappingFailed)
                }
            }
        } else {
            Some(MapRangeError::OutOfFrames)
        };

        if let Some(error) = failure {
            for rollback_page in Page::range_inclusive(start_page, end_page).take(mapped_pages) {
                if let Ok((frame, flush)) = mapper.unmap(rollback_page) {
                    flush.flush();
                    crate::smp::shootdown();
                    let _ = allocator.deallocate_frame(frame);
                }
            }
            return Err(error);
        }
    }

    Ok(())
}

pub fn kernel_address_space() -> Option<AddressSpace> {
    let paging = PAGING.lock();
    paging.mapper.as_ref()?;
    PhysFrame::from_start_address(x86_64::PhysAddr::new(paging.level_4_frame))
        .ok()
        .map(|level_4_frame| AddressSpace { level_4_frame })
}

pub fn create_user_address_space(user_address: u64) -> Option<AddressSpace> {
    let paging = PAGING.lock();
    paging.mapper.as_ref()?;
    let root_frame = memory::allocate_frame()?;
    let kernel_table = page_table_at(paging.physical_memory_offset, paging.level_4_frame)?;
    let new_table = page_table_at_mut(
        paging.physical_memory_offset,
        root_frame.start_address().as_u64(),
    )?;
    *new_table = kernel_table.clone();
    new_table[Page::<Size4KiB>::containing_address(VirtAddr::new(user_address)).p4_index()]
        .set_unused();
    Some(AddressSpace {
        level_4_frame: root_frame,
    })
}

pub fn map_user_range_in(
    address_space: AddressSpace,
    start: u64,
    size: usize,
    writable: bool,
    executable: bool,
) -> Result<(), MapRangeError> {
    map_user_range_in_impl(address_space, start, size, writable, executable, true)
}

/// Map pages into an address space that is not installed in CR3 on any CPU.
///
/// ELF construction and rollback operate on brand-new, unpublished address
/// spaces.  Sending acknowledged global TLB shootdowns while editing such a
/// page table is both unnecessary and, under SMP load, creates an avoidable
/// cross-CPU liveness dependency.  Keep the public mapping primitive
/// conservative for potentially-live spaces, while giving the loader an
/// explicit no-shootdown path whose safety contract is easy to audit.
pub(crate) fn map_user_range_in_inactive(
    address_space: AddressSpace,
    start: u64,
    size: usize,
    writable: bool,
    executable: bool,
) -> Result<(), MapRangeError> {
    map_user_range_in_impl(address_space, start, size, writable, executable, false)
}

fn map_user_range_in_impl(
    address_space: AddressSpace,
    start: u64,
    size: usize,
    writable: bool,
    executable: bool,
    synchronize_tlb: bool,
) -> Result<(), MapRangeError> {
    let (start_page, end_page) = page_range(start, size)?;
    let paging = PAGING.lock();
    let mut mapper = mapper_for(&paging, address_space)?;
    let mut allocator = memory::allocator();
    let flags = user_flags(writable, executable);
    let mut mapped_pages = 0;

    for page in Page::range_inclusive(start_page, end_page) {
        let failure = if mapper.translate_addr(page.start_address()).is_some() {
            Some(MapRangeError::AlreadyMapped)
        } else if let Some(frame) = allocator.allocate_frame() {
            match map_to_reclaim_on_failure(&mut mapper, page, frame, flags, &mut allocator, 3) {
                Ok(flush) => {
                    if synchronize_tlb {
                        if Cr3::read().0 == address_space.level_4_frame {
                            flush.flush();
                        } else {
                            flush.ignore();
                        }
                        crate::smp::shootdown();
                    } else {
                        flush.ignore();
                    }
                    mapped_pages += 1;
                    None
                }
                Err(_) => {
                    let _ = allocator.deallocate_frame(frame);
                    Some(MapRangeError::MappingFailed)
                }
            }
        } else {
            Some(MapRangeError::OutOfFrames)
        };

        if let Some(error) = failure {
            for rollback_page in Page::range_inclusive(start_page, end_page).take(mapped_pages) {
                if let Ok((frame, flush)) = mapper.unmap(rollback_page) {
                    if synchronize_tlb {
                        if Cr3::read().0 == address_space.level_4_frame {
                            flush.flush();
                        } else {
                            flush.ignore();
                        }
                        crate::smp::shootdown();
                    } else {
                        flush.ignore();
                    }
                    let _ = allocator.deallocate_frame(frame);
                }
            }
            return Err(error);
        }
    }
    Ok(())
}
/// Eager byte-copy clone (legacy). Prefer [`share_user_range_in`] for fork.
#[allow(dead_code)]
pub fn clone_user_range_in(
    source: AddressSpace,
    destination: AddressSpace,
    start: u64,
    size: usize,
    writable: bool,
    executable: bool,
) -> Result<(), MapRangeError> {
    if source == destination {
        return Err(MapRangeError::InvalidRange);
    }
    map_user_range_in(destination, start, size, writable, executable)?;
    let copy_result = (|| {
        let (start_page, end_page) = page_range(start, size)?;
        let paging = PAGING.lock();
        let source_mapper = mapper_for(&paging, source)?;
        let destination_mapper = mapper_for(&paging, destination)?;
        for page in Page::range_inclusive(start_page, end_page) {
            let TranslateResult::Mapped {
                frame,
                offset,
                flags,
            } = source_mapper.translate(page.start_address())
            else {
                return Err(MapRangeError::NotMapped);
            };
            if offset != 0
                || !flags.contains(PageTableFlags::USER_ACCESSIBLE)
                || flags.contains(PageTableFlags::WRITABLE) != writable
                || flags.contains(PageTableFlags::NO_EXECUTE) == executable
            {
                return Err(MapRangeError::MappingFailed);
            }
            let source_physical = frame.start_address().as_u64();
            let destination_physical = destination_mapper
                .translate_addr(page.start_address())
                .ok_or(MapRangeError::NotMapped)?
                .as_u64();
            let source_pointer = paging
                .physical_memory_offset
                .checked_add(source_physical)
                .ok_or(MapRangeError::InvalidRange)? as *const u8;
            let destination_pointer = paging
                .physical_memory_offset
                .checked_add(destination_physical)
                .ok_or(MapRangeError::InvalidRange)?
                as *mut u8;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    source_pointer,
                    destination_pointer,
                    Size4KiB::SIZE as usize,
                );
            }
        }
        Ok(())
    })();
    if copy_result.is_err() {
        let _ = unmap_user_range_in(destination, start, size);
    }
    copy_result
}

/// Copy-on-write share: map `destination` to the same physical frames as `source`.
///
/// If the logical mapping is writable, both address spaces receive the page as
/// read-only. The first write faults and is resolved by [`try_break_cow`].
pub fn share_user_range_in(
    source: AddressSpace,
    destination: AddressSpace,
    start: u64,
    size: usize,
    writable: bool,
    executable: bool,
) -> Result<(), MapRangeError> {
    if source == destination {
        return Err(MapRangeError::InvalidRange);
    }
    let (start_page, end_page) = page_range(start, size)?;
    // Writable pages become RO in both spaces so a later write can break COW.
    let shared_writable = false;
    let flags = user_flags(shared_writable, executable);

    let paging = PAGING.lock();
    let mut source_mapper = mapper_for(&paging, source)?;
    let mut destination_mapper = mapper_for(&paging, destination)?;
    let mut cow = COW_TABLE.lock();
    let mut allocator = memory::allocator();

    for (mapped_pages, page) in Page::range_inclusive(start_page, end_page).enumerate() {
        let TranslateResult::Mapped {
            frame,
            offset,
            flags: source_flags,
        } = source_mapper.translate(page.start_address())
        else {
            rollback_shared(
                &mut destination_mapper,
                &mut allocator,
                &mut cow,
                start_page,
                mapped_pages,
            );
            return Err(MapRangeError::NotMapped);
        };
        if offset != 0 || !source_flags.contains(PageTableFlags::USER_ACCESSIBLE) {
            rollback_shared(
                &mut destination_mapper,
                &mut allocator,
                &mut cow,
                start_page,
                mapped_pages,
            );
            return Err(MapRangeError::MappingFailed);
        }
        let frame = PhysFrame::<Size4KiB>::from_start_address(frame.start_address())
            .map_err(|_| MapRangeError::MappingFailed)?;

        if destination_mapper
            .translate_addr(page.start_address())
            .is_some()
        {
            rollback_shared(
                &mut destination_mapper,
                &mut allocator,
                &mut cow,
                start_page,
                mapped_pages,
            );
            return Err(MapRangeError::AlreadyMapped);
        }

        // Reserve ownership before publishing a PTE. If the ref table is full,
        // rollback must never release an unregistered reference to a live frame.
        if cow.share(frame.start_address().as_u64()).is_err() {
            rollback_shared(
                &mut destination_mapper,
                &mut allocator,
                &mut cow,
                start_page,
                mapped_pages,
            );
            return Err(MapRangeError::OutOfFrames);
        }
        // Map destination to the same frame.
        match unsafe { destination_mapper.map_to(page, frame, flags, &mut *allocator) } {
            Ok(flush) => {
                flush.ignore();
                crate::smp::shootdown();
            }
            Err(_) => {
                let _ = cow.release(frame.start_address().as_u64());
                rollback_shared(
                    &mut destination_mapper,
                    &mut allocator,
                    &mut cow,
                    start_page,
                    mapped_pages,
                );
                return Err(MapRangeError::MappingFailed);
            }
        }

        // Strip write permission from the source page when the range is logically writable.
        if writable && source_flags.contains(PageTableFlags::WRITABLE) {
            let ro_flags = user_flags(false, executable);
            if let Ok(flush) = unsafe { source_mapper.update_flags(page, ro_flags) } {
                flush.flush();
                crate::smp::shootdown();
            } else {
                rollback_shared(
                    &mut destination_mapper,
                    &mut allocator,
                    &mut cow,
                    start_page,
                    mapped_pages + 1,
                );
                return Err(MapRangeError::MappingFailed);
            }
        }
    }
    Ok(())
}

fn rollback_shared(
    destination_mapper: &mut OffsetPageTable<'static>,
    allocator: &mut memory::PhysicalFrameAllocator,
    cow: &mut CowTable,
    start_page: Page<Size4KiB>,
    mapped_pages: usize,
) {
    let mut page = start_page;
    for _ in 0..mapped_pages {
        if let Ok((frame, flush)) = destination_mapper.unmap(page) {
            flush.ignore();
            crate::smp::shootdown();
            let phys = frame.start_address().as_u64();
            if cow.release(phys) {
                let _ = allocator.deallocate_frame(frame);
            }
        }
        page = Page::containing_address(page.start_address() + Size4KiB::SIZE);
    }
}

/// Resolve a user write fault on a COW page.
///
/// Returns `true` if the fault was handled (caller should resume the process).
pub fn try_break_cow(address_space: AddressSpace, fault_address: u64) -> bool {
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(fault_address));
    let paging = PAGING.lock();
    let Ok(mut mapper) = mapper_for(&paging, address_space) else {
        return false;
    };

    let TranslateResult::Mapped {
        frame: old_frame,
        offset,
        flags,
    } = mapper.translate(page.start_address())
    else {
        return false;
    };
    if offset != 0 || !flags.contains(PageTableFlags::PRESENT) {
        return false;
    }
    let Ok(old_frame) = PhysFrame::<Size4KiB>::from_start_address(old_frame.start_address()) else {
        return false;
    };
    // Only break when the hardware page is currently read-only.
    if flags.contains(PageTableFlags::WRITABLE) {
        return false;
    }

    let old_phys = old_frame.start_address().as_u64();
    let mut cow = COW_TABLE.lock();
    let refs = cow.refcount(old_phys);

    // Preserve NX / USER bits from the existing mapping; add WRITABLE.
    let mut new_flags =
        PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE;
    if flags.contains(PageTableFlags::NO_EXECUTE) {
        new_flags |= PageTableFlags::NO_EXECUTE;
    }

    if refs <= 1 {
        // Sole owner: just restore write permission. Keep the frame.
        match unsafe { mapper.update_flags(page, new_flags) } {
            Ok(flush) => {
                flush.flush();
                crate::smp::shootdown();
            }
            Err(_) => return false,
        }
        if let Some(slot) = cow.find_slot(old_phys) {
            cow.entries[slot] = CowEntry::empty();
        }
        return true;
    }

    // Shared: allocate a private copy.
    let Some(new_frame) = memory::allocate_frame() else {
        return false;
    };
    let source_pointer = (paging
        .physical_memory_offset
        .checked_add(old_phys)
        .unwrap_or(0)) as *const u8;
    let destination_pointer = (paging
        .physical_memory_offset
        .checked_add(new_frame.start_address().as_u64())
        .unwrap_or(0)) as *mut u8;
    if source_pointer.is_null() || destination_pointer.is_null() {
        let _ = memory::deallocate_frame(new_frame);
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            source_pointer,
            destination_pointer,
            Size4KiB::SIZE as usize,
        );
    }

    // Replace mapping. x86_64 Mapper has no direct remap, so unmap + map.
    let Ok((_, flush)) = mapper.unmap(page) else {
        let _ = memory::deallocate_frame(new_frame);
        return false;
    };
    flush.ignore();
    crate::smp::shootdown();

    let mut allocator = memory::allocator();
    match unsafe { mapper.map_to(page, new_frame, new_flags, &mut *allocator) } {
        Ok(flush) => {
            flush.flush();
            crate::smp::shootdown();
        }
        Err(_) => {
            // Best-effort restore of the old mapping.
            let _ = unsafe { mapper.map_to(page, old_frame, flags, &mut *allocator) };
            let _ = memory::deallocate_frame(new_frame);
            return false;
        }
    }

    // Drop one reference on the shared frame.
    if cow.release(old_phys) {
        let _ = memory::deallocate_frame(old_frame);
    }
    true
}
pub(crate) fn protect_user_range_in_inactive(
    address_space: AddressSpace,
    start: u64,
    size: usize,
    writable: bool,
    executable: bool,
) -> Result<(), MapRangeError> {
    protect_user_range_in_impl(address_space, start, size, writable, executable, false)
}

fn protect_user_range_in_impl(
    address_space: AddressSpace,
    start: u64,
    size: usize,
    writable: bool,
    executable: bool,
    synchronize_tlb: bool,
) -> Result<(), MapRangeError> {
    let (start_page, end_page) = page_range(start, size)?;
    let paging = PAGING.lock();
    let mut mapper = mapper_for(&paging, address_space)?;
    let flags = user_flags(writable, executable);

    for page in Page::range_inclusive(start_page, end_page) {
        let flush =
            unsafe { mapper.update_flags(page, flags) }.map_err(|_| MapRangeError::NotMapped)?;
        if synchronize_tlb {
            if Cr3::read().0 == address_space.level_4_frame {
                flush.flush();
            } else {
                flush.ignore();
            }
            crate::smp::shootdown();
        } else {
            flush.ignore();
        }
    }
    Ok(())
}

pub fn write_user_bytes(
    address_space: AddressSpace,
    start: u64,
    bytes: &[u8],
) -> Result<(), MapRangeError> {
    let paging = PAGING.lock();
    let mapper = mapper_for(&paging, address_space)?;
    let mut copied = 0;
    while copied < bytes.len() {
        let virtual_address = start
            .checked_add(copied as u64)
            .ok_or(MapRangeError::InvalidRange)?;
        let physical_address = mapper
            .translate_addr(VirtAddr::new(virtual_address))
            .ok_or(MapRangeError::NotMapped)?;
        let page_remaining =
            Size4KiB::SIZE as usize - virtual_address as usize % Size4KiB::SIZE as usize;
        let count = core::cmp::min(page_remaining, bytes.len() - copied);
        let destination = paging
            .physical_memory_offset
            .checked_add(physical_address.as_u64())
            .ok_or(MapRangeError::InvalidRange)? as *mut u8;
        unsafe {
            core::ptr::copy_nonoverlapping(bytes[copied..].as_ptr(), destination, count);
        }
        copied += count;
    }
    Ok(())
}

/// Inspect user pages through the kernel's physical mapping without switching CR3.
pub(crate) fn read_user_bytes_in(
    address_space: AddressSpace,
    start: u64,
    output: &mut [u8],
) -> Result<(), MapRangeError> {
    let paging = PAGING.lock();
    let mapper = mapper_for(&paging, address_space)?;
    let mut copied = 0;
    while copied < output.len() {
        let address = start
            .checked_add(copied as u64)
            .ok_or(MapRangeError::InvalidRange)?;
        let (physical, count, flags) = translated_chunk(&mapper, address, output.len() - copied)
            .map_err(|_| MapRangeError::NotMapped)?;
        if !flags.contains(PageTableFlags::USER_ACCESSIBLE) {
            return Err(MapRangeError::InvalidRange);
        }
        let source = paging
            .physical_memory_offset
            .checked_add(physical)
            .ok_or(MapRangeError::InvalidRange)? as *const u8;
        // The paging lock keeps the translated, allocated frame live while copying.
        unsafe {
            core::ptr::copy_nonoverlapping(source, output[copied..].as_mut_ptr(), count);
        }
        copied += count;
    }
    Ok(())
}

pub fn zero_user_range_in(
    address_space: AddressSpace,
    start: u64,
    size: usize,
) -> Result<(), MapRangeError> {
    let _ = page_range(start, size)?;
    let paging = PAGING.lock();
    let mapper = mapper_for(&paging, address_space)?;
    let mut cleared = 0;
    while cleared < size {
        let virtual_address = start
            .checked_add(cleared as u64)
            .ok_or(MapRangeError::InvalidRange)?;
        let physical_address = mapper
            .translate_addr(VirtAddr::new(virtual_address))
            .ok_or(MapRangeError::NotMapped)?;
        let page_remaining =
            Size4KiB::SIZE as usize - virtual_address as usize % Size4KiB::SIZE as usize;
        let count = core::cmp::min(page_remaining, size - cleared);
        let destination = paging
            .physical_memory_offset
            .checked_add(physical_address.as_u64())
            .ok_or(MapRangeError::InvalidRange)? as *mut u8;
        unsafe { core::ptr::write_bytes(destination, 0, count) };
        cleared += count;
    }
    Ok(())
}
fn release_frame(frame: PhysFrame<Size4KiB>) -> bool {
    let phys = frame.start_address().as_u64();
    let should_free = COW_TABLE.lock().release(phys);
    if should_free {
        memory::deallocate_frame(frame)
    } else {
        true
    }
}

pub fn unmap_user_range_in(
    address_space: AddressSpace,
    start: u64,
    size: usize,
) -> Result<(), MapRangeError> {
    let (start_page, end_page) = page_range(start, size)?;
    let active = Cr3::read().0 == address_space.level_4_frame;
    let paging = PAGING.lock();
    let mut mapper = mapper_for(&paging, address_space)?;
    for page in Page::range_inclusive(start_page, end_page) {
        let (frame, flush) = mapper.unmap(page).map_err(|_| MapRangeError::NotMapped)?;
        if active {
            flush.flush();
            crate::smp::shootdown();
        } else {
            flush.ignore();
            crate::smp::shootdown();
        }
        if !release_frame(frame) {
            return Err(MapRangeError::MappingFailed);
        }
    }
    Ok(())
}

/// Destroy an address space that has been retired or was never published.
///
/// Safety/liveness contract: no CPU may have `address_space` installed in CR3.
/// Callers satisfy this by using the routine only for loader rollback, failed
/// clones, tests, or scheduler-retired processes.  Because no CPU can cache a
/// translation belonging to this CR3, global TLB shootdowns are neither
/// necessary nor correct synchronization for this teardown path.
pub fn destroy_user_address_space(
    address_space: AddressSpace,
    ranges: &[(u64, usize)],
) -> Result<(), MapRangeError> {
    if ranges.is_empty() || Cr3::read().0 == address_space.level_4_frame {
        return Err(MapRangeError::MappingFailed);
    }

    let paging = PAGING.lock();
    let mut mapper = mapper_for(&paging, address_space)?;
    for &(start, size) in ranges {
        let (start_page, end_page) = page_range(start, size)?;
        for page in Page::range_inclusive(start_page, end_page) {
            let (frame, flush) = mapper.unmap(page).map_err(|_| MapRangeError::NotMapped)?;
            // The address space is retired/unpublished by contract, so no CPU
            // can hold a translation for it.  Do not introduce a synchronous
            // cross-CPU shootdown while tearing it down.
            flush.ignore();
            if !release_frame(frame) {
                return Err(MapRangeError::MappingFailed);
            }
        }
    }

    let first_page = Page::<Size4KiB>::containing_address(VirtAddr::new(ranges[0].0));
    let root = page_table_at_mut(
        paging.physical_memory_offset,
        address_space.level_4_frame.start_address().as_u64(),
    )
    .ok_or(MapRangeError::MappingFailed)?;
    let p3_frame = root[first_page.p4_index()]
        .frame()
        .map_err(|_| MapRangeError::MappingFailed)?;
    let p3 = page_table_at_mut(
        paging.physical_memory_offset,
        p3_frame.start_address().as_u64(),
    )
    .ok_or(MapRangeError::MappingFailed)?;
    let p2_frame = p3[first_page.p3_index()]
        .frame()
        .map_err(|_| MapRangeError::MappingFailed)?;
    let p2 = page_table_at_mut(
        paging.physical_memory_offset,
        p2_frame.start_address().as_u64(),
    )
    .ok_or(MapRangeError::MappingFailed)?;
    let p1_frame = p2[first_page.p2_index()]
        .frame()
        .map_err(|_| MapRangeError::MappingFailed)?;

    root[first_page.p4_index()].set_unused();
    for frame in [p1_frame, p2_frame, p3_frame, address_space.level_4_frame] {
        if !memory::deallocate_frame(frame) {
            return Err(MapRangeError::MappingFailed);
        }
    }
    Ok(())
}

pub fn user_range_is_unmapped_in(address_space: AddressSpace, start: u64, size: usize) -> bool {
    let Ok((start_page, end_page)) = page_range(start, size) else {
        return false;
    };
    let paging = PAGING.lock();
    let Ok(mapper) = mapper_for(&paging, address_space) else {
        return false;
    };
    Page::range_inclusive(start_page, end_page)
        .all(|page| mapper.translate_addr(page.start_address()).is_none())
}

pub fn user_range_has_protection_in(
    address_space: AddressSpace,
    start: u64,
    size: usize,
    writable: bool,
    executable: bool,
) -> bool {
    let Ok((start_page, end_page)) = page_range(start, size) else {
        return false;
    };
    let paging = PAGING.lock();
    let Ok(mapper) = mapper_for(&paging, address_space) else {
        return false;
    };
    Page::range_inclusive(start_page, end_page).all(|page| {
        let TranslateResult::Mapped { flags, .. } = mapper.translate(page.start_address()) else {
            return false;
        };
        flags.contains(PageTableFlags::PRESENT)
            && flags.contains(PageTableFlags::USER_ACCESSIBLE)
            && flags.contains(PageTableFlags::WRITABLE) == writable
            && flags.contains(PageTableFlags::NO_EXECUTE) != executable
    })
}

pub fn discard_empty_user_address_space(address_space: AddressSpace) -> bool {
    if Cr3::read().0 == address_space.level_4_frame {
        return false;
    }
    memory::deallocate_frame(address_space.level_4_frame)
}
pub fn switch_to(address_space: AddressSpace) {
    if Cr3::read().0 == address_space.level_4_frame {
        return;
    }
    unsafe { Cr3::write(address_space.level_4_frame, Cr3Flags::empty()) };
}

fn mapper_for(
    paging: &PagingState,
    address_space: AddressSpace,
) -> Result<OffsetPageTable<'static>, MapRangeError> {
    let table = page_table_at_mut(
        paging.physical_memory_offset,
        address_space.level_4_frame.start_address().as_u64(),
    )
    .ok_or(MapRangeError::MappingFailed)?;
    Ok(unsafe { OffsetPageTable::new(table, VirtAddr::new(paging.physical_memory_offset)) })
}

fn page_table_at(offset: u64, physical: u64) -> Option<&'static PageTable> {
    let address = offset.checked_add(physical)?;
    Some(unsafe { &*(address as *const PageTable) })
}

fn page_table_at_mut(offset: u64, physical: u64) -> Option<&'static mut PageTable> {
    let address = offset.checked_add(physical)?;
    Some(unsafe { &mut *(address as *mut PageTable) })
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UserMemoryError {
    AddressOverflow,
    NotMapped,
    PermissionDenied,
}

pub fn copy_from_current_user(start: u64, destination: &mut [u8]) -> Result<(), UserMemoryError> {
    copy_current_user(start, destination, false)
}

pub fn copy_to_current_user(start: u64, source: &[u8]) -> Result<(), UserMemoryError> {
    let mut copied = 0;
    while copied < source.len() {
        let paging = PAGING.lock();
        let address_space = AddressSpace {
            level_4_frame: Cr3::read().0,
        };
        let mapper = mapper_for(&paging, address_space).map_err(|_| UserMemoryError::NotMapped)?;
        let virtual_address = start
            .checked_add(copied as u64)
            .ok_or(UserMemoryError::AddressOverflow)?;
        let (physical_address, count, flags) =
            match translated_chunk(&mapper, virtual_address, source.len() - copied) {
                Ok(chunk) => chunk,
                Err(UserMemoryError::NotMapped) => {
                    drop(paging);
                    if crate::task::try_handle_file_fault(virtual_address, true) {
                        continue;
                    }
                    return Err(UserMemoryError::NotMapped);
                }
                Err(error) => return Err(error),
            };
        if !flags.contains(PageTableFlags::USER_ACCESSIBLE) {
            return Err(UserMemoryError::PermissionDenied);
        }
        if !flags.contains(PageTableFlags::WRITABLE) {
            // Fault resolution acquires PAGING itself. Re-translate after it
            // replaces a shared frame so writes use the private physical page.
            drop(paging);
            if crate::task::try_handle_cow_fault(virtual_address) {
                continue;
            }
            return Err(UserMemoryError::PermissionDenied);
        }
        let destination = paging
            .physical_memory_offset
            .checked_add(physical_address)
            .ok_or(UserMemoryError::AddressOverflow)? as *mut u8;
        unsafe {
            core::ptr::copy_nonoverlapping(source[copied..].as_ptr(), destination, count);
        }
        copied += count;
    }
    Ok(())
}

fn copy_current_user(
    start: u64,
    destination: &mut [u8],
    require_writable: bool,
) -> Result<(), UserMemoryError> {
    let mut copied = 0;
    while copied < destination.len() {
        let paging = PAGING.lock();
        let address_space = AddressSpace {
            level_4_frame: Cr3::read().0,
        };
        let mapper = mapper_for(&paging, address_space).map_err(|_| UserMemoryError::NotMapped)?;
        let virtual_address = start
            .checked_add(copied as u64)
            .ok_or(UserMemoryError::AddressOverflow)?;
        let (physical_address, count, flags) =
            match translated_chunk(&mapper, virtual_address, destination.len() - copied) {
                Ok(chunk) => chunk,
                Err(UserMemoryError::NotMapped) => {
                    drop(paging);
                    if crate::task::try_handle_file_fault(virtual_address, require_writable) {
                        continue;
                    }
                    return Err(UserMemoryError::NotMapped);
                }
                Err(error) => return Err(error),
            };
        if !flags.contains(PageTableFlags::USER_ACCESSIBLE)
            || (require_writable && !flags.contains(PageTableFlags::WRITABLE))
        {
            return Err(UserMemoryError::PermissionDenied);
        }
        let source = paging
            .physical_memory_offset
            .checked_add(physical_address)
            .ok_or(UserMemoryError::AddressOverflow)? as *const u8;
        unsafe {
            core::ptr::copy_nonoverlapping(source, destination[copied..].as_mut_ptr(), count);
        }
        copied += count;
    }
    Ok(())
}

fn translated_chunk(
    mapper: &OffsetPageTable<'static>,
    virtual_address: u64,
    remaining: usize,
) -> Result<(u64, usize, PageTableFlags), UserMemoryError> {
    match mapper.translate(VirtAddr::new(virtual_address)) {
        TranslateResult::Mapped {
            frame,
            offset,
            flags,
        } => {
            let available = usize::try_from(frame.size() - offset)
                .map_err(|_| UserMemoryError::AddressOverflow)?;
            let count = core::cmp::min(available, remaining);
            let physical = frame
                .start_address()
                .as_u64()
                .checked_add(offset)
                .ok_or(UserMemoryError::AddressOverflow)?;
            Ok((physical, count, flags))
        }
        TranslateResult::NotMapped | TranslateResult::InvalidFrameAddress(_) => {
            Err(UserMemoryError::NotMapped)
        }
    }
}
pub fn stats() -> Stats {
    PAGING.lock().stats()
}

/// Stage 8.4 shared-memory backing-page allocation. The object owns the
/// initial frame reference; each user mapping takes an additional reference
/// through the existing bounded frame-reference table.
pub fn allocate_shared_frame() -> Option<u64> {
    allocate_file_frame(&[0; 4096])
}

/// Stage 8.4 map one shared backing page into a user address space. Shared
/// memory is always NX; writable mappings are permitted only after the IPC
/// capability layer has checked WRITE authority.
pub fn map_shared_frame(space: AddressSpace, address: u64, physical: u64, writable: bool) -> bool {
    map_file_frame(space, address, physical, writable)
}

/// Drop the shared-memory object's ownership reference to a backing frame.
pub fn release_shared_frame(frame: u64) -> bool {
    release_file_frame(frame)
}

/// Frame-cache ownership: one reference is retained by the cache itself.
pub fn allocate_file_frame(bytes: &[u8; 4096]) -> Option<u64> {
    let frame = memory::allocate_frame()?;
    let physical = frame.start_address().as_u64();
    let paging = PAGING.lock();
    let destination = paging.physical_memory_offset.checked_add(physical)? as *mut u8;
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), destination, 4096);
    }
    Some(physical)
}

pub fn map_private_frame_from_bytes(
    space: AddressSpace,
    address: u64,
    bytes: &[u8; 4096],
    writable: bool,
) -> bool {
    let Some(frame) = memory::allocate_frame() else {
        return false;
    };
    let paging = PAGING.lock();
    let Ok(mut mapper) = mapper_for(&paging, space) else {
        let _ = memory::deallocate_frame(frame);
        return false;
    };
    let Ok(page) = Page::<Size4KiB>::from_start_address(VirtAddr::new(address)) else {
        let _ = memory::deallocate_frame(frame);
        return false;
    };
    if mapper.translate_addr(page.start_address()).is_some() {
        let _ = memory::deallocate_frame(frame);
        return false;
    }
    let Some(destination) = paging
        .physical_memory_offset
        .checked_add(frame.start_address().as_u64())
        .map(|address| address as *mut u8)
    else {
        let _ = memory::deallocate_frame(frame);
        return false;
    };
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), destination, 4096);
    }
    let mut allocator = memory::allocator();
    match unsafe { mapper.map_to(page, frame, user_flags(writable, false), &mut *allocator) } {
        Ok(flush) => {
            if Cr3::read().0 == space.level_4_frame {
                flush.flush();
                crate::smp::shootdown();
            } else {
                flush.ignore();
                crate::smp::shootdown();
            }
            true
        }
        Err(_) => {
            let _ = memory::deallocate_frame(frame);
            false
        }
    }
}
pub fn file_frame_references(frame: u64) -> u32 {
    COW_TABLE.lock().refcount(frame)
}
pub fn release_file_frame(frame: u64) -> bool {
    let Ok(frame) = PhysFrame::from_start_address(x86_64::PhysAddr::new(frame)) else {
        return false;
    };
    release_frame(frame)
}
pub fn read_file_frame(frame: u64, offset: usize, bytes: &mut [u8]) {
    assert!(offset <= 4096 && bytes.len() <= 4096 - offset);
    let paging = PAGING.lock();
    unsafe {
        core::ptr::copy_nonoverlapping(
            (paging.physical_memory_offset + frame + offset as u64) as *const u8,
            bytes.as_mut_ptr(),
            bytes.len(),
        );
    }
}
pub fn write_file_frame(frame: u64, offset: usize, bytes: &[u8]) {
    assert!(offset <= 4096 && bytes.len() <= 4096 - offset);
    let paging = PAGING.lock();
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (paging.physical_memory_offset + frame + offset as u64) as *mut u8,
            bytes.len(),
        );
    }
}
pub fn map_file_frame(space: AddressSpace, address: u64, physical: u64, writable: bool) -> bool {
    let paging = PAGING.lock();
    let Ok(mut mapper) = mapper_for(&paging, space) else {
        return false;
    };
    let Ok(page) = Page::<Size4KiB>::from_start_address(VirtAddr::new(address)) else {
        return false;
    };
    if mapper.translate_addr(page.start_address()).is_some() {
        return false;
    }
    let Ok(frame) = PhysFrame::from_start_address(x86_64::PhysAddr::new(physical)) else {
        return false;
    };
    let mut refs = COW_TABLE.lock();
    let mut allocator = memory::allocator();
    if refs.share(physical).is_err() {
        return false;
    }
    match unsafe { mapper.map_to(page, frame, user_flags(writable, false), &mut *allocator) } {
        Ok(flush) => {
            if Cr3::read().0 == space.level_4_frame {
                flush.flush();
                crate::smp::shootdown();
            } else {
                flush.ignore();
                crate::smp::shootdown();
            }
            true
        }
        Err(_) => {
            let _ = refs.release(physical);
            false
        }
    }
}

pub fn user_frame_in(space: AddressSpace, address: u64) -> Option<u64> {
    let paging = PAGING.lock();
    let mapper = mapper_for(&paging, space).ok()?;
    mapper
        .translate_addr(VirtAddr::new(address & !4095))
        .map(|frame| frame.as_u64())
}

/// Boot-only exhaustion test: run before scheduling, with no shared frames live.
pub fn frame_ownership_self_test() -> bool {
    const BASE: u64 = 0x4000_000e_0000;
    let baseline = memory::stats().allocated_frames;
    if COW_TABLE.lock().entries.iter().any(|entry| entry.occupied) {
        return false;
    }
    let Some(source) = create_user_address_space(BASE) else {
        return false;
    };
    let Some(destination) = create_user_address_space(BASE) else {
        return false;
    };
    if map_user_range_in(source, BASE, 4096, true, false).is_err()
        || write_user_bytes(source, BASE, &[0x71]).is_err()
    {
        return false;
    }
    let before_failure = memory::stats().allocated_frames;
    let overflow_rejected = {
        let mut table = COW_TABLE.lock();
        for (index, entry) in table.entries.iter_mut().enumerate() {
            *entry = CowEntry {
                frame: 0x1000_0000_0000 + (index as u64 * 4096),
                refcount: 2,
                occupied: true,
            };
        }
        let frame = table.entries[0].frame;
        table.entries[0].refcount = u32::MAX;
        table.share(frame) == Err(MapRangeError::OutOfFrames)
            && table.entries[0].refcount == u32::MAX
    };
    let failed_safely = share_user_range_in(source, destination, BASE, 4096, true, false)
        == Err(MapRangeError::OutOfFrames)
        && memory::stats().allocated_frames == before_failure
        && user_range_is_unmapped_in(destination, BASE, 4096);
    // Remove only the artificial entries installed above, before normal cleanup.
    for entry in COW_TABLE.lock().entries.iter_mut() {
        *entry = CowEntry::empty();
    }
    let mut byte = [0];
    let intact = read_user_bytes_in(source, BASE, &mut byte).is_ok()
        && byte == [0x71]
        && user_range_has_protection_in(source, BASE, 4096, true, false);
    let cleaned = discard_empty_user_address_space(destination)
        && destroy_user_address_space(source, &[(BASE, 4096)]).is_ok();
    overflow_rejected
        && failed_safely
        && intact
        && cleaned
        && memory::stats().allocated_frames == baseline
}

/// Map a device register page through the direct-map offset, uncached and NX.
pub fn map_mmio(physical: u64) -> Result<u64, MapRangeError> {
    let mut state = PAGING.lock();
    let virtual_address = 0xffff_fe00_0000_0000u64
        .checked_add(physical)
        .ok_or(MapRangeError::InvalidRange)?;
    let mapper = state.mapper.as_mut().ok_or(MapRangeError::NotInitialized)?;
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(virtual_address));
    let frame = PhysFrame::containing_address(x86_64::PhysAddr::new(physical));
    if mapper.translate_addr(page.start_address()).is_none() {
        let mut allocator = memory::allocator();
        unsafe {
            mapper.map_to(
                page,
                frame,
                PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::NO_CACHE
                    | PageTableFlags::NO_EXECUTE,
                &mut *allocator,
            )
        }
        .map_err(|_| MapRangeError::MappingFailed)?
        .flush();
    }
    Ok(virtual_address)
}

/// Replace a shared kernel test mapping, retiring its old frame only after all
/// online CPUs have invalidated cached translations. Used by the SMP regression.
pub fn replace_smp_test_page(address: u64, value: u64) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut state = PAGING.lock();
        let offset = state.physical_memory_offset;
        let mapper = state.mapper.as_mut().expect("paging initialized");
        let mut allocator = memory::allocator();
        let replacement = allocator.allocate_frame().expect("SMP replacement frame");
        unsafe {
            ((offset + replacement.start_address().as_u64()) as *mut u64).write_volatile(value);
        }
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
        let (old, flush) = mapper.unmap(page).expect("SMP test page mapped");
        flush.ignore();
        unsafe {
            mapper.map_to(
                page,
                replacement,
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE,
                &mut *allocator,
            )
        }
        .expect("SMP remap")
        .flush();
        crate::smp::shootdown();
        assert!(allocator.deallocate_frame(old));
    });
}
pub fn remove_smp_test_page(address: u64) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut state = PAGING.lock();
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
        let (frame, flush) = state
            .mapper
            .as_mut()
            .unwrap()
            .unmap(page)
            .expect("SMP cleanup");
        flush.flush();
        crate::smp::shootdown();
        assert!(memory::deallocate_frame(frame));
    });
}

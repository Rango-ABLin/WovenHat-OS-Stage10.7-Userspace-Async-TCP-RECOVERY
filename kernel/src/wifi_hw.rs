//! WovenWiFi Stage 13.10A â€” physical PCI Wi-Fi backend boundary.
//!
//! Establishes the hardware-facing ownership boundary without claiming support
//! for a specific chipset yet. A valid candidate must be a PCI network
//! controller in the wireless/other subclass, expose an MMIO BAR, and be
//! prepared for memory-space decoding + DMA bus mastering.
//!
//! Register layouts, firmware protocols, DMA ring formats, interrupts, channel
//! control, and RF behavior remain chipset-specific work for later stages.

use core::sync::atomic::{fence, Ordering};

use x86_64::structures::paging::{PhysFrame, Size4KiB};

use crate::{hal::pci, memory, paging};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HwBindError {
    NotWireless,
    NoMmioBar,
    ConfigWriteFailed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct WifiPciFunction {
    pub address: pci::Address,
    pub vendor_id: u16,
    pub device_id: u16,
    pub revision: u8,
    pub mmio_bar: u8,
    pub mmio_base: u64,
    pub msix: bool,
    pub msi: bool,
    pub pcie: bool,
}

pub const fn is_wireless_candidate(device: &pci::Device) -> bool {
    device.class == 0x02 && device.subclass == 0x80
}

fn first_mmio_bar(device: &pci::Device) -> Option<(u8, u64)> {
    let mut index = 0usize;
    while index < device.bars.len() {
        let bar = device.bars[index];
        if bar.valid
            && matches!(bar.kind, pci::BarKind::Memory32 | pci::BarKind::Memory64)
            && bar.address != 0
        {
            return Some((index as u8, bar.address));
        }
        index += 1;
    }
    None
}

pub fn descriptor_from_device(device: pci::Device) -> Result<WifiPciFunction, HwBindError> {
    if !is_wireless_candidate(&device) {
        return Err(HwBindError::NotWireless);
    }
    let Some((bar, base)) = first_mmio_bar(&device) else {
        return Err(HwBindError::NoMmioBar);
    };
    Ok(WifiPciFunction {
        address: pci::Address {
            segment: device.segment,
            bus: device.bus,
            device: device.device,
            function: device.function,
        },
        vendor_id: device.vendor_id,
        device_id: device.device_id,
        revision: device.revision,
        mmio_bar: bar,
        mmio_base: base,
        msix: device.capabilities.msix,
        msi: device.capabilities.msi,
        pcie: device.capabilities.pcie,
    })
}

/// Prepare a discovered PCI Wi-Fi function for a later chipset driver.
/// PCI memory decoding and DMA bus mastering are enabled only after the device
/// has passed the class and MMIO validation boundary above.
pub fn bind_pci_function(device: pci::Device) -> Result<WifiPciFunction, HwBindError> {
    let descriptor = descriptor_from_device(device)?;
    if !pci::enable_memory_bus_master(descriptor.address) {
        return Err(HwBindError::ConfigWriteFailed);
    }
    Ok(descriptor)
}

pub fn discover_first() -> Option<WifiPciFunction> {
    for index in 0..64 {
        let Some(device) = pci::device(index) else { continue };
        if is_wireless_candidate(&device) {
            if let Ok(bound) = bind_pci_function(device) {
                return Some(bound);
            }
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmioError {
    Unaligned,
    OutOfRange,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MmioRegion {
    base: u64,
    span: usize,
}

impl MmioRegion {
    pub const fn new(base: u64, span: usize) -> Result<Self, MmioError> {
        if base & 3 != 0 {
            return Err(MmioError::Unaligned);
        }
        if span < 4 || span & 3 != 0 {
            return Err(MmioError::OutOfRange);
        }
        Ok(Self { base, span })
    }

    pub const fn base(&self) -> u64 { self.base }
    pub const fn span(&self) -> usize { self.span }

    pub fn register_address(&self, offset: usize) -> Result<u64, MmioError> {
        if offset & 3 != 0 || offset.checked_add(4).is_none_or(|end| end > self.span) {
            return Err(MmioError::OutOfRange);
        }
        self.base
            .checked_add(offset as u64)
            .ok_or(MmioError::OutOfRange)
    }
}

pub const DMA_RING_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaError {
    Full,
    Empty,
    InvalidLength,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct DmaDescriptor {
    pub physical_address: u64,
    pub length: u16,
    pub flags: u16,
}

pub struct DmaRing {
    entries: [DmaDescriptor; DMA_RING_CAPACITY],
    producer: usize,
    consumer: usize,
    count: usize,
}

impl DmaRing {
    pub const fn new() -> Self {
        Self {
            entries: [DmaDescriptor {
                physical_address: 0,
                length: 0,
                flags: 0,
            }; DMA_RING_CAPACITY],
            producer: 0,
            consumer: 0,
            count: 0,
        }
    }

    pub const fn len(&self) -> usize { self.count }
    pub const fn is_empty(&self) -> bool { self.count == 0 }

    pub fn push(&mut self, descriptor: DmaDescriptor) -> Result<(), DmaError> {
        if descriptor.length == 0 {
            return Err(DmaError::InvalidLength);
        }
        if self.count == DMA_RING_CAPACITY {
            return Err(DmaError::Full);
        }
        self.entries[self.producer] = descriptor;
        self.producer = (self.producer + 1) % DMA_RING_CAPACITY;
        self.count += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Result<DmaDescriptor, DmaError> {
        if self.count == 0 {
            return Err(DmaError::Empty);
        }
        let descriptor = self.entries[self.consumer];
        self.entries[self.consumer] = DmaDescriptor::default();
        self.consumer = (self.consumer + 1) % DMA_RING_CAPACITY;
        self.count -= 1;
        Ok(descriptor)
    }

    pub fn reset(&mut self) {
        self.entries = [DmaDescriptor::default(); DMA_RING_CAPACITY];
        self.producer = 0;
        self.consumer = 0;
        self.count = 0;
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct InterruptState {
    pub pending: u32,
    pub handled: u64,
}

impl InterruptState {
    pub fn raise(&mut self, causes: u32) {
        self.pending |= causes;
    }

    pub fn take(&mut self) -> u32 {
        let causes = self.pending;
        if causes != 0 {
            self.pending = 0;
            self.handled = self.handled.saturating_add(1);
        }
        causes
    }

    pub fn reset(&mut self) {
        self.pending = 0;
    }
}

pub struct HardwareQueues {
    pub tx: DmaRing,
    pub rx: DmaRing,
    pub interrupts: InterruptState,
}

impl HardwareQueues {
    pub const fn new() -> Self {
        Self {
            tx: DmaRing::new(),
            rx: DmaRing::new(),
            interrupts: InterruptState {
                pending: 0,
                handled: 0,
            },
        }
    }

    pub fn quiesce(&mut self) {
        self.tx.reset();
        self.rx.reset();
        self.interrupts.reset();
    }
}

pub const DMA_PAGE_SIZE: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaMemoryError {
    OutOfFrames,
    DirectMapUnavailable,
    AddressOverflow,
}

/// One kernel-owned 4 KiB frame suitable for a device DMA buffer.
///
/// The frame remains exclusively owned by this value and is returned to the
/// physical allocator on Drop. The virtual address is the bootloader direct
/// mapping of the same physical frame, so CPU and device refer to one backing
/// allocation without copying.
pub struct DmaPage {
    frame: PhysFrame<Size4KiB>,
    physical_address: u64,
    virtual_address: u64,
}

impl DmaPage {
    pub fn allocate_zeroed() -> Result<Self, DmaMemoryError> {
        let frame = memory::allocate_frame().ok_or(DmaMemoryError::OutOfFrames)?;
        let physical_address = frame.start_address().as_u64();
        let Some(offset) = paging::physical_memory_offset() else {
            let _ = memory::deallocate_frame(frame);
            return Err(DmaMemoryError::DirectMapUnavailable);
        };
        let Some(virtual_address) = offset.checked_add(physical_address) else {
            let _ = memory::deallocate_frame(frame);
            return Err(DmaMemoryError::AddressOverflow);
        };

        // SAFETY: `frame` is exclusively owned by this DmaPage and the
        // bootloader direct map makes the complete 4 KiB frame writable at
        // `virtual_address`.
        unsafe {
            core::ptr::write_bytes(virtual_address as *mut u8, 0, DMA_PAGE_SIZE);
        }
        Ok(Self {
            frame,
            physical_address,
            virtual_address,
        })
    }

    pub const fn physical_address(&self) -> u64 { self.physical_address }
    pub const fn virtual_address(&self) -> u64 { self.virtual_address }
    pub const fn len(&self) -> usize { DMA_PAGE_SIZE }
    pub const fn is_empty(&self) -> bool { false }

    pub fn write_u32(&mut self, offset: usize, value: u32) -> Result<(), DmaMemoryError> {
        if offset & 3 != 0 || offset.checked_add(4).is_none_or(|end| end > DMA_PAGE_SIZE) {
            return Err(DmaMemoryError::AddressOverflow);
        }
        // SAFETY: Bounds/alignment were checked and this DmaPage uniquely owns
        // the backing frame for the duration of the write.
        unsafe {
            ((self.virtual_address + offset as u64) as *mut u32).write_volatile(value);
        }
        Ok(())
    }

    pub fn read_u32(&self, offset: usize) -> Result<u32, DmaMemoryError> {
        if offset & 3 != 0 || offset.checked_add(4).is_none_or(|end| end > DMA_PAGE_SIZE) {
            return Err(DmaMemoryError::AddressOverflow);
        }
        // SAFETY: Bounds/alignment were checked and the direct mapping remains
        // live while self owns the physical frame.
        Ok(unsafe {
            ((self.virtual_address + offset as u64) as *const u32).read_volatile()
        })
    }
}

impl Drop for DmaPage {
    fn drop(&mut self) {
        // The allocator rejects duplicate returns; ownership of `frame` is
        // confined to this value, so a normal Drop returns it exactly once.
        let _ = memory::deallocate_frame(self.frame);
    }
}

/// Publish CPU writes before handing DMA ownership to a device.
pub fn dma_publish() {
    fence(Ordering::Release);
}

/// Observe device DMA writes before the CPU consumes a completed descriptor.
pub fn dma_consume() {
    fence(Ordering::Acquire);
}

/// Audited volatile 32-bit register access over an already mapped MMIO window.
///
/// Construction is unsafe because the caller must prove that `virtual_base`
/// denotes a live device mapping for the complete span. Offset validation and
/// all volatile pointer operations are then contained here.
pub struct VolatileMmio32 {
    region: MmioRegion,
    virtual_base: u64,
}

impl VolatileMmio32 {
    /// # Safety
    ///
    /// `virtual_base..virtual_base + region.span()` must be a valid, writable
    /// kernel mapping of the device register window for the lifetime of this
    /// value. No ordinary RAM alias may be concurrently treated as Rust data.
    pub const unsafe fn from_mapped(region: MmioRegion, virtual_base: u64) -> Self {
        Self { region, virtual_base }
    }

    pub fn read(&self, offset: usize) -> Result<u32, MmioError> {
        let _ = self.region.register_address(offset)?;
        let virtual_address = self
            .virtual_base
            .checked_add(offset as u64)
            .ok_or(MmioError::OutOfRange)?;
        // SAFETY: Constructor contract establishes a live MMIO mapping and
        // register_address validated this aligned 32-bit access.
        Ok(unsafe { (virtual_address as *const u32).read_volatile() })
    }

    pub fn write(&mut self, offset: usize, value: u32) -> Result<(), MmioError> {
        let _ = self.region.register_address(offset)?;
        let virtual_address = self
            .virtual_base
            .checked_add(offset as u64)
            .ok_or(MmioError::OutOfRange)?;
        // SAFETY: Constructor contract establishes a live writable MMIO
        // mapping and register_address validated this aligned 32-bit access.
        unsafe { (virtual_address as *mut u32).write_volatile(value) };
        Ok(())
    }
}

/// Map one physical device register page uncached/NX and return the isolated
/// volatile accessor. Stage 13.10C deliberately limits a binding to one page;
/// larger chipset BARs will be mapped page-by-page by the concrete driver.
pub fn map_mmio_page(physical: u64) -> Result<VolatileMmio32, MmioError> {
    if physical & (DMA_PAGE_SIZE as u64 - 1) != 0 {
        return Err(MmioError::Unaligned);
    }
    let virtual_base = paging::map_mmio(physical).map_err(|_| MmioError::OutOfRange)?;
    let region = MmioRegion::new(physical, DMA_PAGE_SIZE)?;
    // SAFETY: paging::map_mmio created a writable, uncached, NX kernel mapping
    // of the physical register page represented by `region`.
    Ok(unsafe { VolatileMmio32::from_mapped(region, virtual_base) })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorOwner {
    Cpu,
    Device,
    Completed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueError {
    Full,
    Empty,
    InvalidLength,
    NotCpuOwned,
    NotDeviceOwned,
    NotCompleted,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct OwnedDescriptor {
    descriptor: DmaDescriptor,
    owner: DescriptorOwner,
}

impl OwnedDescriptor {
    const fn empty() -> Self {
        Self {
            descriptor: DmaDescriptor {
                physical_address: 0,
                length: 0,
                flags: 0,
            },
            owner: DescriptorOwner::Cpu,
        }
    }
}

/// Fixed-capacity DMA descriptor lifecycle.
///
/// CPU -> Device is a publish operation: descriptor fields are fully written
/// before the ownership transition. Device -> Completed is represented by the
/// interrupt/backend side. Completed -> CPU is the only legal reclaim path,
/// preventing a buffer from being reused while hardware still owns it.
pub struct DeviceQueue {
    entries: [OwnedDescriptor; DMA_RING_CAPACITY],
    head: usize,
    tail: usize,
    count: usize,
    device_owned: usize,
    completed: usize,
}

impl DeviceQueue {
    pub const fn new() -> Self {
        Self {
            entries: [OwnedDescriptor::empty(); DMA_RING_CAPACITY],
            head: 0,
            tail: 0,
            count: 0,
            device_owned: 0,
            completed: 0,
        }
    }

    pub const fn len(&self) -> usize { self.count }
    pub const fn is_empty(&self) -> bool { self.count == 0 }
    pub const fn device_owned(&self) -> usize { self.device_owned }
    pub const fn completed(&self) -> usize { self.completed }

    pub fn submit(&mut self, descriptor: DmaDescriptor) -> Result<usize, QueueError> {
        if descriptor.length == 0 {
            return Err(QueueError::InvalidLength);
        }
        if self.count == DMA_RING_CAPACITY {
            return Err(QueueError::Full);
        }
        if self.entries[self.tail].owner != DescriptorOwner::Cpu {
            return Err(QueueError::NotCpuOwned);
        }
        let slot = self.tail;
        self.entries[slot].descriptor = descriptor;
        dma_publish();
        self.entries[slot].owner = DescriptorOwner::Device;
        self.tail = (self.tail + 1) % DMA_RING_CAPACITY;
        self.count += 1;
        self.device_owned += 1;
        Ok(slot)
    }

    /// Model the device/interrupt side completing one descriptor. A concrete
    /// chipset ISR will call the equivalent transition only after reading the
    /// hardware completion state.
    pub fn complete(&mut self, slot: usize) -> Result<(), QueueError> {
        if slot >= DMA_RING_CAPACITY || self.entries[slot].owner != DescriptorOwner::Device {
            return Err(QueueError::NotDeviceOwned);
        }
        dma_consume();
        self.entries[slot].owner = DescriptorOwner::Completed;
        self.device_owned -= 1;
        self.completed += 1;
        Ok(())
    }

    pub fn reclaim(&mut self) -> Result<DmaDescriptor, QueueError> {
        if self.count == 0 {
            return Err(QueueError::Empty);
        }
        if self.entries[self.head].owner != DescriptorOwner::Completed {
            return Err(QueueError::NotCompleted);
        }
        dma_consume();
        let descriptor = self.entries[self.head].descriptor;
        self.entries[self.head] = OwnedDescriptor::empty();
        self.head = (self.head + 1) % DMA_RING_CAPACITY;
        self.count -= 1;
        self.completed -= 1;
        Ok(descriptor)
    }

    /// Reset is legal only after hardware has been stopped and no descriptor
    /// remains device-owned. This prevents teardown from freeing DMA memory
    /// while a bus master may still access it.
    pub fn quiesce(&mut self) -> Result<(), QueueError> {
        if self.device_owned != 0 {
            return Err(QueueError::NotCpuOwned);
        }
        self.entries = [OwnedDescriptor::empty(); DMA_RING_CAPACITY];
        self.head = 0;
        self.tail = 0;
        self.count = 0;
        self.completed = 0;
        Ok(())
    }
}

pub fn stage13_10d_self_test() -> bool {
    let mut queue = DeviceQueue::new();
    let first = DmaDescriptor {
        physical_address: 0x0040_0000,
        length: 1536,
        flags: 0x11,
    };
    let second = DmaDescriptor {
        physical_address: 0x0041_0000,
        length: 2048,
        flags: 0x22,
    };

    let Ok(first_slot) = queue.submit(first) else { return false };
    let Ok(second_slot) = queue.submit(second) else { return false };
    if first_slot != 0
        || second_slot != 1
        || queue.len() != 2
        || queue.device_owned() != 2
        || queue.completed() != 0
        || queue.reclaim() != Err(QueueError::NotCompleted)
        || queue.quiesce() != Err(QueueError::NotCpuOwned)
    {
        return false;
    }

    // Complete out of order. Reclaim must still respect queue head ordering.
    if queue.complete(second_slot).is_err()
        || queue.completed() != 1
        || queue.reclaim() != Err(QueueError::NotCompleted)
        || queue.complete(first_slot).is_err()
        || queue.complete(first_slot) != Err(QueueError::NotDeviceOwned)
        || queue.device_owned() != 0
        || queue.completed() != 2
        || queue.reclaim() != Ok(first)
        || queue.reclaim() != Ok(second)
        || !queue.is_empty()
    {
        return false;
    }

    // Exercise ring wraparound and prove a device-owned slot cannot be reused.
    for index in 0..DMA_RING_CAPACITY {
        let descriptor = DmaDescriptor {
            physical_address: 0x0080_0000 + index as u64 * DMA_PAGE_SIZE as u64,
            length: 512,
            flags: index as u16,
        };
        let Ok(slot) = queue.submit(descriptor) else { return false };
        if queue.complete(slot).is_err() || queue.reclaim() != Ok(descriptor) {
            return false;
        }
    }

    queue.is_empty()
        && queue.device_owned() == 0
        && queue.completed() == 0
        && queue.quiesce().is_ok()
}
pub fn stage13_10c_self_test() -> bool {
    let before = memory::stats().allocated_frames;
    {
        let Ok(mut page) = DmaPage::allocate_zeroed() else { return false };
        if page.physical_address() & (DMA_PAGE_SIZE as u64 - 1) != 0
            || page.virtual_address() == 0
            || page.len() != DMA_PAGE_SIZE
            || page.is_empty()
            || page.read_u32(0) != Ok(0)
            || page.write_u32(0, 0x5748_444d).is_err()
        {
            return false;
        }
        dma_publish();
        dma_consume();
        if page.read_u32(0) != Ok(0x5748_444d)
            || page.write_u32(2, 1).is_ok()
            || page.read_u32(DMA_PAGE_SIZE).is_ok()
        {
            return false;
        }

        // Exercise the same audited volatile access boundary against the
        // owned test frame. This validates pointer containment without
        // touching a nonexistent physical Wi-Fi device in QEMU.
        let Ok(region) = MmioRegion::new(page.physical_address(), DMA_PAGE_SIZE) else {
            return false;
        };
        // SAFETY: For this self-test the DmaPage is live, writable and
        // exclusively owned for the complete synthetic register window.
        let mut registers = unsafe {
            VolatileMmio32::from_mapped(region, page.virtual_address())
        };
        if registers.write(4, 0xa5a5_5a5a).is_err()
            || registers.read(4) != Ok(0xa5a5_5a5a)
            || registers.read(2).is_ok()
            || registers.write(DMA_PAGE_SIZE, 0).is_ok()
        {
            return false;
        }
    }
    memory::stats().allocated_frames == before
}
pub fn stage13_10b_self_test() -> bool {
    let Ok(mmio) = MmioRegion::new(0xfebc_0000, 0x1000) else { return false };
    if mmio.register_address(0) != Ok(0xfebc_0000)
        || mmio.register_address(0x0ffc) != Ok(0xfebc_0ffc)
        || mmio.register_address(2).is_ok()
        || mmio.register_address(0x1000).is_ok()
    {
        return false;
    }

    let mut queues = HardwareQueues::new();
    let tx = DmaDescriptor {
        physical_address: 0x0020_0000,
        length: 1500,
        flags: 1,
    };
    let rx = DmaDescriptor {
        physical_address: 0x0021_0000,
        length: 1600,
        flags: 2,
    };

    if queues.tx.push(tx).is_err()
        || queues.rx.push(rx).is_err()
        || queues.tx.len() != 1
        || queues.rx.len() != 1
    {
        return false;
    }

    queues.interrupts.raise(0x1);
    queues.interrupts.raise(0x4);
    if queues.interrupts.take() != 0x5
        || queues.interrupts.pending != 0
        || queues.interrupts.handled != 1
    {
        return false;
    }

    if queues.tx.pop() != Ok(tx) || queues.rx.pop() != Ok(rx) {
        return false;
    }

    for index in 0..DMA_RING_CAPACITY {
        let descriptor = DmaDescriptor {
            physical_address: 0x0030_0000 + (index as u64 * 0x1000),
            length: 512,
            flags: 0,
        };
        if queues.tx.push(descriptor).is_err() {
            return false;
        }
    }
    if queues.tx.push(tx) != Err(DmaError::Full) {
        return false;
    }

    queues.interrupts.raise(0xffff);
    queues.quiesce();
    queues.tx.is_empty()
        && queues.rx.is_empty()
        && queues.interrupts.pending == 0
        && queues.interrupts.handled == 1
}
pub fn self_test() -> bool {
    let mut good = pci::Device {
        segment: 0,
        bus: 2,
        device: 3,
        function: 0,
        vendor_id: 0x8086,
        device_id: 0x2723,
        revision: 1,
        class: 0x02,
        subclass: 0x80,
        ..pci::Device::default()
    };
    good.bars[0] = pci::Bar {
        valid: true,
        kind: pci::BarKind::Memory64,
        address: 0xfebc_0000,
        prefetchable: false,
    };
    good.capabilities.msi = true;
    good.capabilities.pcie = true;
    let Ok(desc) = descriptor_from_device(good) else { return false };
    if desc.vendor_id != 0x8086
        || desc.device_id != 0x2723
        || desc.mmio_bar != 0
        || desc.mmio_base != 0xfebc_0000
        || !desc.msi
        || !desc.pcie
    {
        return false;
    }

    let mut wired = good;
    wired.subclass = 0x00;
    if descriptor_from_device(wired) != Err(HwBindError::NotWireless) {
        return false;
    }

    let mut no_bar = good;
    no_bar.bars = [pci::Bar::default(); 6];
    descriptor_from_device(no_bar) == Err(HwBindError::NoMmioBar)
}
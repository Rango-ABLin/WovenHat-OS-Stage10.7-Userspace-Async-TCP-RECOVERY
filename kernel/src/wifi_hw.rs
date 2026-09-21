//! WovenWiFi Stage 13.10A â€” physical PCI Wi-Fi backend boundary.
//!
//! Establishes the hardware-facing ownership boundary without claiming support
//! for a specific chipset yet. A valid candidate must be a PCI network
//! controller in the wireless/other subclass, expose an MMIO BAR, and be
//! prepared for memory-space decoding + DMA bus mastering.
//!
//! Register layouts, firmware protocols, DMA ring formats, interrupts, channel
//! control, and RF behavior remain chipset-specific work for later stages.

use crate::hal::pci;

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
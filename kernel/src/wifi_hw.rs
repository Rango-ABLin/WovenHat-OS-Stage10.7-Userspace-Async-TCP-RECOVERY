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
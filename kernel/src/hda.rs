use core::ptr;

use crate::{
    hal::pci::{self, Address, BarKind},
    irq_lock::IrqMutex as Mutex,
    paging,
};

const AUDIO_CLASS: u8 = 0x04;
const HDA_SUBCLASS: u8 = 0x03;
const POLL_LIMIT: usize = 1_000_000;

static CONTROLLER: Mutex<Option<HdaController>> = Mutex::with_rank(None, 10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    MissingController,
    MissingMemoryBar,
    MmioUnavailable,
    PciEnableFailed,
    InvalidCapabilities,
    ResetTimeout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControllerSummary {
    pub vendor_id: u16,
    pub device_id: u16,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub input_streams: u8,
    pub output_streams: u8,
    pub bidirectional_streams: u8,
    pub supports_64bit: bool,
}

pub struct HdaController {
    pci: pci::Device,
    mmio: u64,
    input_streams: u8,
    output_streams: u8,
    bidirectional_streams: u8,
    supports_64bit: bool,
}

impl HdaController {
    fn initialize(device: pci::Device) -> Result<Self, InitError> {
        let bar = device
            .bars
            .iter()
            .find(|bar| bar.valid && matches!(bar.kind, BarKind::Memory32 | BarKind::Memory64))
            .ok_or(InitError::MissingMemoryBar)?;

        let address = Address {
            segment: device.segment,
            bus: device.bus,
            device: device.device,
            function: device.function,
        };
        if !pci::enable_memory_bus_master(address) {
            return Err(InitError::PciEnableFailed);
        }

        let gcap = mmio_read16(bar.address)?;
        let output_streams = ((gcap >> 12) & 0x0f) as u8;
        let input_streams = ((gcap >> 8) & 0x0f) as u8;
        let bidirectional_streams = ((gcap >> 3) & 0x1f) as u8;
        let supports_64bit = gcap & 1 != 0;
        if output_streams == 0 && input_streams == 0 && bidirectional_streams == 0 {
            return Err(InitError::InvalidCapabilities);
        }

        let mut controller = Self {
            pci: device,
            mmio: bar.address,
            input_streams,
            output_streams,
            bidirectional_streams,
            supports_64bit,
        };
        controller.reset()?;
        Ok(controller)
    }

    fn reset(&mut self) -> Result<(), InitError> {
        let gctl = self.mmio + 0x08;
        let current = mmio_read32(gctl)?;
        mmio_write32(gctl, current & !1)?;
        wait_for_bit(gctl, 1, false)?;
        mmio_write32(gctl, current | 1)?;
        wait_for_bit(gctl, 1, true)?;
        Ok(())
    }

    pub fn summary(&self) -> ControllerSummary {
        ControllerSummary {
            vendor_id: self.pci.vendor_id,
            device_id: self.pci.device_id,
            bus: self.pci.bus,
            device: self.pci.device,
            function: self.pci.function,
            input_streams: self.input_streams,
            output_streams: self.output_streams,
            bidirectional_streams: self.bidirectional_streams,
            supports_64bit: self.supports_64bit,
        }
    }
}

pub fn probe() -> bool {
    find_controller().is_some()
}

pub fn init() -> Result<ControllerSummary, InitError> {
    let mut slot = CONTROLLER.lock();
    if let Some(controller) = slot.as_ref() {
        return Ok(controller.summary());
    }
    let device = find_controller().ok_or(InitError::MissingController)?;
    let controller = HdaController::initialize(device)?;
    let summary = controller.summary();
    *slot = Some(controller);
    Ok(summary)
}

fn find_controller() -> Option<pci::Device> {
    for index in 0..64 {
        if let Some(device) = pci::device(index) {
            if device.class == AUDIO_CLASS && device.subclass == HDA_SUBCLASS {
                return Some(device);
            }
        }
    }
    None
}

fn wait_for_bit(address: u64, mask: u32, set: bool) -> Result<(), InitError> {
    for _ in 0..POLL_LIMIT {
        let value = mmio_read32(address)?;
        if (value & mask != 0) == set {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(InitError::ResetTimeout)
}

fn mmio_read16(physical: u64) -> Result<u16, InitError> {
    let page = physical & !0xfff;
    let offset = physical & 0xfff;
    let mapped = paging::map_mmio(page).map_err(|_| InitError::MmioUnavailable)?;
    Ok(unsafe { ptr::read_volatile((mapped + offset) as *const u16) })
}

fn mmio_read32(physical: u64) -> Result<u32, InitError> {
    let page = physical & !0xfff;
    let offset = physical & 0xfff;
    let mapped = paging::map_mmio(page).map_err(|_| InitError::MmioUnavailable)?;
    Ok(unsafe { ptr::read_volatile((mapped + offset) as *const u32) })
}

fn mmio_write32(physical: u64, value: u32) -> Result<(), InitError> {
    let page = physical & !0xfff;
    let offset = physical & 0xfff;
    let mapped = paging::map_mmio(page).map_err(|_| InitError::MmioUnavailable)?;
    unsafe { ptr::write_volatile((mapped + offset) as *mut u32, value) };
    Ok(())
}

pub fn self_test() -> bool {
    let gcap: u16 = (4 << 12) | (2 << 8) | (1 << 3) | 1;
    let output = ((gcap >> 12) & 0x0f) as u8;
    let input = ((gcap >> 8) & 0x0f) as u8;
    let bidirectional = ((gcap >> 3) & 0x1f) as u8;
    output == 4 && input == 2 && bidirectional == 1 && (gcap & 1 != 0)
}

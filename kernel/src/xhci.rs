use core::{ptr, sync::atomic::{fence, Ordering}};

use crate::{hal::pci::{self, Address, BarKind}, irq_lock::IrqMutex as Mutex, memory, paging};

const USB_CLASS: u8 = 0x0c;
const USB_SUBCLASS: u8 = 0x03;
const XHCI_PROG_IF: u8 = 0x30;
const PAGE_SIZE: usize = 4096;
const POLL_LIMIT: usize = 4_000_000;
const TRB_TYPE_ENABLE_SLOT: u32 = 9;
const TRB_TYPE_COMMAND_COMPLETION: u32 = 33;
const TRB_TYPE_PORT_STATUS_CHANGE: u32 = 34;

static CONTROLLER: Mutex<Option<XhciController>> = Mutex::with_rank(None, 10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    MissingController,
    MissingMemoryBar,
    DmaUnavailable,
    MmioUnavailable,
    ControllerTimeout,
    CommandFailed,
    NoConnectedPort,
    InvalidRegisters,
}

#[derive(Clone, Copy)]
struct DmaPage {
    physical: u64,
    virtual_address: u64,
}

impl DmaPage {
    fn allocate_zeroed() -> Option<Self> {
        let frame = memory::allocate_contiguous_frames(1)?;
        let physical = frame.start_address().as_u64();
        let offset = paging::physical_memory_offset()?;
        let virtual_address = offset.checked_add(physical)?;
        // SAFETY: the allocated frame is exclusively owned by this DMA page.
        unsafe { ptr::write_bytes(virtual_address as *mut u8, 0, PAGE_SIZE) };
        Some(Self { physical, virtual_address })
    }

    fn bytes_mut(&mut self) -> &mut [u8; PAGE_SIZE] {
        // SAFETY: all access is serialized by the controller mutex.
        unsafe { &mut *(self.virtual_address as *mut [u8; PAGE_SIZE]) }
    }
}

#[derive(Clone, Copy, Default)]
struct Trb {
    parameter: u64,
    status: u32,
    control: u32,
}

struct Ring {
    page: DmaPage,
    index: usize,
    cycle: bool,
}

impl Ring {
    fn new(mut page: DmaPage) -> Self {
        page.bytes_mut().fill(0);
        let mut ring = Self { page, index: 0, cycle: true };
        ring.install_link();
        ring
    }

    fn install_link(&mut self) {
        let slot = (PAGE_SIZE / 16) - 1;
        let mut trb = Trb { parameter: self.page.physical, status: 0, control: (6 << 10) | (1 << 1) };
        if self.cycle { trb.control |= 1; }
        self.write(slot, trb);
    }

    fn push(&mut self, mut trb: Trb) -> u64 {
        if self.index == (PAGE_SIZE / 16) - 1 {
            self.index = 0;
            self.cycle = !self.cycle;
            self.install_link();
        }
        if self.cycle { trb.control |= 1; } else { trb.control &= !1; }
        let index = self.index;
        self.write(index, trb);
        fence(Ordering::Release);
        self.index += 1;
        self.page.physical + (index as u64 * 16)
    }

    fn write(&mut self, index: usize, trb: Trb) {
        let offset = index * 16;
        let bytes = self.page.bytes_mut();
        bytes[offset..offset+8].copy_from_slice(&trb.parameter.to_le_bytes());
        bytes[offset+8..offset+12].copy_from_slice(&trb.status.to_le_bytes());
        bytes[offset+12..offset+16].copy_from_slice(&trb.control.to_le_bytes());
    }
}

pub struct XhciController {
    mmio: u64,
    cap_length: u8,
    max_slots: u8,
    max_ports: u8,
    op_base: u64,
    runtime_base: u64,
    doorbell_base: u64,
    dcbaa: DmaPage,
    command_ring: Ring,
    event_ring: DmaPage,
    erst: DmaPage,
    event_index: usize,
    event_cycle: bool,
    connected_port: u8,
    slot_id: u8,
}

impl XhciController {
    fn initialize(device: pci::Device) -> Result<Self, InitError> {
        let bar = device.bars.iter().find(|bar| bar.valid && matches!(bar.kind, BarKind::Memory32 | BarKind::Memory64)).ok_or(InitError::MissingMemoryBar)?;
        let address = Address { segment: device.segment, bus: device.bus, device: device.device, function: device.function };
        if !pci::enable_memory_bus_master(address) { return Err(InitError::MissingController); }
        let mmio = bar.address;
        let cap_length = mmio_read8(mmio)?;
        if cap_length < 0x20 { return Err(InitError::InvalidRegisters); }
        let hcs1 = mmio_read32(mmio + 0x04)?;
        let max_slots = (hcs1 & 0xff) as u8;
        let max_ports = ((hcs1 >> 24) & 0xff) as u8;
        if max_slots == 0 || max_ports == 0 { return Err(InitError::InvalidRegisters); }
        let runtime_base = mmio + u64::from(mmio_read32(mmio + 0x18)? & !0x1f);
        let doorbell_base = mmio + u64::from(mmio_read32(mmio + 0x14)? & !0x3);
        let op_base = mmio + u64::from(cap_length);

        let command = mmio_read32(op_base)?;
        mmio_write32(op_base, command & !1)?;
        wait_until(op_base + 0x04, 1, 1)?;
        mmio_write32(op_base, (command & !1) | (1 << 1))?;
        wait_until(op_base, 1 << 1, 0)?;

        let mut dcbaa = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        let command_page = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        let mut event_ring = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        let mut erst = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        dcbaa.bytes_mut().fill(0);
        event_ring.bytes_mut().fill(0);
        erst.bytes_mut().fill(0);
        let command_ring = Ring::new(command_page);

        mmio_write64(op_base + 0x30, dcbaa.physical)?;
        mmio_write64(op_base + 0x18, command_ring.page.physical | 1)?;
        mmio_write32(op_base + 0x38, u32::from(max_slots))?;

        let interrupter = runtime_base + 0x20;
        mmio_write32(interrupter + 0x08, 1)?;
        let erst_bytes = erst.bytes_mut();
        erst_bytes[0..8].copy_from_slice(&event_ring.physical.to_le_bytes());
        erst_bytes[8..12].copy_from_slice(&((PAGE_SIZE / 16) as u32).to_le_bytes());
        mmio_write64(interrupter + 0x10, erst.physical)?;
        mmio_write64(interrupter + 0x18, event_ring.physical)?;

        mmio_write32(op_base, 1)?;
        wait_until(op_base + 0x04, 1, 0)?;

        let connected_port = find_connected_port(op_base, max_ports)?;
        let mut controller = Self {
            mmio, cap_length, max_slots, max_ports, op_base, runtime_base, doorbell_base,
            dcbaa, command_ring, event_ring, erst, event_index: 0, event_cycle: true,
            connected_port, slot_id: 0,
        };
        controller.reset_port(connected_port)?;
        controller.slot_id = controller.enable_slot()?;
        Ok(controller)
    }

    fn reset_port(&mut self, port: u8) -> Result<(), InitError> {
        let portsc = self.op_base + 0x400 + (u64::from(port - 1) * 0x10);
        let value = mmio_read32(portsc)?;
        mmio_write32(portsc, value | (1 << 4))?;
        for _ in 0..POLL_LIMIT {
            let current = mmio_read32(portsc)?;
            if current & (1 << 4) == 0 && current & 1 != 0 { return Ok(()); }
            core::hint::spin_loop();
        }
        Err(InitError::ControllerTimeout)
    }

    fn enable_slot(&mut self) -> Result<u8, InitError> {
        let ptr = self.command_ring.push(Trb { parameter: 0, status: 0, control: TRB_TYPE_ENABLE_SLOT << 10 });
        mmio_write32(self.doorbell_base, 0)?;
        for _ in 0..64 {
            let event = self.wait_event()?;
            if trb_type(event.control) != TRB_TYPE_COMMAND_COMPLETION {
                // Root-port reset/connection changes can legitimately enqueue
                // Port Status Change events ahead of command completions.
                continue;
            }
            if event.parameter != ptr || completion_code(event.status) != 1 {
                return Err(InitError::CommandFailed);
            }
            let slot = ((event.control >> 24) & 0xff) as u8;
            if slot == 0 { return Err(InitError::CommandFailed); }
            return Ok(slot);
        }
        Err(InitError::CommandFailed)
    }

    fn wait_event(&mut self) -> Result<Trb, InitError> {
        for _ in 0..POLL_LIMIT {
            fence(Ordering::Acquire);
            let offset = self.event_index * 16;
            let bytes = self.event_ring.bytes_mut();
            let control = u32::from_le_bytes(bytes[offset+12..offset+16].try_into().unwrap());
            if (control & 1 != 0) == self.event_cycle {
                let event = Trb {
                    parameter: u64::from_le_bytes(bytes[offset..offset+8].try_into().unwrap()),
                    status: u32::from_le_bytes(bytes[offset+8..offset+12].try_into().unwrap()),
                    control,
                };
                self.event_index += 1;
                if self.event_index == PAGE_SIZE / 16 { self.event_index = 0; self.event_cycle = !self.event_cycle; }
                let dequeue = self.event_ring.physical + (self.event_index as u64 * 16);
                mmio_write64(self.runtime_base + 0x20 + 0x18, dequeue | (1 << 3))?;
                return Ok(event);
            }
            core::hint::spin_loop();
        }
        Err(InitError::ControllerTimeout)
    }

    pub fn summary(&self) -> (u8,u8,u8,u8) {
        (self.max_slots, self.max_ports, self.connected_port, self.slot_id)
    }
}

pub fn init() -> Result<(u8,u8,u8,u8), InitError> {
    let mut slot = CONTROLLER.lock();
    if let Some(controller) = slot.as_ref() { return Ok(controller.summary()); }
    let device = find_controller().ok_or(InitError::MissingController)?;
    let controller = XhciController::initialize(device)?;
    let summary = controller.summary();
    *slot = Some(controller);
    Ok(summary)
}

pub fn probe() -> bool { find_controller().is_some() }

fn find_controller() -> Option<pci::Device> {
    for index in 0..64 {
        if let Some(device) = pci::device(index) {
            if device.class == USB_CLASS && device.subclass == USB_SUBCLASS && device.prog_if == XHCI_PROG_IF { return Some(device); }
        }
    }
    None
}

fn find_connected_port(op_base: u64, max_ports: u8) -> Result<u8, InitError> {
    for port in 1..=max_ports {
        let portsc = mmio_read32(op_base + 0x400 + (u64::from(port - 1) * 0x10))?;
        if portsc & 1 != 0 { return Ok(port); }
    }
    Err(InitError::NoConnectedPort)
}

pub fn self_test() -> bool {
    trb_type(TRB_TYPE_ENABLE_SLOT << 10) == TRB_TYPE_ENABLE_SLOT
        && completion_code(1 << 24) == 1
        && trb_type(TRB_TYPE_PORT_STATUS_CHANGE << 10) == TRB_TYPE_PORT_STATUS_CHANGE
}

fn trb_type(control: u32) -> u32 { (control >> 10) & 0x3f }
fn completion_code(status: u32) -> u8 { (status >> 24) as u8 }

fn wait_until(address: u64, mask: u32, expected: u32) -> Result<(), InitError> {
    for _ in 0..POLL_LIMIT {
        if mmio_read32(address)? & mask == expected { return Ok(()); }
        core::hint::spin_loop();
    }
    Err(InitError::ControllerTimeout)
}

fn mmio_read8(physical: u64) -> Result<u8, InitError> {
    let page = physical & !0xfff;
    let offset = physical & 0xfff;
    let mapped = paging::map_mmio(page).map_err(|_| InitError::MmioUnavailable)?;
    Ok(unsafe { ptr::read_volatile((mapped + offset) as *const u8) })
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
fn mmio_write64(physical: u64, value: u64) -> Result<(), InitError> {
    mmio_write32(physical, value as u32)?;
    mmio_write32(physical + 4, (value >> 32) as u32)
}

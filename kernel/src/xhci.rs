use core::{
    cmp, ptr,
    sync::atomic::{fence, Ordering},
};

use crate::{
    hal::pci::{self, Address, BarKind},
    irq_lock::IrqMutex as Mutex,
    memory, paging,
};

const USB_CLASS: u8 = 0x0c;
const USB_SUBCLASS: u8 = 0x03;
const XHCI_PROG_IF: u8 = 0x30;
const PAGE_SIZE: usize = 4096;
const POLL_LIMIT: usize = 4_000_000;
const MAX_EVENTS_PER_WAIT: usize = 64;

const TRB_TYPE_NORMAL: u32 = 1;
const TRB_TYPE_SETUP_STAGE: u32 = 2;
const TRB_TYPE_DATA_STAGE: u32 = 3;
const TRB_TYPE_STATUS_STAGE: u32 = 4;
const TRB_TYPE_ENABLE_SLOT: u32 = 9;
const TRB_TYPE_ADDRESS_DEVICE: u32 = 11;
const TRB_TYPE_CONFIGURE_ENDPOINT: u32 = 12;
const TRB_TYPE_TRANSFER_EVENT: u32 = 32;
const TRB_TYPE_COMMAND_COMPLETION: u32 = 33;
const TRB_TYPE_PORT_STATUS_CHANGE: u32 = 34;

const USB_REQUEST_GET_DESCRIPTOR: u8 = 6;
const USB_REQUEST_SET_CONFIGURATION: u8 = 9;
const USB_REQUEST_SET_IDLE: u8 = 10;
const USB_REQUEST_SET_PROTOCOL: u8 = 11;
const USB_DESCRIPTOR_DEVICE: u8 = 1;
const USB_DESCRIPTOR_CONFIGURATION: u8 = 2;
const USB_CLASS_HID: u8 = 3;
const USB_PROTOCOL_KEYBOARD: u8 = 1;
const USB_PROTOCOL_MOUSE: u8 = 2;

static CONTROLLER: Mutex<Option<XhciController>> = Mutex::with_rank(None, 10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    MissingController,
    MissingMemoryBar,
    DmaUnavailable,
    MmioUnavailable,
    ControllerTimeout,
    CommandFailed,
    TransferFailed,
    NoConnectedPort,
    InvalidRegisters,
    DescriptorInvalid,
    HidNotFound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HidKind {
    Keyboard,
    Mouse,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HidSummary {
    pub slot: u8,
    pub port: u8,
    pub interface: u8,
    pub endpoint: u8,
    pub max_packet: u16,
    pub kind: HidKind,
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
        // SAFETY: the allocated frame is exclusively owned by this DMA object.
        unsafe { ptr::write_bytes(virtual_address as *mut u8, 0, PAGE_SIZE) };
        Some(Self {
            physical,
            virtual_address,
        })
    }

    fn bytes(&self) -> &[u8; PAGE_SIZE] {
        // SAFETY: DMA pages remain allocated for the lifetime of the controller.
        unsafe { &*(self.virtual_address as *const [u8; PAGE_SIZE]) }
    }

    fn bytes_mut(&mut self) -> &mut [u8; PAGE_SIZE] {
        // SAFETY: all mutable access is serialized by the controller mutex.
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
        let mut ring = Self {
            page,
            index: 0,
            cycle: true,
        };
        ring.install_link();
        ring
    }

    fn install_link(&mut self) {
        let slot = (PAGE_SIZE / 16) - 1;
        let mut trb = Trb {
            parameter: self.page.physical,
            status: 0,
            control: (6 << 10) | (1 << 1),
        };
        if self.cycle {
            trb.control |= 1;
        }
        self.write(slot, trb);
    }

    fn push(&mut self, mut trb: Trb) -> u64 {
        if self.index == (PAGE_SIZE / 16) - 1 {
            self.index = 0;
            self.cycle = !self.cycle;
            self.install_link();
        }
        if self.cycle {
            trb.control |= 1;
        } else {
            trb.control &= !1;
        }
        let index = self.index;
        self.write(index, trb);
        fence(Ordering::Release);
        self.index += 1;
        self.page.physical + (index as u64 * 16)
    }

    fn write(&mut self, index: usize, trb: Trb) {
        let offset = index * 16;
        let bytes = self.page.bytes_mut();
        bytes[offset..offset + 8].copy_from_slice(&trb.parameter.to_le_bytes());
        bytes[offset + 8..offset + 12].copy_from_slice(&trb.status.to_le_bytes());
        bytes[offset + 12..offset + 16].copy_from_slice(&trb.control.to_le_bytes());
    }
}

#[derive(Clone, Copy)]
struct HidInterface {
    interface: u8,
    configuration: u8,
    endpoint_address: u8,
    max_packet: u16,
    interval: u8,
    protocol: u8,
}

pub struct XhciController {
    max_slots: u8,
    max_ports: u8,
    op_base: u64,
    runtime_base: u64,
    doorbell_base: u64,
    context_size: usize,
    dcbaa: DmaPage,
    command_ring: Ring,
    event_ring: DmaPage,
    _erst: DmaPage,
    event_index: usize,
    event_cycle: bool,
    connected_port: u8,
    slot_id: u8,
    output_context: Option<DmaPage>,
    ep0_ring: Option<Ring>,
    hid_ring: Option<Ring>,
    hid_buffer: Option<DmaPage>,
    hid: Option<HidInterface>,
}

impl XhciController {
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
            return Err(InitError::MissingController);
        }
        let mmio = bar.address;
        let cap_length = mmio_read8(mmio)?;
        if cap_length < 0x20 {
            return Err(InitError::InvalidRegisters);
        }
        let hcs1 = mmio_read32(mmio + 0x04)?;
        let hcc1 = mmio_read32(mmio + 0x10)?;
        let max_slots = (hcs1 & 0xff) as u8;
        let max_ports = ((hcs1 >> 24) & 0xff) as u8;
        if max_slots == 0 || max_ports == 0 {
            return Err(InitError::InvalidRegisters);
        }
        let context_size = if hcc1 & (1 << 2) != 0 { 64 } else { 32 };
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
        // Stage 13.6 deliberately keeps completion delivery polling-based until
        // the generic MSI/MSI-X allocator is available. Keep the interrupter
        // disabled while still programming its event-ring state.
        mmio_write32(interrupter, 0)?;
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
            max_slots,
            max_ports,
            op_base,
            runtime_base,
            doorbell_base,
            context_size,
            dcbaa,
            command_ring,
            event_ring,
            _erst: erst,
            event_index: 0,
            event_cycle: true,
            connected_port,
            slot_id: 0,
            output_context: None,
            ep0_ring: None,
            hid_ring: None,
            hid_buffer: None,
            hid: None,
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
            if current & (1 << 4) == 0 && current & 1 != 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(InitError::ControllerTimeout)
    }

    fn port_speed(&self) -> Result<u8, InitError> {
        let portsc = self.op_base + 0x400 + (u64::from(self.connected_port - 1) * 0x10);
        Ok(((mmio_read32(portsc)? >> 10) & 0x0f) as u8)
    }

    fn enable_slot(&mut self) -> Result<u8, InitError> {
        let command_ptr = self.command_ring.push(Trb {
            parameter: 0,
            status: 0,
            control: TRB_TYPE_ENABLE_SLOT << 10,
        });
        mmio_write32(self.doorbell_base, 0)?;
        let event = self.wait_command_completion(command_ptr)?;
        let slot = ((event.control >> 24) & 0xff) as u8;
        if slot == 0 {
            return Err(InitError::CommandFailed);
        }
        Ok(slot)
    }

    fn address_device(&mut self) -> Result<(), InitError> {
        let speed = self.port_speed()?;
        let ep0_max_packet: u32 = match speed {
            3 => 64,
            4 | 5 => 512,
            _ => 8,
        };
        let mut output = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        let ep0_page = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        let ep0_ring = Ring::new(ep0_page);
        let mut input = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        output.bytes_mut().fill(0);
        input.bytes_mut().fill(0);

        write_u64(
            self.dcbaa.bytes_mut(),
            usize::from(self.slot_id) * 8,
            output.physical,
        );
        write_u32(input.bytes_mut(), 4, 0x3); // add Slot + EP0 contexts
        let slot_offset = self.context_size;
        write_u32(
            input.bytes_mut(),
            slot_offset,
            (u32::from(speed) << 20) | (1 << 27),
        );
        write_u32(
            input.bytes_mut(),
            slot_offset + 4,
            u32::from(self.connected_port) << 16,
        );
        let ep0_offset = self.context_size * 2;
        write_u32(
            input.bytes_mut(),
            ep0_offset + 4,
            (3 << 1) | (4 << 3) | (ep0_max_packet << 16),
        );
        write_u64(
            input.bytes_mut(),
            ep0_offset + 8,
            ep0_ring.page.physical | 1,
        );
        write_u32(input.bytes_mut(), ep0_offset + 16, 8);
        fence(Ordering::Release);

        let command_ptr = self.command_ring.push(Trb {
            parameter: input.physical,
            status: 0,
            control: (TRB_TYPE_ADDRESS_DEVICE << 10) | (u32::from(self.slot_id) << 24),
        });
        mmio_write32(self.doorbell_base, 0)?;
        self.wait_command_completion(command_ptr)?;
        self.output_context = Some(output);
        self.ep0_ring = Some(ep0_ring);
        Ok(())
    }

    fn enumerate_hid(&mut self) -> Result<HidSummary, InitError> {
        self.address_device()?;

        let mut descriptor = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        self.control_in(
            0x80,
            USB_REQUEST_GET_DESCRIPTOR,
            u16::from(USB_DESCRIPTOR_DEVICE) << 8,
            0,
            &mut descriptor,
            18,
        )?;
        let device = descriptor.bytes();
        if device[0] < 18 || device[1] != USB_DESCRIPTOR_DEVICE {
            return Err(InitError::DescriptorInvalid);
        }

        self.control_in(
            0x80,
            USB_REQUEST_GET_DESCRIPTOR,
            u16::from(USB_DESCRIPTOR_CONFIGURATION) << 8,
            0,
            &mut descriptor,
            9,
        )?;
        let header = descriptor.bytes();
        if header[0] < 9 || header[1] != USB_DESCRIPTOR_CONFIGURATION {
            return Err(InitError::DescriptorInvalid);
        }
        let total_length = usize::from(u16::from_le_bytes([header[2], header[3]]));
        if !(9..=PAGE_SIZE).contains(&total_length) {
            return Err(InitError::DescriptorInvalid);
        }
        let configuration = header[5];
        self.control_in(
            0x80,
            USB_REQUEST_GET_DESCRIPTOR,
            u16::from(USB_DESCRIPTOR_CONFIGURATION) << 8,
            0,
            &mut descriptor,
            total_length,
        )?;
        let hid = parse_hid_interface(&descriptor.bytes()[..total_length], configuration)
            .ok_or(InitError::HidNotFound)?;

        self.control_no_data(
            0x00,
            USB_REQUEST_SET_CONFIGURATION,
            u16::from(hid.configuration),
            0,
        )?;
        if hid.protocol == USB_PROTOCOL_KEYBOARD || hid.protocol == USB_PROTOCOL_MOUSE {
            // HID Set Protocol: wValue=0 selects the fixed boot report format.
            self.control_no_data(0x21, USB_REQUEST_SET_PROTOCOL, 0, u16::from(hid.interface))?;
            // A small non-zero idle period makes an unchanged boot report observable
            // on QEMU's interrupt endpoint without synthesizing host key input.
            self.control_no_data(0x21, USB_REQUEST_SET_IDLE, 1 << 8, u16::from(hid.interface))?;
        }
        self.configure_hid_endpoint(hid)?;
        self.hid = Some(hid);

        Ok(HidSummary {
            slot: self.slot_id,
            port: self.connected_port,
            interface: hid.interface,
            endpoint: hid.endpoint_address,
            max_packet: hid.max_packet,
            kind: hid_kind(hid.protocol),
        })
    }

    fn control_in(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        buffer: &mut DmaPage,
        length: usize,
    ) -> Result<usize, InitError> {
        if length == 0 || length > PAGE_SIZE {
            return Err(InitError::DescriptorInvalid);
        }
        buffer.bytes_mut()[..length].fill(0);
        let setup = setup_packet(request_type, request, value, index, length as u16);
        let ring = self.ep0_ring.as_mut().ok_or(InitError::CommandFailed)?;
        ring.push(Trb {
            parameter: setup,
            status: 8,
            control: (TRB_TYPE_SETUP_STAGE << 10) | (1 << 6) | (3 << 16),
        });
        ring.push(Trb {
            parameter: buffer.physical,
            status: length as u32,
            control: (TRB_TYPE_DATA_STAGE << 10) | (1 << 16),
        });
        let status_ptr = ring.push(Trb {
            parameter: 0,
            status: 0,
            control: (TRB_TYPE_STATUS_STAGE << 10) | (1 << 5),
        });
        self.ring_endpoint(1)?;
        self.wait_transfer_completion(status_ptr, self.slot_id)?;
        fence(Ordering::Acquire);
        Ok(length)
    }

    fn control_no_data(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
    ) -> Result<(), InitError> {
        let setup = setup_packet(request_type, request, value, index, 0);
        let ring = self.ep0_ring.as_mut().ok_or(InitError::CommandFailed)?;
        ring.push(Trb {
            parameter: setup,
            status: 8,
            control: (TRB_TYPE_SETUP_STAGE << 10) | (1 << 6),
        });
        let status_ptr = ring.push(Trb {
            parameter: 0,
            status: 0,
            control: (TRB_TYPE_STATUS_STAGE << 10) | (1 << 16) | (1 << 5),
        });
        self.ring_endpoint(1)?;
        self.wait_transfer_completion(status_ptr, self.slot_id)?;
        Ok(())
    }

    fn configure_hid_endpoint(&mut self, hid: HidInterface) -> Result<(), InitError> {
        if hid.endpoint_address & 0x80 == 0 || hid.max_packet == 0 {
            return Err(InitError::DescriptorInvalid);
        }
        let endpoint_number = hid.endpoint_address & 0x0f;
        if endpoint_number == 0 {
            return Err(InitError::DescriptorInvalid);
        }
        let dci = endpoint_number.saturating_mul(2).saturating_add(1);
        if dci > 31 {
            return Err(InitError::DescriptorInvalid);
        }

        let hid_page = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        let hid_ring = Ring::new(hid_page);
        let hid_buffer = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        let mut input = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        input.bytes_mut().fill(0);

        let output = self.output_context.ok_or(InitError::CommandFailed)?;
        input.bytes_mut()[self.context_size..self.context_size * 2]
            .copy_from_slice(&output.bytes()[..self.context_size]);
        write_u32(input.bytes_mut(), 4, 1 | (1_u32 << dci));
        let slot_offset = self.context_size;
        let mut slot_dword0 = read_u32(input.bytes(), slot_offset);
        slot_dword0 &= !(0x1f << 27);
        slot_dword0 |= u32::from(dci) << 27;
        write_u32(input.bytes_mut(), slot_offset, slot_dword0);

        let endpoint_offset = self.context_size * (usize::from(dci) + 1);
        let interval = xhci_interval(self.port_speed()?, hid.interval);
        write_u32(
            input.bytes_mut(),
            endpoint_offset,
            u32::from(interval) << 16,
        );
        write_u32(
            input.bytes_mut(),
            endpoint_offset + 4,
            (3 << 1) | (7 << 3) | (u32::from(hid.max_packet) << 16),
        );
        write_u64(
            input.bytes_mut(),
            endpoint_offset + 8,
            hid_ring.page.physical | 1,
        );
        write_u32(
            input.bytes_mut(),
            endpoint_offset + 16,
            u32::from(hid.max_packet) | (u32::from(hid.max_packet) << 16),
        );
        fence(Ordering::Release);

        let command_ptr = self.command_ring.push(Trb {
            parameter: input.physical,
            status: 0,
            control: (TRB_TYPE_CONFIGURE_ENDPOINT << 10) | (u32::from(self.slot_id) << 24),
        });
        mmio_write32(self.doorbell_base, 0)?;
        self.wait_command_completion(command_ptr)?;
        self.hid_ring = Some(hid_ring);
        self.hid_buffer = Some(hid_buffer);
        Ok(())
    }

    pub fn poll_hid_report(&mut self) -> Result<[u8; 8], InitError> {
        let hid = self.hid.ok_or(InitError::HidNotFound)?;
        let endpoint_number = hid.endpoint_address & 0x0f;
        let dci = endpoint_number.saturating_mul(2).saturating_add(1);
        let transfer_length = cmp::min(usize::from(hid.max_packet), 8);
        if transfer_length == 0 {
            return Err(InitError::DescriptorInvalid);
        }
        let buffer_physical = {
            let buffer = self.hid_buffer.as_mut().ok_or(InitError::DmaUnavailable)?;
            buffer.bytes_mut()[..transfer_length].fill(0);
            buffer.physical
        };
        let normal_ptr = {
            let ring = self.hid_ring.as_mut().ok_or(InitError::CommandFailed)?;
            ring.push(Trb {
                parameter: buffer_physical,
                status: transfer_length as u32,
                control: (TRB_TYPE_NORMAL << 10) | (1 << 5),
            })
        };
        self.ring_endpoint(dci)?;
        self.wait_transfer_completion(normal_ptr, self.slot_id)?;
        fence(Ordering::Acquire);
        let buffer = self.hid_buffer.as_ref().ok_or(InitError::DmaUnavailable)?;
        let mut report = [0_u8; 8];
        report[..transfer_length].copy_from_slice(&buffer.bytes()[..transfer_length]);
        Ok(report)
    }

    fn ring_endpoint(&self, dci: u8) -> Result<(), InitError> {
        mmio_write32(
            self.doorbell_base + (u64::from(self.slot_id) * 4),
            u32::from(dci),
        )
    }

    fn wait_command_completion(&mut self, command_ptr: u64) -> Result<Trb, InitError> {
        for _ in 0..MAX_EVENTS_PER_WAIT {
            let event = self.wait_event()?;
            if trb_type(event.control) != TRB_TYPE_COMMAND_COMPLETION {
                continue;
            }
            if event.parameter != command_ptr || completion_code(event.status) != 1 {
                return Err(InitError::CommandFailed);
            }
            return Ok(event);
        }
        Err(InitError::CommandFailed)
    }

    fn wait_transfer_completion(&mut self, trb_ptr: u64, slot: u8) -> Result<Trb, InitError> {
        for _ in 0..MAX_EVENTS_PER_WAIT {
            let event = self.wait_event()?;
            if trb_type(event.control) != TRB_TYPE_TRANSFER_EVENT {
                continue;
            }
            if ((event.control >> 24) & 0xff) as u8 != slot || event.parameter != trb_ptr {
                continue;
            }
            let code = completion_code(event.status);
            if code == 1 || code == 13 {
                return Ok(event);
            }
            return Err(InitError::TransferFailed);
        }
        Err(InitError::TransferFailed)
    }

    fn wait_event(&mut self) -> Result<Trb, InitError> {
        for _ in 0..POLL_LIMIT {
            fence(Ordering::Acquire);
            let offset = self.event_index * 16;
            let bytes = self.event_ring.bytes_mut();
            let control = u32::from_le_bytes(bytes[offset + 12..offset + 16].try_into().unwrap());
            if (control & 1 != 0) == self.event_cycle {
                let event = Trb {
                    parameter: u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap()),
                    status: u32::from_le_bytes(bytes[offset + 8..offset + 12].try_into().unwrap()),
                    control,
                };
                self.event_index += 1;
                if self.event_index == PAGE_SIZE / 16 {
                    self.event_index = 0;
                    self.event_cycle = !self.event_cycle;
                }
                let dequeue = self.event_ring.physical + (self.event_index as u64 * 16);
                mmio_write64(self.runtime_base + 0x20 + 0x18, dequeue | (1 << 3))?;
                return Ok(event);
            }
            core::hint::spin_loop();
        }
        Err(InitError::ControllerTimeout)
    }

    pub fn summary(&self) -> (u8, u8, u8, u8) {
        (
            self.max_slots,
            self.max_ports,
            self.connected_port,
            self.slot_id,
        )
    }
}

pub fn init() -> Result<(u8, u8, u8, u8), InitError> {
    let mut slot = CONTROLLER.lock();
    if let Some(controller) = slot.as_ref() {
        return Ok(controller.summary());
    }
    let device = find_controller().ok_or(InitError::MissingController)?;
    let controller = XhciController::initialize(device)?;
    let summary = controller.summary();
    *slot = Some(controller);
    Ok(summary)
}

pub fn init_hid() -> Result<HidSummary, InitError> {
    let mut slot = CONTROLLER.lock();
    if slot.is_none() {
        let device = find_controller().ok_or(InitError::MissingController)?;
        *slot = Some(XhciController::initialize(device)?);
    }
    slot.as_mut()
        .ok_or(InitError::MissingController)?
        .enumerate_hid()
}

pub fn poll_hid_report() -> Result<[u8; 8], InitError> {
    CONTROLLER
        .lock()
        .as_mut()
        .ok_or(InitError::MissingController)?
        .poll_hid_report()
}

pub fn probe() -> bool {
    find_controller().is_some()
}

fn find_controller() -> Option<pci::Device> {
    for index in 0..64 {
        if let Some(device) = pci::device(index) {
            if device.class == USB_CLASS
                && device.subclass == USB_SUBCLASS
                && device.prog_if == XHCI_PROG_IF
            {
                return Some(device);
            }
        }
    }
    None
}

fn find_connected_port(op_base: u64, max_ports: u8) -> Result<u8, InitError> {
    for port in 1..=max_ports {
        let portsc = mmio_read32(op_base + 0x400 + (u64::from(port - 1) * 0x10))?;
        if portsc & 1 != 0 {
            return Ok(port);
        }
    }
    Err(InitError::NoConnectedPort)
}

fn parse_hid_interface(bytes: &[u8], configuration: u8) -> Option<HidInterface> {
    let mut offset = 0usize;
    let mut current: Option<(u8, u8)> = None;
    while offset + 2 <= bytes.len() {
        let length = usize::from(bytes[offset]);
        if length < 2 || offset + length > bytes.len() {
            return None;
        }
        match bytes[offset + 1] {
            4 if length >= 9 => {
                if bytes[offset + 5] == USB_CLASS_HID {
                    current = Some((bytes[offset + 2], bytes[offset + 7]));
                } else {
                    current = None;
                }
            }
            5 if length >= 7 => {
                if let Some((interface, protocol)) = current {
                    let address = bytes[offset + 2];
                    let attributes = bytes[offset + 3] & 0x03;
                    if address & 0x80 != 0 && attributes == 0x03 {
                        let max_packet =
                            u16::from_le_bytes([bytes[offset + 4], bytes[offset + 5]]) & 0x07ff;
                        if max_packet != 0 {
                            return Some(HidInterface {
                                interface,
                                configuration,
                                endpoint_address: address,
                                max_packet,
                                interval: bytes[offset + 6],
                                protocol,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
        offset += length;
    }
    None
}

fn hid_kind(protocol: u8) -> HidKind {
    match protocol {
        USB_PROTOCOL_KEYBOARD => HidKind::Keyboard,
        USB_PROTOCOL_MOUSE => HidKind::Mouse,
        _ => HidKind::Other,
    }
}

fn xhci_interval(speed: u8, usb_interval: u8) -> u8 {
    let interval = usb_interval.max(1);
    if speed >= 3 {
        interval.saturating_sub(1).min(15)
    } else {
        let mut frames = u16::from(interval);
        let mut exponent = 0_u8;
        while frames > 1 {
            frames = frames.div_ceil(2);
            exponent = exponent.saturating_add(1);
        }
        exponent.saturating_add(3).min(15)
    }
}

fn setup_packet(request_type: u8, request: u8, value: u16, index: u16, length: u16) -> u64 {
    u64::from(request_type)
        | (u64::from(request) << 8)
        | (u64::from(value) << 16)
        | (u64::from(index) << 32)
        | (u64::from(length) << 48)
}

pub fn decode_boot_keyboard(report: [u8; 8]) -> Option<u8> {
    report[2..8].iter().copied().find(|key| *key != 0)
}

pub fn self_test() -> bool {
    let descriptor = [
        9, 2, 25, 0, 1, 1, 0, 0x80, 50, 9, 4, 0, 0, 1, 3, 1, 1, 0, 7, 5, 0x81, 3, 8, 0, 10,
    ];
    let hid = parse_hid_interface(&descriptor, 1);
    trb_type(TRB_TYPE_ENABLE_SLOT << 10) == TRB_TYPE_ENABLE_SLOT
        && completion_code(1 << 24) == 1
        && trb_type(TRB_TYPE_PORT_STATUS_CHANGE << 10) == TRB_TYPE_PORT_STATUS_CHANGE
        && hid
            .map(|entry| entry.protocol == USB_PROTOCOL_KEYBOARD && entry.endpoint_address == 0x81)
            == Some(true)
        && decode_boot_keyboard([0, 0, 4, 0, 0, 0, 0, 0]) == Some(4)
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn trb_type(control: u32) -> u32 {
    (control >> 10) & 0x3f
}
fn completion_code(status: u32) -> u8 {
    (status >> 24) as u8
}

fn wait_until(address: u64, mask: u32, expected: u32) -> Result<(), InitError> {
    for _ in 0..POLL_LIMIT {
        if mmio_read32(address)? & mask == expected {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(InitError::ControllerTimeout)
}

fn mmio_read8(physical: u64) -> Result<u8, InitError> {
    let page = physical & !0xfff;
    let offset = physical & 0xfff;
    let mapped = paging::map_mmio(page).map_err(|_| InitError::MmioUnavailable)?;
    // SAFETY: map_mmio establishes a mapping for this controller register page.
    Ok(unsafe { ptr::read_volatile((mapped + offset) as *const u8) })
}

fn mmio_read32(physical: u64) -> Result<u32, InitError> {
    let page = physical & !0xfff;
    let offset = physical & 0xfff;
    let mapped = paging::map_mmio(page).map_err(|_| InitError::MmioUnavailable)?;
    // SAFETY: map_mmio establishes a mapping for this controller register page.
    Ok(unsafe { ptr::read_volatile((mapped + offset) as *const u32) })
}

fn mmio_write32(physical: u64, value: u32) -> Result<(), InitError> {
    let page = physical & !0xfff;
    let offset = physical & 0xfff;
    let mapped = paging::map_mmio(page).map_err(|_| InitError::MmioUnavailable)?;
    // SAFETY: map_mmio establishes a mapping for this controller register page.
    unsafe { ptr::write_volatile((mapped + offset) as *mut u32, value) };
    Ok(())
}

fn mmio_write64(physical: u64, value: u64) -> Result<(), InitError> {
    mmio_write32(physical, value as u32)?;
    mmio_write32(physical + 4, (value >> 32) as u32)
}

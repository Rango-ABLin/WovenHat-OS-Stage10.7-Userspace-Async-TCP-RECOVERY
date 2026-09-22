use core::{
    ptr,
    sync::atomic::{fence, Ordering},
};

use crate::{
    block::{BlockDevice, Error, SECTOR_SIZE},
    hal::pci::{self, Address, BarKind},
    irq_lock::IrqMutex as Mutex,
    memory, paging,
};

const NVME_CLASS: u8 = 0x01;
const NVME_SUBCLASS: u8 = 0x08;
const NVME_PROG_IF: u8 = 0x02;
const QUEUE_DEPTH: u16 = 16;
const PAGE_SIZE: usize = 4096;
const ADMIN_IDENTIFY: u8 = 0x06;
const ADMIN_CREATE_IO_CQ: u8 = 0x05;
const ADMIN_CREATE_IO_SQ: u8 = 0x01;
const IO_FLUSH: u8 = 0x00;
const IO_WRITE: u8 = 0x01;
const IO_READ: u8 = 0x02;
const READY_POLL_LIMIT: usize = 2_000_000;
const COMPLETION_POLL_LIMIT: usize = 2_000_000;

static CONTROLLER: Mutex<Option<NvmeController>> = Mutex::with_rank(None, 10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    MissingController,
    MissingMemoryBar,
    UnsupportedPageSize,
    UnsupportedNamespace,
    DmaUnavailable,
    MmioUnavailable,
    ControllerTimeout,
    CommandFailed,
    IoQueueFailed,
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
        // SAFETY: The frame is exclusively owned by this DMA page and is
        // reachable through the bootloader's direct physical-memory mapping.
        unsafe { ptr::write_bytes(virtual_address as *mut u8, 0, PAGE_SIZE) };
        Some(Self {
            physical,
            virtual_address,
        })
    }

    fn bytes_mut(&mut self) -> &mut [u8; PAGE_SIZE] {
        // SAFETY: DmaPage owns the frame and Queue access is serialized by the
        // controller lock, so no competing Rust reference is created.
        unsafe { &mut *(self.virtual_address as *mut [u8; PAGE_SIZE]) }
    }
}

struct Queue {
    sq: DmaPage,
    cq: DmaPage,
    depth: u16,
    sq_tail: u16,
    cq_head: u16,
    phase: bool,
    next_cid: u16,
    sq_doorbell: u64,
    cq_doorbell: u64,
}

impl Queue {
    fn new(sq: DmaPage, cq: DmaPage, depth: u16, sq_doorbell: u64, cq_doorbell: u64) -> Self {
        Self {
            sq,
            cq,
            depth,
            sq_tail: 0,
            cq_head: 0,
            phase: true,
            next_cid: 1,
            sq_doorbell,
            cq_doorbell,
        }
    }

    fn submit(&mut self, command: &[u8; 64]) -> Result<(), InitError> {
        let offset = usize::from(self.sq_tail) * 64;
        self.sq.bytes_mut()[offset..offset + 64].copy_from_slice(command);
        fence(Ordering::Release);
        self.sq_tail = (self.sq_tail + 1) % self.depth;
        mmio_write32(self.sq_doorbell, u32::from(self.sq_tail))
    }

    fn wait(&mut self, cid: u16) -> Result<(), InitError> {
        for _ in 0..COMPLETION_POLL_LIMIT {
            fence(Ordering::Acquire);
            let offset = usize::from(self.cq_head) * 16;
            let bytes = &self.cq.bytes_mut()[offset..offset + 16];
            let status = u16::from_le_bytes([bytes[14], bytes[15]]);
            if completion_phase(status, self.phase) {
                let completed_cid = u16::from_le_bytes([bytes[12], bytes[13]]);
                if completed_cid != cid || status >> 1 != 0 {
                    return Err(InitError::CommandFailed);
                }
                self.cq_head += 1;
                if self.cq_head == self.depth {
                    self.cq_head = 0;
                    self.phase = !self.phase;
                }
                mmio_write32(self.cq_doorbell, u32::from(self.cq_head))?;
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(InitError::ControllerTimeout)
    }

    fn command(
        &mut self,
        opcode: u8,
        nsid: u32,
        prp1: u64,
        cdw10: u32,
        cdw11: u32,
        cdw12: u32,
    ) -> Result<(), InitError> {
        let cid = self.next_cid;
        self.next_cid = self.next_cid.wrapping_add(1).max(1);
        let mut command = [0_u8; 64];
        command[0] = opcode;
        command[2..4].copy_from_slice(&cid.to_le_bytes());
        command[4..8].copy_from_slice(&nsid.to_le_bytes());
        command[24..32].copy_from_slice(&prp1.to_le_bytes());
        command[40..44].copy_from_slice(&cdw10.to_le_bytes());
        command[44..48].copy_from_slice(&cdw11.to_le_bytes());
        command[48..52].copy_from_slice(&cdw12.to_le_bytes());
        self.submit(&command)?;
        self.wait(cid)
    }
}

pub struct NvmeController {
    namespace_id: u32,
    sectors: u64,
    io: Queue,
    transfer: DmaPage,
}

impl NvmeController {
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
        let registers = bar.address;
        let cap = mmio_read64(registers)?;
        let mqes = ((cap & 0xffff) as u16).saturating_add(1);
        let depth = core::cmp::min(QUEUE_DEPTH, mqes);
        if depth < 2 || ((cap >> 48) & 0xf) != 0 {
            return Err(InitError::UnsupportedPageSize);
        }
        let stride = 4_u64 << ((cap >> 32) & 0xf);

        let cc = mmio_read32(registers + 0x14)?;
        mmio_write32(registers + 0x14, cc & !1)?;
        wait_ready(registers, false)?;

        let admin_sq = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        let admin_cq = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        mmio_write32(
            registers + 0x24,
            (u32::from(depth - 1) << 16) | u32::from(depth - 1),
        )?;
        mmio_write64(registers + 0x28, admin_sq.physical)?;
        mmio_write64(registers + 0x30, admin_cq.physical)?;

        // IOSQES=6 (64 bytes), IOCQES=4 (16 bytes), MPS=0 (4 KiB), EN=1.
        mmio_write32(registers + 0x14, (6 << 16) | (4 << 20) | 1)?;
        wait_ready(registers, true)?;

        let mut admin = Queue::new(
            admin_sq,
            admin_cq,
            depth,
            registers + 0x1000,
            registers + 0x1000 + stride,
        );
        let mut identify = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        admin.command(ADMIN_IDENTIFY, 0, identify.physical, 1, 0, 0)?;
        let namespace_count = read_u32(identify.bytes_mut(), 516);
        if namespace_count == 0 {
            return Err(InitError::UnsupportedNamespace);
        }

        identify.bytes_mut().fill(0);
        admin.command(ADMIN_IDENTIFY, 1, identify.physical, 0, 0, 0)?;
        let namespace_size = read_u64(identify.bytes_mut(), 0);
        let flbas = identify.bytes_mut()[26] & 0x0f;
        let lbaf_offset = 128 + usize::from(flbas) * 4;
        let lba_shift = identify.bytes_mut()[lbaf_offset + 2];
        if lba_shift != 9 || namespace_size == 0 {
            return Err(InitError::UnsupportedNamespace);
        }

        let io_sq = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        let io_cq = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        admin
            .command(
                ADMIN_CREATE_IO_CQ,
                0,
                io_cq.physical,
                (u32::from(depth - 1) << 16) | 1,
                1,
                0,
            )
            .map_err(|_| InitError::IoQueueFailed)?;
        admin
            .command(
                ADMIN_CREATE_IO_SQ,
                0,
                io_sq.physical,
                (u32::from(depth - 1) << 16) | 1,
                (1 << 16) | 1,
                0,
            )
            .map_err(|_| InitError::IoQueueFailed)?;

        let io = Queue::new(
            io_sq,
            io_cq,
            depth,
            registers + 0x1000 + 2 * stride,
            registers + 0x1000 + 3 * stride,
        );
        let transfer = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        Ok(Self {
            namespace_id: 1,
            sectors: namespace_size,
            io,
            transfer,
        })
    }

    fn rw(&mut self, lba: u64, sector: &mut [u8], write: bool) -> Result<(), Error> {
        if sector.len() != SECTOR_SIZE {
            return Err(Error::InvalidBuffer);
        }
        if !lba_in_range(lba, self.sectors) {
            return Err(Error::OutOfBounds);
        }
        if write {
            self.transfer.bytes_mut()[..SECTOR_SIZE].copy_from_slice(sector);
            fence(Ordering::Release);
        }
        self.io
            .command(
                if write { IO_WRITE } else { IO_READ },
                self.namespace_id,
                self.transfer.physical,
                lba as u32,
                (lba >> 32) as u32,
                0,
            )
            .map_err(|_| Error::DeviceFault)?;
        fence(Ordering::Acquire);
        if !write {
            sector.copy_from_slice(&self.transfer.bytes_mut()[..SECTOR_SIZE]);
        }
        Ok(())
    }
}

impl BlockDevice for NvmeController {
    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn flush(&mut self) -> Result<(), Error> {
        self.io
            .command(IO_FLUSH, self.namespace_id, 0, 0, 0, 0)
            .map_err(|_| Error::DeviceFault)
    }

    fn read_sector(&mut self, lba: u64, sector: &mut [u8]) -> Result<(), Error> {
        self.rw(lba, sector, false)
    }

    fn write_sector(&mut self, lba: u64, sector: &[u8]) -> Result<(), Error> {
        if sector.len() != SECTOR_SIZE {
            return Err(Error::InvalidBuffer);
        }
        let mut copy = [0_u8; SECTOR_SIZE];
        copy.copy_from_slice(sector);
        self.rw(lba, &mut copy, true)
    }
}

pub fn init() -> Result<u64, InitError> {
    let mut slot = CONTROLLER.lock();
    if let Some(controller) = slot.as_ref() {
        return Ok(controller.sectors);
    }
    let device = find_controller().ok_or(InitError::MissingController)?;
    let controller = NvmeController::initialize(device)?;
    let sectors = controller.sectors;
    *slot = Some(controller);
    Ok(sectors)
}

pub fn with_controller<R>(operation: impl FnOnce(&mut NvmeController) -> R) -> Option<R> {
    CONTROLLER.lock().as_mut().map(operation)
}

pub fn probe() -> bool {
    find_controller().is_some()
}

fn find_controller() -> Option<pci::Device> {
    for index in 0..64 {
        if let Some(device) = pci::device(index) {
            if device.class == NVME_CLASS
                && device.subclass == NVME_SUBCLASS
                && device.prog_if == NVME_PROG_IF
            {
                return Some(device);
            }
        }
    }
    None
}

pub fn self_test() -> bool {
    let cap = 0x0000_0000_0000_003f_u64;
    let mqes = ((cap & 0xffff) as u16).saturating_add(1);
    mqes == 64
        && completion_phase(1, true)
        && !completion_phase(0, true)
        && lba_in_range(0, 1)
        && !lba_in_range(1, 1)
}

fn completion_phase(status: u16, expected: bool) -> bool {
    (status & 1 != 0) == expected
}

fn lba_in_range(lba: u64, sectors: u64) -> bool {
    lba < sectors
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

fn wait_ready(registers: u64, ready: bool) -> Result<(), InitError> {
    for _ in 0..READY_POLL_LIMIT {
        if (mmio_read32(registers + 0x1c)? & 1 != 0) == ready {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(InitError::ControllerTimeout)
}

fn mmio_read32(physical_address: u64) -> Result<u32, InitError> {
    let page = physical_address & !0xfff;
    let offset = physical_address & 0xfff;
    let mapped = paging::map_mmio(page).map_err(|_| InitError::MmioUnavailable)?;
    // SAFETY: The address belongs to the controller BAR and is mapped uncached.
    Ok(unsafe { ptr::read_volatile((mapped + offset) as *const u32) })
}

fn mmio_write32(physical_address: u64, value: u32) -> Result<(), InitError> {
    let page = physical_address & !0xfff;
    let offset = physical_address & 0xfff;
    let mapped = paging::map_mmio(page).map_err(|_| InitError::MmioUnavailable)?;
    // SAFETY: The address belongs to the controller BAR and is mapped uncached.
    unsafe { ptr::write_volatile((mapped + offset) as *mut u32, value) };
    Ok(())
}

fn mmio_read64(physical_address: u64) -> Result<u64, InitError> {
    let low = u64::from(mmio_read32(physical_address)?);
    let high = u64::from(mmio_read32(physical_address + 4)?);
    Ok(low | (high << 32))
}

fn mmio_write64(physical_address: u64, value: u64) -> Result<(), InitError> {
    mmio_write32(physical_address, value as u32)?;
    mmio_write32(physical_address + 4, (value >> 32) as u32)
}

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

const AHCI_CLASS: u8 = 0x01;
const AHCI_SUBCLASS: u8 = 0x06;
const AHCI_PROG_IF: u8 = 0x01;
const PAGE_SIZE: usize = 4096;
const MAX_PORTS: usize = 32;
const SATA_SIG_ATA: u32 = 0x0000_0101;
const HBA_GHC: u64 = 0x04;
const HBA_PI: u64 = 0x0c;
const HBA_PORT_BASE: u64 = 0x100;
const HBA_PORT_STRIDE: u64 = 0x80;
const PX_CLB: u64 = 0x00;
const PX_CLBU: u64 = 0x04;
const PX_FB: u64 = 0x08;
const PX_FBU: u64 = 0x0c;
const PX_IS: u64 = 0x10;
const PX_IE: u64 = 0x14;
const PX_CMD: u64 = 0x18;
const PX_TFD: u64 = 0x20;
const PX_SIG: u64 = 0x24;
const PX_SSTS: u64 = 0x28;
const PX_SERR: u64 = 0x30;
const PX_SACT: u64 = 0x34;
const PX_CI: u64 = 0x38;
const CMD_ST: u32 = 1;
const CMD_FRE: u32 = 1 << 4;
const CMD_FR: u32 = 1 << 14;
const CMD_CR: u32 = 1 << 15;
const TFD_BSY: u32 = 0x80;
const TFD_DRQ: u32 = 0x08;
const GHC_AE: u32 = 1 << 31;
const FIS_TYPE_REG_H2D: u8 = 0x27;
const ATA_IDENTIFY: u8 = 0xec;
const ATA_READ_DMA_EXT: u8 = 0x25;
const ATA_WRITE_DMA_EXT: u8 = 0x35;
const ATA_FLUSH_CACHE_EXT: u8 = 0xea;
const POLL_LIMIT: usize = 2_000_000;

static CONTROLLER: Mutex<Option<AhciDisk>> = Mutex::with_rank(None, 10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    MissingController,
    MissingAbar,
    MmioUnavailable,
    NoSataPort,
    DmaUnavailable,
    PortTimeout,
    DeviceFault,
    UnsupportedDevice,
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
        // SAFETY: this page is exclusively owned by the AHCI driver.
        unsafe { ptr::write_bytes(virtual_address as *mut u8, 0, PAGE_SIZE) };
        Some(Self { physical, virtual_address })
    }

    fn bytes_mut(&mut self) -> &mut [u8; PAGE_SIZE] {
        // SAFETY: controller access is serialized by CONTROLLER.
        unsafe { &mut *(self.virtual_address as *mut [u8; PAGE_SIZE]) }
    }
}

pub struct AhciDisk {
    port: u64,
    sectors: u64,
    command_list: DmaPage,
    _received_fis: DmaPage,
    command_table: DmaPage,
    transfer: DmaPage,
}

impl AhciDisk {
    fn initialize(device: pci::Device) -> Result<Self, InitError> {
        let abar = device.bars[5];
        if !abar.valid || !matches!(abar.kind, BarKind::Memory32 | BarKind::Memory64) {
            return Err(InitError::MissingAbar);
        }
        let address = Address {
            segment: device.segment,
            bus: device.bus,
            device: device.device,
            function: device.function,
        };
        if !pci::enable_memory_bus_master(address) {
            return Err(InitError::MissingController);
        }
        let hba = abar.address;
        let ghc = mmio_read32(hba + HBA_GHC)?;
        mmio_write32(hba + HBA_GHC, ghc | GHC_AE)?;
        let implemented = mmio_read32(hba + HBA_PI)?;

        let mut selected = None;
        for index in 0..MAX_PORTS {
            if implemented & (1_u32 << index) == 0 {
                continue;
            }
            let port = hba + HBA_PORT_BASE + index as u64 * HBA_PORT_STRIDE;
            let ssts = mmio_read32(port + PX_SSTS)?;
            let det = ssts & 0x0f;
            let ipm = (ssts >> 8) & 0x0f;
            if det != 3 || ipm != 1 {
                continue;
            }
            if mmio_read32(port + PX_SIG)? != SATA_SIG_ATA {
                continue;
            }
            selected = Some(port);
            break;
        }
        let port = selected.ok_or(InitError::NoSataPort)?;

        stop_engine(port)?;
        let command_list = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        let received_fis = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        let command_table = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;
        let transfer = DmaPage::allocate_zeroed().ok_or(InitError::DmaUnavailable)?;

        mmio_write32(port + PX_CLB, command_list.physical as u32)?;
        mmio_write32(port + PX_CLBU, (command_list.physical >> 32) as u32)?;
        mmio_write32(port + PX_FB, received_fis.physical as u32)?;
        mmio_write32(port + PX_FBU, (received_fis.physical >> 32) as u32)?;
        mmio_write32(port + PX_IE, 0)?;
        mmio_write32(port + PX_IS, u32::MAX)?;
        mmio_write32(port + PX_SERR, u32::MAX)?;
        start_engine(port)?;

        let mut disk = Self {
            port,
            sectors: 0,
            command_list,
            _received_fis: received_fis,
            command_table,
            transfer,
        };
        disk.identify()?;
        Ok(disk)
    }

    fn prepare_command(&mut self, write: bool, byte_count: usize) {
        self.command_list.bytes_mut().fill(0);
        self.command_table.bytes_mut().fill(0);

        let header = self.command_list.bytes_mut();
        let cfl = 5_u16;
        let flags = cfl | if write { 1 << 6 } else { 0 };
        header[0..2].copy_from_slice(&flags.to_le_bytes());
        header[2..4].copy_from_slice(&1_u16.to_le_bytes());
        header[8..12].copy_from_slice(&(self.command_table.physical as u32).to_le_bytes());
        header[12..16].copy_from_slice(&((self.command_table.physical >> 32) as u32).to_le_bytes());

        let table = self.command_table.bytes_mut();
        table[0x80..0x84].copy_from_slice(&(self.transfer.physical as u32).to_le_bytes());
        table[0x84..0x88].copy_from_slice(&((self.transfer.physical >> 32) as u32).to_le_bytes());
        table[0x8c..0x90].copy_from_slice(&((byte_count as u32).saturating_sub(1)).to_le_bytes());
    }

    fn issue(&mut self, command: u8, lba: u64, sectors: u16, write: bool, bytes: usize) -> Result<(), InitError> {
        self.prepare_command(write, bytes);
        let table = self.command_table.bytes_mut();
        table[0] = FIS_TYPE_REG_H2D;
        table[1] = 1 << 7;
        table[2] = command;
        table[7] = 1 << 6; // LBA mode
        table[4] = lba as u8;
        table[5] = (lba >> 8) as u8;
        table[6] = (lba >> 16) as u8;
        table[8] = (lba >> 24) as u8;
        table[9] = (lba >> 32) as u8;
        table[10] = (lba >> 40) as u8;
        table[12] = sectors as u8;
        table[13] = (sectors >> 8) as u8;

        wait_tfd_clear(self.port)?;
        mmio_write32(self.port + PX_IS, u32::MAX)?;
        fence(Ordering::Release);
        mmio_write32(self.port + PX_CI, 1)?;
        for _ in 0..POLL_LIMIT {
            fence(Ordering::Acquire);
            if mmio_read32(self.port + PX_CI)? & 1 == 0 {
                let is = mmio_read32(self.port + PX_IS)?;
                if is & (1 << 30) != 0 {
                    return Err(InitError::DeviceFault);
                }
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(InitError::PortTimeout)
    }

    fn identify(&mut self) -> Result<(), InitError> {
        self.transfer.bytes_mut().fill(0);
        self.issue(ATA_IDENTIFY, 0, 0, false, SECTOR_SIZE)?;
        let words = self.transfer.bytes_mut();
        let command_set = u16::from_le_bytes([words[166], words[167]]);
        if command_set & (1 << 10) == 0 {
            return Err(InitError::UnsupportedDevice);
        }
        let sectors = u64::from_le_bytes([
            words[200], words[201], words[202], words[203],
            words[204], words[205], words[206], words[207],
        ]);
        if sectors == 0 {
            return Err(InitError::UnsupportedDevice);
        }
        self.sectors = sectors;
        Ok(())
    }

    fn rw(&mut self, lba: u64, sector: &mut [u8], write: bool) -> Result<(), Error> {
        if sector.len() != SECTOR_SIZE {
            return Err(Error::InvalidBuffer);
        }
        if lba >= self.sectors {
            return Err(Error::OutOfBounds);
        }
        if write {
            self.transfer.bytes_mut()[..SECTOR_SIZE].copy_from_slice(sector);
            fence(Ordering::Release);
        }
        self.issue(
            if write { ATA_WRITE_DMA_EXT } else { ATA_READ_DMA_EXT },
            lba,
            1,
            write,
            SECTOR_SIZE,
        )
        .map_err(|_| Error::DeviceFault)?;
        fence(Ordering::Acquire);
        if !write {
            sector.copy_from_slice(&self.transfer.bytes_mut()[..SECTOR_SIZE]);
        }
        Ok(())
    }
}

impl BlockDevice for AhciDisk {
    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn flush(&mut self) -> Result<(), Error> {
        self.issue(ATA_FLUSH_CACHE_EXT, 0, 0, false, 0)
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

pub fn probe() -> bool {
    find_controller().is_some()
}

pub fn init() -> Result<u64, InitError> {
    let mut slot = CONTROLLER.lock();
    if let Some(disk) = slot.as_ref() {
        return Ok(disk.sectors);
    }
    let device = find_controller().ok_or(InitError::MissingController)?;
    let disk = AhciDisk::initialize(device)?;
    let sectors = disk.sectors;
    *slot = Some(disk);
    Ok(sectors)
}

pub fn with_controller<R>(operation: impl FnOnce(&mut AhciDisk) -> R) -> Option<R> {
    CONTROLLER.lock().as_mut().map(operation)
}

fn find_controller() -> Option<pci::Device> {
    for index in 0..64 {
        if let Some(device) = pci::device(index) {
            if device.class == AHCI_CLASS
                && device.subclass == AHCI_SUBCLASS
                && device.prog_if == AHCI_PROG_IF
            {
                return Some(device);
            }
        }
    }
    None
}

fn stop_engine(port: u64) -> Result<(), InitError> {
    let mut cmd = mmio_read32(port + PX_CMD)?;
    cmd &= !CMD_ST;
    mmio_write32(port + PX_CMD, cmd)?;
    for _ in 0..POLL_LIMIT {
        if mmio_read32(port + PX_CMD)? & CMD_CR == 0 {
            break;
        }
        core::hint::spin_loop();
    }
    cmd = mmio_read32(port + PX_CMD)? & !CMD_FRE;
    mmio_write32(port + PX_CMD, cmd)?;
    for _ in 0..POLL_LIMIT {
        if mmio_read32(port + PX_CMD)? & CMD_FR == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(InitError::PortTimeout)
}

fn start_engine(port: u64) -> Result<(), InitError> {
    for _ in 0..POLL_LIMIT {
        if mmio_read32(port + PX_CMD)? & CMD_CR == 0 {
            let cmd = mmio_read32(port + PX_CMD)? | CMD_FRE | CMD_ST;
            mmio_write32(port + PX_CMD, cmd)?;
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(InitError::PortTimeout)
}

fn wait_tfd_clear(port: u64) -> Result<(), InitError> {
    for _ in 0..POLL_LIMIT {
        let tfd = mmio_read32(port + PX_TFD)?;
        if tfd & (TFD_BSY | TFD_DRQ) == 0 && mmio_read32(port + PX_SACT)? == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(InitError::PortTimeout)
}

fn mmio_read32(physical_address: u64) -> Result<u32, InitError> {
    let page = physical_address & !0xfff;
    let offset = physical_address & 0xfff;
    let mapped = paging::map_mmio(page).map_err(|_| InitError::MmioUnavailable)?;
    // SAFETY: address is an AHCI MMIO register mapped uncached.
    Ok(unsafe { ptr::read_volatile((mapped + offset) as *const u32) })
}

fn mmio_write32(physical_address: u64, value: u32) -> Result<(), InitError> {
    let page = physical_address & !0xfff;
    let offset = physical_address & 0xfff;
    let mapped = paging::map_mmio(page).map_err(|_| InitError::MmioUnavailable)?;
    // SAFETY: address is an AHCI MMIO register mapped uncached.
    unsafe { ptr::write_volatile((mapped + offset) as *mut u32, value) };
    Ok(())
}

pub fn self_test() -> bool {
    sata_signature_valid(SATA_SIG_ATA)
        && !sata_signature_valid(0xeb14_0101)
        && lba_in_range(0, 1)
        && !lba_in_range(1, 1)
}

fn sata_signature_valid(signature: u32) -> bool {
    signature == SATA_SIG_ATA
}

fn lba_in_range(lba: u64, sectors: u64) -> bool {
    lba < sectors
}

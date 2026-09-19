use core::ptr;

use crate::{
    hal::pci::{self, Address, BarKind},
    irq_lock::IrqMutex as Mutex,
    memory, paging,
};

const AUDIO_CLASS: u8 = 0x04;
const HDA_SUBCLASS: u8 = 0x03;
const POLL_LIMIT: usize = 1_000_000;
const REG_GCAP: u64 = 0x00;
const REG_GCTL: u64 = 0x08;
const REG_STATESTS: u64 = 0x0e;
const REG_CORBLBASE: u64 = 0x40;
const REG_CORBUBASE: u64 = 0x44;
const REG_CORBWP: u64 = 0x48;
const REG_CORBRP: u64 = 0x4a;
const REG_CORBCTL: u64 = 0x4c;
const REG_CORBSTS: u64 = 0x4d;
const REG_CORBSIZE: u64 = 0x4e;
const REG_RIRBLBASE: u64 = 0x50;
const REG_RIRBUBASE: u64 = 0x54;
const REG_RIRBWP: u64 = 0x58;
const REG_RINTCNT: u64 = 0x5a;
const REG_RIRBCTL: u64 = 0x5c;
const REG_RIRBSTS: u64 = 0x5d;
const REG_RIRBSIZE: u64 = 0x5e;

static CONTROLLER: Mutex<Option<HdaController>> = Mutex::with_rank(None, 10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    MissingController, MissingMemoryBar, MmioUnavailable, PciEnableFailed,
    InvalidCapabilities, ResetTimeout, DmaUnavailable, UnsupportedRingSize,
    CorbResetTimeout, RingStartFailed, CommandTimeout, CodecResponseError, InvalidCodec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControllerSummary {
    pub vendor_id: u16, pub device_id: u16, pub bus: u8, pub device: u8, pub function: u8,
    pub input_streams: u8, pub output_streams: u8, pub bidirectional_streams: u8,
    pub supports_64bit: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodecSummary {
    pub address: u8, pub vendor_id: u32, pub revision_id: u32,
    pub root_start_node: u8, pub root_node_count: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TopologySummary {
    pub codec_address: u8,
    pub audio_function_groups: u8,
    pub widgets: u16,
    pub audio_outputs: u16,
    pub audio_inputs: u16,
    pub mixers: u16,
    pub selectors: u16,
    pub pin_complexes: u16,
    pub power_widgets: u16,
    pub volume_knobs: u16,
}
struct DmaPage { physical: u64, virtual_address: u64 }
impl DmaPage {
    fn allocate() -> Result<Self, InitError> {
        let frame = memory::allocate_contiguous_frames(1).ok_or(InitError::DmaUnavailable)?;
        let physical = frame.start_address().as_u64();
        let virtual_address = paging::map_mmio(physical).map_err(|_| InitError::MmioUnavailable)?;
        unsafe { ptr::write_bytes(virtual_address as *mut u8, 0, 4096) };
        Ok(Self { physical, virtual_address })
    }
}
pub struct HdaController {
    pci: pci::Device, mmio: u64, input_streams: u8, output_streams: u8,
    bidirectional_streams: u8, supports_64bit: bool,
    corb: Option<DmaPage>, rirb: Option<DmaPage>, rirb_read: u16,
}
impl HdaController {
    fn initialize(device: pci::Device) -> Result<Self, InitError> {
        let bar = device.bars.iter()
            .find(|bar| bar.valid && matches!(bar.kind, BarKind::Memory32 | BarKind::Memory64))
            .ok_or(InitError::MissingMemoryBar)?;
        let address = Address { segment: device.segment, bus: device.bus, device: device.device, function: device.function };
        if !pci::enable_memory_bus_master(address) { return Err(InitError::PciEnableFailed); }
        let gcap = mmio_read16(bar.address + REG_GCAP)?;
        let output_streams = ((gcap >> 12) & 0x0f) as u8;
        let input_streams = ((gcap >> 8) & 0x0f) as u8;
        let bidirectional_streams = ((gcap >> 3) & 0x1f) as u8;
        let supports_64bit = gcap & 1 != 0;
        if output_streams == 0 && input_streams == 0 && bidirectional_streams == 0 {
            return Err(InitError::InvalidCapabilities);
        }
        let mut c = Self { pci: device, mmio: bar.address, input_streams, output_streams,
            bidirectional_streams, supports_64bit, corb: None, rirb: None, rirb_read: 0 };
        c.reset()?;
        c.setup_command_rings()?;
        Ok(c)
    }
    fn reset(&mut self) -> Result<(), InitError> {
        let gctl = self.mmio + REG_GCTL;
        let current = mmio_read32(gctl)?;
        mmio_write32(gctl, current & !1)?;
        wait_for_bit(gctl, 1, false)?;
        mmio_write32(gctl, current | 1)?;
        wait_for_bit(gctl, 1, true)
    }
    fn setup_command_rings(&mut self) -> Result<(), InitError> {
        mmio_write8(self.mmio + REG_CORBCTL, 0)?;
        mmio_write8(self.mmio + REG_RIRBCTL, 0)?;
        let corb_caps = mmio_read8(self.mmio + REG_CORBSIZE)?;
        let rirb_caps = mmio_read8(self.mmio + REG_RIRBSIZE)?;
        if corb_caps & 0x40 == 0 || rirb_caps & 0x40 == 0 { return Err(InitError::UnsupportedRingSize); }
        mmio_write8(self.mmio + REG_CORBSIZE, (corb_caps & !3) | 2)?;
        mmio_write8(self.mmio + REG_RIRBSIZE, (rirb_caps & !3) | 2)?;
        let corb = DmaPage::allocate()?;
        let rirb = DmaPage::allocate()?;
        mmio_write32(self.mmio + REG_CORBLBASE, corb.physical as u32)?;
        mmio_write32(self.mmio + REG_CORBUBASE, (corb.physical >> 32) as u32)?;
        mmio_write32(self.mmio + REG_RIRBLBASE, rirb.physical as u32)?;
        mmio_write32(self.mmio + REG_RIRBUBASE, (rirb.physical >> 32) as u32)?;
        mmio_write16(self.mmio + REG_CORBWP, 0)?;
        mmio_write16(self.mmio + REG_CORBRP, 0x8000)?;
        for _ in 0..POLL_LIMIT {
            if mmio_read16(self.mmio + REG_CORBRP)? & 0x8000 != 0 { break; }
            core::hint::spin_loop();
        }
        if mmio_read16(self.mmio + REG_CORBRP)? & 0x8000 == 0 { return Err(InitError::CorbResetTimeout); }
        mmio_write16(self.mmio + REG_CORBRP, 0)?;
        for _ in 0..POLL_LIMIT {
            if mmio_read16(self.mmio + REG_CORBRP)? & 0x8000 == 0 { break; }
            core::hint::spin_loop();
        }
        if mmio_read16(self.mmio + REG_CORBRP)? & 0x8000 != 0 { return Err(InitError::CorbResetTimeout); }
        mmio_write16(self.mmio + REG_RIRBWP, 0x8000)?;
        mmio_write16(self.mmio + REG_RINTCNT, 0xff)?;
        mmio_write8(self.mmio + REG_CORBSTS, 0xff)?;
        mmio_write8(self.mmio + REG_RIRBSTS, 0xff)?;
        mmio_write8(self.mmio + REG_RIRBCTL, 0x02)?;
        mmio_write8(self.mmio + REG_CORBCTL, 0x02)?;
        if mmio_read8(self.mmio + REG_CORBCTL)? & 2 == 0 || mmio_read8(self.mmio + REG_RIRBCTL)? & 2 == 0 {
            return Err(InitError::RingStartFailed);
        }
        self.corb = Some(corb); self.rirb = Some(rirb); self.rirb_read = 0; Ok(())
    }
    fn command(&mut self, codec: u8, node: u8, verb: u16, payload: u8) -> Result<u32, InitError> {
        if codec > 0x0f { return Err(InitError::InvalidCodec); }
        let command = (u32::from(codec) << 28) | (u32::from(node) << 20)
            | (u32::from(verb & 0x0fff) << 8) | u32::from(payload);
        let corb = self.corb.as_ref().ok_or(InitError::DmaUnavailable)?;
        let rirb = self.rirb.as_ref().ok_or(InitError::DmaUnavailable)?;
        let current_wp = mmio_read16(self.mmio + REG_CORBWP)? & 0xff;
        let next_wp = current_wp.wrapping_add(1) & 0xff;
        unsafe { ptr::write_volatile((corb.virtual_address as *mut u32).add(next_wp as usize), command); }
        mmio_write16(self.mmio + REG_CORBWP, next_wp)?;
        for _ in 0..POLL_LIMIT {
            let hardware_wp = mmio_read16(self.mmio + REG_RIRBWP)? & 0xff;
            if hardware_wp != self.rirb_read {
                let next = self.rirb_read.wrapping_add(1) & 0xff;
                let entry = unsafe { ptr::read_volatile((rirb.virtual_address as *const u64).add(next as usize)) };
                self.rirb_read = next;
                let response_ex = (entry >> 32) as u32;
                if response_ex & (1 << 4) != 0 { continue; }
                if (response_ex & 0x0f) as u8 != codec { return Err(InitError::CodecResponseError); }
                return Ok(entry as u32);
            }
            core::hint::spin_loop();
        }
        Err(InitError::CommandTimeout)
    }
    fn codec_summary(&mut self) -> Result<CodecSummary, InitError> {
        let state = mmio_read16(self.mmio + REG_STATESTS)? & 0x7fff;
        let address = (0u8..15).find(|codec| state & (1u16 << codec) != 0).ok_or(InitError::InvalidCodec)?;
        let vendor_id = self.command(address, 0, 0x0f00, 0)?;
        let revision_id = self.command(address, 0, 0x0f00, 2)?;
        let nodes = self.command(address, 0, 0x0f00, 4)?;
        Ok(CodecSummary { address, vendor_id, revision_id,
            root_start_node: ((nodes >> 16) & 0xff) as u8, root_node_count: (nodes & 0xff) as u8 })
    }

    fn topology_summary(&mut self) -> Result<TopologySummary, InitError> {
        let codec = self.codec_summary()?;
        let mut summary = TopologySummary {
            codec_address: codec.address,
            ..TopologySummary::default()
        };

        for fg_offset in 0..codec.root_node_count {
            let node = codec.root_start_node.wrapping_add(fg_offset);
            let function_type = self.command(codec.address, node, 0x0f00, 0x05)?;
            if function_type & 0xff != 0x01 {
                continue;
            }
            summary.audio_function_groups = summary.audio_function_groups.saturating_add(1);

            let widgets = self.command(codec.address, node, 0x0f00, 0x04)?;
            let widget_start = ((widgets >> 16) & 0xff) as u8;
            let widget_count = (widgets & 0xff) as u8;

            for widget_offset in 0..widget_count {
                let widget_node = widget_start.wrapping_add(widget_offset);
                let caps = self.command(codec.address, widget_node, 0x0f00, 0x09)?;
                let widget_type = ((caps >> 20) & 0x0f) as u8;
                summary.widgets = summary.widgets.saturating_add(1);
                match widget_type {
                    0x0 => summary.audio_outputs = summary.audio_outputs.saturating_add(1),
                    0x1 => summary.audio_inputs = summary.audio_inputs.saturating_add(1),
                    0x2 => summary.mixers = summary.mixers.saturating_add(1),
                    0x3 => summary.selectors = summary.selectors.saturating_add(1),
                    0x4 => summary.pin_complexes = summary.pin_complexes.saturating_add(1),
                    0x5 => summary.power_widgets = summary.power_widgets.saturating_add(1),
                    0x6 => summary.volume_knobs = summary.volume_knobs.saturating_add(1),
                    _ => {}
                }
            }
        }

        if summary.audio_function_groups == 0 || summary.widgets == 0 {
            return Err(InitError::CodecResponseError);
        }
        Ok(summary)
    }
    pub fn summary(&self) -> ControllerSummary {
        ControllerSummary { vendor_id:self.pci.vendor_id, device_id:self.pci.device_id, bus:self.pci.bus,
            device:self.pci.device, function:self.pci.function, input_streams:self.input_streams,
            output_streams:self.output_streams, bidirectional_streams:self.bidirectional_streams,
            supports_64bit:self.supports_64bit }
    }
}
pub fn probe() -> bool { find_controller().is_some() }
pub fn init() -> Result<ControllerSummary, InitError> {
    let mut slot=CONTROLLER.lock();
    if let Some(c)=slot.as_ref(){return Ok(c.summary());}
    let device=find_controller().ok_or(InitError::MissingController)?;
    let c=HdaController::initialize(device)?; let summary=c.summary(); *slot=Some(c); Ok(summary)
}
pub fn discover_codec() -> Result<CodecSummary, InitError> {
    CONTROLLER.lock().as_mut().ok_or(InitError::MissingController)?.codec_summary()
}

pub fn discover_topology() -> Result<TopologySummary, InitError> {
    CONTROLLER.lock().as_mut().ok_or(InitError::MissingController)?.topology_summary()
}
fn find_controller()->Option<pci::Device>{
    for i in 0..64 { if let Some(d)=pci::device(i) { if d.class==AUDIO_CLASS && d.subclass==HDA_SUBCLASS{return Some(d);} } } None
}
fn wait_for_bit(a:u64,m:u32,s:bool)->Result<(),InitError>{
    for _ in 0..POLL_LIMIT { let v=mmio_read32(a)?; if (v&m!=0)==s{return Ok(());} core::hint::spin_loop(); } Err(InitError::ResetTimeout)
}
fn mmio_read8(p:u64)->Result<u8,InitError>{let v=paging::map_mmio(p).map_err(|_|InitError::MmioUnavailable)?;Ok(unsafe{ptr::read_volatile(v as *const u8)})}
fn mmio_read16(p:u64)->Result<u16,InitError>{let v=paging::map_mmio(p).map_err(|_|InitError::MmioUnavailable)?;Ok(unsafe{ptr::read_volatile(v as *const u16)})}
fn mmio_read32(p:u64)->Result<u32,InitError>{let v=paging::map_mmio(p).map_err(|_|InitError::MmioUnavailable)?;Ok(unsafe{ptr::read_volatile(v as *const u32)})}
fn mmio_write8(p:u64,x:u8)->Result<(),InitError>{let v=paging::map_mmio(p).map_err(|_|InitError::MmioUnavailable)?;unsafe{ptr::write_volatile(v as *mut u8,x)};Ok(())}
fn mmio_write16(p:u64,x:u16)->Result<(),InitError>{let v=paging::map_mmio(p).map_err(|_|InitError::MmioUnavailable)?;unsafe{ptr::write_volatile(v as *mut u16,x)};Ok(())}
fn mmio_write32(p:u64,x:u32)->Result<(),InitError>{let v=paging::map_mmio(p).map_err(|_|InitError::MmioUnavailable)?;unsafe{ptr::write_volatile(v as *mut u32,x)};Ok(())}
pub fn self_test()->bool{
    let gcap:u16=(4<<12)|(2<<8)|(1<<3)|1;
    let command=(2u32<<28)|(3u32<<20)|(0x0f00u32<<8)|4;
    ((gcap>>12)&15)==4 && ((gcap>>8)&15)==2 && ((gcap>>3)&31)==1 && gcap&1!=0 && command==0x203f0004
}

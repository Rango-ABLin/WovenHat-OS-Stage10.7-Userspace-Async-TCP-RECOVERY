use core::{
    ptr,
    sync::atomic::{compiler_fence, Ordering},
};

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
    MissingController,
    MissingMemoryBar,
    MmioUnavailable,
    PciEnableFailed,
    InvalidCapabilities,
    ResetTimeout,
    DmaUnavailable,
    UnsupportedRingSize,
    CorbResetTimeout,
    RingStartFailed,
    CommandTimeout,
    CodecResponseError,
    InvalidCodec,
    MissingOutputConverter,
    MissingInputConverter,
    MissingOutputPin,
    MissingAmpControl,
    AmpStateMismatch,
    StreamResetTimeout,
    StreamStartFailed,
    StreamNoProgress,
    CaptureBufferUnchanged,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodecSummary {
    pub address: u8,
    pub vendor_id: u32,
    pub revision_id: u32,
    pub root_start_node: u8,
    pub root_node_count: u8,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaybackSummary {
    pub stream_index: u8,
    pub stream_tag: u8,
    pub converter_node: u8,
    pub format: u16,
    pub bytes: u32,
    pub position_before: u32,
    pub position_after: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MixerSummary {
    pub codec_address: u8,
    pub converter_node: u8,
    pub pin_node: u8,
    pub amp_node: u8,
    pub gain_steps: u8,
    pub offset: u8,
    pub step_size: u8,
    pub mute_supported: bool,
    pub test_gain: u8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureSummary {
    pub stream_index: u8,
    pub stream_tag: u8,
    pub converter_node: u8,
    pub format: u16,
    pub bytes: u32,
    pub position_before: u32,
    pub position_after: u32,
    pub changed_bytes: u32,
}
struct DmaPage {
    physical: u64,
    virtual_address: u64,
}
impl DmaPage {
    fn allocate() -> Result<Self, InitError> {
        let frame = memory::allocate_contiguous_frames(1).ok_or(InitError::DmaUnavailable)?;
        let physical = frame.start_address().as_u64();
        let virtual_address = paging::map_mmio(physical).map_err(|_| InitError::MmioUnavailable)?;
        unsafe { ptr::write_bytes(virtual_address as *mut u8, 0, 4096) };
        Ok(Self {
            physical,
            virtual_address,
        })
    }
}
pub struct HdaController {
    pci: pci::Device,
    mmio: u64,
    input_streams: u8,
    output_streams: u8,
    bidirectional_streams: u8,
    supports_64bit: bool,
    corb: Option<DmaPage>,
    rirb: Option<DmaPage>,
    rirb_read: u16,
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
        let gcap = mmio_read16(bar.address + REG_GCAP)?;
        let output_streams = ((gcap >> 12) & 0x0f) as u8;
        let input_streams = ((gcap >> 8) & 0x0f) as u8;
        let bidirectional_streams = ((gcap >> 3) & 0x1f) as u8;
        let supports_64bit = gcap & 1 != 0;
        if output_streams == 0 && input_streams == 0 && bidirectional_streams == 0 {
            return Err(InitError::InvalidCapabilities);
        }
        let mut c = Self {
            pci: device,
            mmio: bar.address,
            input_streams,
            output_streams,
            bidirectional_streams,
            supports_64bit,
            corb: None,
            rirb: None,
            rirb_read: 0,
        };
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
        if corb_caps & 0x40 == 0 || rirb_caps & 0x40 == 0 {
            return Err(InitError::UnsupportedRingSize);
        }
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
            if mmio_read16(self.mmio + REG_CORBRP)? & 0x8000 != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        if mmio_read16(self.mmio + REG_CORBRP)? & 0x8000 == 0 {
            return Err(InitError::CorbResetTimeout);
        }
        mmio_write16(self.mmio + REG_CORBRP, 0)?;
        for _ in 0..POLL_LIMIT {
            if mmio_read16(self.mmio + REG_CORBRP)? & 0x8000 == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        if mmio_read16(self.mmio + REG_CORBRP)? & 0x8000 != 0 {
            return Err(InitError::CorbResetTimeout);
        }
        mmio_write16(self.mmio + REG_RIRBWP, 0x8000)?;
        mmio_write16(self.mmio + REG_RINTCNT, 0xff)?;
        mmio_write8(self.mmio + REG_CORBSTS, 0xff)?;
        mmio_write8(self.mmio + REG_RIRBSTS, 0xff)?;
        mmio_write8(self.mmio + REG_RIRBCTL, 0x02)?;
        mmio_write8(self.mmio + REG_CORBCTL, 0x02)?;
        if mmio_read8(self.mmio + REG_CORBCTL)? & 2 == 0
            || mmio_read8(self.mmio + REG_RIRBCTL)? & 2 == 0
        {
            return Err(InitError::RingStartFailed);
        }
        self.corb = Some(corb);
        self.rirb = Some(rirb);
        self.rirb_read = 0;
        Ok(())
    }
    fn command(&mut self, codec: u8, node: u8, verb: u16, payload: u8) -> Result<u32, InitError> {
        if codec > 0x0f {
            return Err(InitError::InvalidCodec);
        }
        let command = (u32::from(codec) << 28)
            | (u32::from(node) << 20)
            | (u32::from(verb & 0x0fff) << 8)
            | u32::from(payload);
        let corb = self.corb.as_ref().ok_or(InitError::DmaUnavailable)?;
        let rirb = self.rirb.as_ref().ok_or(InitError::DmaUnavailable)?;
        let current_wp = mmio_read16(self.mmio + REG_CORBWP)? & 0xff;
        let next_wp = current_wp.wrapping_add(1) & 0xff;
        unsafe {
            ptr::write_volatile(
                (corb.virtual_address as *mut u32).add(next_wp as usize),
                command,
            );
        }
        mmio_write16(self.mmio + REG_CORBWP, next_wp)?;
        for _ in 0..POLL_LIMIT {
            let hardware_wp = mmio_read16(self.mmio + REG_RIRBWP)? & 0xff;
            if hardware_wp != self.rirb_read {
                let next = self.rirb_read.wrapping_add(1) & 0xff;
                let entry = unsafe {
                    ptr::read_volatile((rirb.virtual_address as *const u64).add(next as usize))
                };
                self.rirb_read = next;
                let response_ex = (entry >> 32) as u32;
                if response_ex & (1 << 4) != 0 {
                    continue;
                }
                if (response_ex & 0x0f) as u8 != codec {
                    return Err(InitError::CodecResponseError);
                }
                return Ok(entry as u32);
            }
            core::hint::spin_loop();
        }
        Err(InitError::CommandTimeout)
    }
    fn command16(&mut self, codec: u8, node: u8, verb: u8, payload: u16) -> Result<u32, InitError> {
        if codec > 0x0f || verb > 0x0f {
            return Err(InitError::InvalidCodec);
        }
        let command = (u32::from(codec) << 28)
            | (u32::from(node) << 20)
            | (u32::from(verb) << 16)
            | u32::from(payload);
        let corb = self.corb.as_ref().ok_or(InitError::DmaUnavailable)?;
        let rirb = self.rirb.as_ref().ok_or(InitError::DmaUnavailable)?;
        let current_wp = mmio_read16(self.mmio + REG_CORBWP)? & 0xff;
        let next_wp = current_wp.wrapping_add(1) & 0xff;
        unsafe {
            ptr::write_volatile(
                (corb.virtual_address as *mut u32).add(next_wp as usize),
                command,
            );
        }
        mmio_write16(self.mmio + REG_CORBWP, next_wp)?;
        for _ in 0..POLL_LIMIT {
            let hardware_wp = mmio_read16(self.mmio + REG_RIRBWP)? & 0xff;
            if hardware_wp != self.rirb_read {
                let next = self.rirb_read.wrapping_add(1) & 0xff;
                let entry = unsafe {
                    ptr::read_volatile((rirb.virtual_address as *const u64).add(next as usize))
                };
                self.rirb_read = next;
                let response_ex = (entry >> 32) as u32;
                if response_ex & (1 << 4) != 0 {
                    continue;
                }
                if (response_ex & 0x0f) as u8 != codec {
                    return Err(InitError::CodecResponseError);
                }
                return Ok(entry as u32);
            }
            core::hint::spin_loop();
        }
        Err(InitError::CommandTimeout)
    }
    fn codec_summary(&mut self) -> Result<CodecSummary, InitError> {
        let state = mmio_read16(self.mmio + REG_STATESTS)? & 0x7fff;
        let address = (0u8..15)
            .find(|codec| state & (1u16 << codec) != 0)
            .ok_or(InitError::InvalidCodec)?;
        let vendor_id = self.command(address, 0, 0x0f00, 0)?;
        let revision_id = self.command(address, 0, 0x0f00, 2)?;
        let nodes = self.command(address, 0, 0x0f00, 4)?;
        Ok(CodecSummary {
            address,
            vendor_id,
            revision_id,
            root_start_node: ((nodes >> 16) & 0xff) as u8,
            root_node_count: (nodes & 0xff) as u8,
        })
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
    fn first_output_converter(&mut self) -> Result<(u8, u8), InitError> {
        let codec = self.codec_summary()?;
        for fg_offset in 0..codec.root_node_count {
            let fg = codec.root_start_node.wrapping_add(fg_offset);
            let function_type = self.command(codec.address, fg, 0x0f00, 0x05)?;
            if function_type & 0xff != 0x01 {
                continue;
            }
            let widgets = self.command(codec.address, fg, 0x0f00, 0x04)?;
            let widget_start = ((widgets >> 16) & 0xff) as u8;
            let widget_count = (widgets & 0xff) as u8;
            for widget_offset in 0..widget_count {
                let node = widget_start.wrapping_add(widget_offset);
                let caps = self.command(codec.address, node, 0x0f00, 0x09)?;
                if ((caps >> 20) & 0x0f) == 0 {
                    return Ok((codec.address, node));
                }
            }
        }
        Err(InitError::MissingOutputConverter)
    }

    fn first_input_converter(&mut self) -> Result<(u8, u8), InitError> {
        let codec = self.codec_summary()?;
        for fg_offset in 0..codec.root_node_count {
            let fg = codec.root_start_node.wrapping_add(fg_offset);
            let function_type = self.command(codec.address, fg, 0x0f00, 0x05)?;
            if function_type & 0xff != 0x01 {
                continue;
            }
            let widgets = self.command(codec.address, fg, 0x0f00, 0x04)?;
            let widget_start = ((widgets >> 16) & 0xff) as u8;
            let widget_count = (widgets & 0xff) as u8;
            for widget_offset in 0..widget_count {
                let node = widget_start.wrapping_add(widget_offset);
                let caps = self.command(codec.address, node, 0x0f00, 0x09)?;
                if ((caps >> 20) & 0x0f) == 1 {
                    return Ok((codec.address, node));
                }
            }
        }
        Err(InitError::MissingInputConverter)
    }
    fn output_route(&mut self) -> Result<(u8, u8, u8), InitError> {
        let codec = self.codec_summary()?;
        let (_, converter) = self.first_output_converter()?;
        let mut output_pins = [0u8; 16];
        let mut pin_count = 0usize;
        for fg_offset in 0..codec.root_node_count {
            let fg = codec.root_start_node.wrapping_add(fg_offset);
            if self.command(codec.address, fg, 0x0f00, 0x05)? & 0xff != 1 {
                continue;
            }
            let widgets = self.command(codec.address, fg, 0x0f00, 0x04)?;
            let first = ((widgets >> 16) & 0xff) as u8;
            let count = (widgets & 0xff) as u8;
            for off in 0..count {
                let node = first.wrapping_add(off);
                let caps = self.command(codec.address, node, 0x0f00, 0x09)?;
                if ((caps >> 20) & 0x0f) == 4 {
                    let pin_caps = self.command(codec.address, node, 0x0f00, 0x0c)?;
                    if pin_caps & (1 << 4) != 0 && pin_count < output_pins.len() {
                        output_pins[pin_count] = node;
                        pin_count += 1;
                    }
                }
            }
        }
        for pin in output_pins[..pin_count].iter().copied() {
            let ci = self.command(codec.address, pin, 0x0f00, 0x0e)?;
            let long_form = ci & 0x80 != 0;
            let count = (ci & 0x7f) as usize;
            let packed = if long_form { 2usize } else { 4usize };
            let mut logical = 0usize;
            let mut request = 0usize;
            while logical < count {
                let response = self.command(codec.address, pin, 0x0f02, request as u8)?;
                for slot in 0..packed {
                    if logical >= count {
                        break;
                    }
                    let raw = if long_form {
                        ((response >> (slot * 16)) & 0xffff) as u16
                    } else {
                        ((response >> (slot * 8)) & 0xff) as u16
                    };
                    let node = if long_form { raw & 0x7fff } else { raw & 0x7f };
                    if node == u16::from(converter) {
                        return Ok((codec.address, converter, pin));
                    }
                    logical += 1;
                }
                request += packed;
            }
        }
        Err(InitError::MissingOutputPin)
    }

    fn mixer_smoke_test(&mut self) -> Result<MixerSummary, InitError> {
        let (codec, converter, pin) = self.output_route()?;
        let converter_caps = self.command(codec, converter, 0x0f00, 0x09)?;
        let pin_caps = self.command(codec, pin, 0x0f00, 0x09)?;
        let amp_node = if converter_caps & (1 << 2) != 0 {
            converter
        } else if pin_caps & (1 << 2) != 0 {
            pin
        } else {
            return Err(InitError::MissingAmpControl);
        };
        let caps = self.command(codec, amp_node, 0x0f00, 0x12)?;
        let mute_supported = caps & (1 << 31) != 0;
        let gain_steps = ((caps >> 8) & 0x7f) as u8;
        let offset = (caps & 0x7f) as u8;
        let step_size = ((caps >> 16) & 0x7f) as u8;
        let test_gain = if gain_steps == 0 { 0 } else { gain_steps / 2 };

        // Set Amplifier Gain/Mute: output amp, both channels.
        let set_payload = 0xb000u16 | u16::from(test_gain);
        let _ = self.command16(codec, amp_node, 0x03, set_payload)?;
        let left = self.command(codec, amp_node, 0x0b00, 0x00)?;
        if (left as u8 & 0x7f) != test_gain {
            return Err(InitError::AmpStateMismatch);
        }

        if mute_supported {
            let _ = self.command16(codec, amp_node, 0x03, set_payload | 0x80)?;
            let muted = self.command(codec, amp_node, 0x0b00, 0x00)?;
            if muted as u8 & 0x80 == 0 {
                return Err(InitError::AmpStateMismatch);
            }
            let _ = self.command16(codec, amp_node, 0x03, set_payload)?;
            let unmuted = self.command(codec, amp_node, 0x0b00, 0x00)?;
            if unmuted as u8 & 0x80 != 0 {
                return Err(InitError::AmpStateMismatch);
            }
        }

        // Explicitly enable the discovered output pin.
        let _ = self.command(codec, pin, 0x0707, 0x40)?;
        Ok(MixerSummary {
            codec_address: codec,
            converter_node: converter,
            pin_node: pin,
            amp_node,
            gain_steps,
            offset,
            step_size,
            mute_supported,
            test_gain,
        })
    }
    fn playback_smoke_test(&mut self) -> Result<PlaybackSummary, InitError> {
        const SD_BASE: u64 = 0x80;
        const SD_STRIDE: u64 = 0x20;
        const SD_CTL: u64 = 0x00;
        const SD_STS: u64 = 0x03;
        const SD_LPIB: u64 = 0x04;
        const SD_CBL: u64 = 0x08;
        const SD_LVI: u64 = 0x0c;
        const SD_FMT: u64 = 0x12;
        const SD_BDPL: u64 = 0x18;
        const SD_BDPU: u64 = 0x1c;

        const STREAM_TAG: u8 = 1;
        const PCM_BYTES: u32 = 4096;
        const PCM_FORMAT: u16 = 0x0011;

        let (codec, converter) = self.first_output_converter()?;
        if self.output_streams == 0 {
            return Err(InitError::MissingOutputConverter);
        }

        let bdl = DmaPage::allocate()?;
        let pcm = DmaPage::allocate()?;

        let samples = PCM_BYTES as usize / 2;
        for index in 0..samples {
            let sample: i16 = if (index / 48) % 2 == 0 {
                0x1800
            } else {
                -0x1800
            };
            unsafe {
                ptr::write_volatile((pcm.virtual_address as *mut i16).add(index), sample);
            }
        }

        unsafe {
            ptr::write_volatile(bdl.virtual_address as *mut u64, pcm.physical);
            ptr::write_volatile((bdl.virtual_address + 8) as *mut u32, PCM_BYTES);
            ptr::write_volatile((bdl.virtual_address + 12) as *mut u32, 1);
        }

        compiler_fence(Ordering::Release);

        let descriptor_index = u64::from(self.input_streams);
        let stream = self.mmio + SD_BASE + descriptor_index * SD_STRIDE;

        let ctl = mmio_read32(stream + SD_CTL)?;
        mmio_write32(stream + SD_CTL, ctl & !(1 << 1))?;
        mmio_write32(stream + SD_CTL, (ctl & !(1 << 1)) | 1)?;
        for _ in 0..POLL_LIMIT {
            if mmio_read32(stream + SD_CTL)? & 1 != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        if mmio_read32(stream + SD_CTL)? & 1 == 0 {
            return Err(InitError::StreamResetTimeout);
        }
        mmio_write32(stream + SD_CTL, ctl & !1)?;
        for _ in 0..POLL_LIMIT {
            if mmio_read32(stream + SD_CTL)? & 1 == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        if mmio_read32(stream + SD_CTL)? & 1 != 0 {
            return Err(InitError::StreamResetTimeout);
        }

        mmio_write8(stream + SD_STS, 0x1c)?;
        mmio_write32(stream + SD_CBL, PCM_BYTES)?;
        mmio_write16(stream + SD_LVI, 0)?;
        mmio_write16(stream + SD_FMT, PCM_FORMAT)?;
        mmio_write32(stream + SD_BDPL, bdl.physical as u32)?;
        mmio_write32(stream + SD_BDPU, (bdl.physical >> 32) as u32)?;

        let _ = self.command16(codec, converter, 0x02, PCM_FORMAT)?;
        let _ = self.command(codec, converter, 0x0706, STREAM_TAG << 4)?;

        let position_before = mmio_read32(stream + SD_LPIB)?;
        let run_ctl = (u32::from(STREAM_TAG) << 20) | (1 << 1);
        mmio_write32(stream + SD_CTL, run_ctl)?;
        if mmio_read32(stream + SD_CTL)? & (1 << 1) == 0 {
            return Err(InitError::StreamStartFailed);
        }

        let mut position_after = position_before;
        for _ in 0..POLL_LIMIT {
            position_after = mmio_read32(stream + SD_LPIB)?;
            if position_after != position_before {
                break;
            }
            core::hint::spin_loop();
        }

        mmio_write32(stream + SD_CTL, run_ctl & !(1 << 1))?;

        if position_after == position_before {
            return Err(InitError::StreamNoProgress);
        }

        Ok(PlaybackSummary {
            stream_index: descriptor_index as u8,
            stream_tag: STREAM_TAG,
            converter_node: converter,
            format: PCM_FORMAT,
            bytes: PCM_BYTES,
            position_before,
            position_after,
        })
    }
    fn capture_smoke_test(&mut self) -> Result<CaptureSummary, InitError> {
        const SD_BASE: u64 = 0x80;
        const SD_CTL: u64 = 0;
        const SD_STS: u64 = 3;
        const SD_LPIB: u64 = 4;
        const SD_CBL: u64 = 8;
        const SD_LVI: u64 = 0x0c;
        const SD_FMT: u64 = 0x12;
        const SD_BDPL: u64 = 0x18;
        const SD_BDPU: u64 = 0x1c;
        const STREAM_TAG: u8 = 2;
        const PCM_BYTES: u32 = 4096;
        const PCM_FORMAT: u16 = 0x0011;
        const SENTINEL: u8 = 0xa5;

        let codec = self.codec_summary()?;
        let mut adc = None;
        let mut input_pins = [0u8; 16];
        let mut input_pin_count = 0usize;
        for fg_offset in 0..codec.root_node_count {
            let fg = codec.root_start_node.wrapping_add(fg_offset);
            if self.command(codec.address, fg, 0x0f00, 0x05)? & 0xff != 1 {
                continue;
            }
            let widgets = self.command(codec.address, fg, 0x0f00, 0x04)?;
            let first = ((widgets >> 16) & 0xff) as u8;
            let count = (widgets & 0xff) as u8;
            for off in 0..count {
                let node = first.wrapping_add(off);
                let caps = self.command(codec.address, node, 0x0f00, 0x09)?;
                match ((caps >> 20) & 0x0f) as u8 {
                    1 if adc.is_none() => adc = Some(node),
                    4 if input_pin_count < input_pins.len() => {
                        let pc = self.command(codec.address, node, 0x0f00, 0x0c)?;
                        if pc & (1 << 5) != 0 {
                            input_pins[input_pin_count] = node;
                            input_pin_count += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
        let converter = adc.ok_or(InitError::MissingInputConverter)?;
        if self.input_streams == 0 {
            return Err(InitError::MissingInputConverter);
        }

        let ci = self.command(codec.address, converter, 0x0f00, 0x0e)?;
        let long_form = ci & 0x80 != 0;
        let conn_count = (ci & 0x7f) as usize;
        if conn_count == 0 {
            return Err(InitError::CodecResponseError);
        }
        let mut selected_pin = None;
        let mut selected_index = None;
        let mut logical = 0usize;
        let mut request = 0usize;
        let mut previous: Option<u16> = None;
        while logical < conn_count {
            let response = self.command(codec.address, converter, 0x0f02, request as u8)?;
            let packed = if long_form { 2usize } else { 4usize };
            for slot in 0..packed {
                if logical >= conn_count {
                    break;
                }
                let raw = if long_form {
                    ((response >> (slot * 16)) & 0xffff) as u16
                } else {
                    ((response >> (slot * 8)) & 0xff) as u16
                };
                let range = if long_form {
                    raw & 0x8000 != 0
                } else {
                    raw & 0x80 != 0
                };
                let node = if long_form { raw & 0x7fff } else { raw & 0x7f };
                if range {
                    if let Some(prev) = previous {
                        let mut n = prev.saturating_add(1);
                        while n <= node {
                            for pin in input_pins[..input_pin_count].iter().copied() {
                                if u16::from(pin) == n && selected_pin.is_none() {
                                    selected_pin = Some(pin);
                                    selected_index = Some(logical as u8);
                                }
                            }
                            if n == u16::MAX {
                                break;
                            }
                            n += 1;
                        }
                    }
                } else {
                    for pin in input_pins[..input_pin_count].iter().copied() {
                        if u16::from(pin) == node && selected_pin.is_none() {
                            selected_pin = Some(pin);
                            selected_index = Some(logical as u8);
                        }
                    }
                }
                previous = Some(node);
                logical += 1;
            }
            request += packed;
        }
        let input_pin = selected_pin.ok_or(InitError::CodecResponseError)?;
        let connection_index = selected_index.ok_or(InitError::CodecResponseError)?;

        let bdl = DmaPage::allocate()?;
        let pcm = DmaPage::allocate()?;
        unsafe {
            ptr::write_bytes(pcm.virtual_address as *mut u8, SENTINEL, PCM_BYTES as usize);
            ptr::write_volatile(bdl.virtual_address as *mut u64, pcm.physical);
            ptr::write_volatile((bdl.virtual_address + 8) as *mut u32, PCM_BYTES);
            ptr::write_volatile((bdl.virtual_address + 12) as *mut u32, 1);
        }
        compiler_fence(Ordering::Release);
        let stream = self.mmio + SD_BASE;
        let ctl = mmio_read32(stream + SD_CTL)?;
        mmio_write32(stream + SD_CTL, ctl & !(1 << 1))?;
        mmio_write32(stream + SD_CTL, (ctl & !(1 << 1)) | 1)?;
        for _ in 0..POLL_LIMIT {
            if mmio_read32(stream + SD_CTL)? & 1 != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        if mmio_read32(stream + SD_CTL)? & 1 == 0 {
            return Err(InitError::StreamResetTimeout);
        }
        mmio_write32(stream + SD_CTL, ctl & !1)?;
        for _ in 0..POLL_LIMIT {
            if mmio_read32(stream + SD_CTL)? & 1 == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        if mmio_read32(stream + SD_CTL)? & 1 != 0 {
            return Err(InitError::StreamResetTimeout);
        }

        mmio_write8(stream + SD_STS, 0x1c)?;
        mmio_write32(stream + SD_CBL, PCM_BYTES)?;
        mmio_write16(stream + SD_LVI, 0)?;
        mmio_write16(stream + SD_FMT, PCM_FORMAT)?;
        mmio_write32(stream + SD_BDPL, bdl.physical as u32)?;
        mmio_write32(stream + SD_BDPU, (bdl.physical >> 32) as u32)?;

        let _ = self.command(codec.address, input_pin, 0x0707, 0x20)?;
        let _ = self.command(codec.address, converter, 0x0701, connection_index)?;
        let _ = self.command16(codec.address, converter, 0x02, PCM_FORMAT)?;
        let _ = self.command(codec.address, converter, 0x0706, STREAM_TAG << 4)?;

        let before = mmio_read32(stream + SD_LPIB)?;
        let run_ctl = (u32::from(STREAM_TAG) << 20) | (1 << 1);
        mmio_write32(stream + SD_CTL, run_ctl)?;
        if mmio_read32(stream + SD_CTL)? & (1 << 1) == 0 {
            return Err(InitError::StreamStartFailed);
        }
        let mut after = before;
        for _ in 0..POLL_LIMIT {
            after = mmio_read32(stream + SD_LPIB)?;
            if after != before {
                break;
            }
            core::hint::spin_loop();
        }
        mmio_write32(stream + SD_CTL, run_ctl & !(1 << 1))?;
        if after == before {
            return Err(InitError::StreamNoProgress);
        }
        compiler_fence(Ordering::Acquire);
        let mut changed = 0u32;
        for off in 0..PCM_BYTES as usize {
            let v = unsafe { ptr::read_volatile((pcm.virtual_address as *const u8).add(off)) };
            if v != SENTINEL {
                changed = changed.saturating_add(1);
            }
        }
        if changed == 0 {
            return Err(InitError::CaptureBufferUnchanged);
        }
        Ok(CaptureSummary {
            stream_index: 0,
            stream_tag: STREAM_TAG,
            converter_node: converter,
            format: PCM_FORMAT,
            bytes: PCM_BYTES,
            position_before: before,
            position_after: after,
            changed_bytes: changed,
        })
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
    if let Some(c) = slot.as_ref() {
        return Ok(c.summary());
    }
    let device = find_controller().ok_or(InitError::MissingController)?;
    let c = HdaController::initialize(device)?;
    let summary = c.summary();
    *slot = Some(c);
    Ok(summary)
}
pub fn discover_codec() -> Result<CodecSummary, InitError> {
    CONTROLLER
        .lock()
        .as_mut()
        .ok_or(InitError::MissingController)?
        .codec_summary()
}

pub fn discover_topology() -> Result<TopologySummary, InitError> {
    CONTROLLER
        .lock()
        .as_mut()
        .ok_or(InitError::MissingController)?
        .topology_summary()
}

pub fn mixer_smoke_test() -> Result<MixerSummary, InitError> {
    CONTROLLER
        .lock()
        .as_mut()
        .ok_or(InitError::MissingController)?
        .mixer_smoke_test()
}
pub fn playback_smoke_test() -> Result<PlaybackSummary, InitError> {
    CONTROLLER
        .lock()
        .as_mut()
        .ok_or(InitError::MissingController)?
        .playback_smoke_test()
}
pub fn capture_smoke_test() -> Result<CaptureSummary, InitError> {
    CONTROLLER
        .lock()
        .as_mut()
        .ok_or(InitError::MissingController)?
        .capture_smoke_test()
}
fn find_controller() -> Option<pci::Device> {
    for i in 0..64 {
        if let Some(d) = pci::device(i) {
            if d.class == AUDIO_CLASS && d.subclass == HDA_SUBCLASS {
                return Some(d);
            }
        }
    }
    None
}
fn wait_for_bit(a: u64, m: u32, s: bool) -> Result<(), InitError> {
    for _ in 0..POLL_LIMIT {
        let v = mmio_read32(a)?;
        if (v & m != 0) == s {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(InitError::ResetTimeout)
}
fn mmio_read8(p: u64) -> Result<u8, InitError> {
    let v = paging::map_mmio(p).map_err(|_| InitError::MmioUnavailable)?;
    Ok(unsafe { ptr::read_volatile(v as *const u8) })
}
fn mmio_read16(p: u64) -> Result<u16, InitError> {
    let v = paging::map_mmio(p).map_err(|_| InitError::MmioUnavailable)?;
    Ok(unsafe { ptr::read_volatile(v as *const u16) })
}
fn mmio_read32(p: u64) -> Result<u32, InitError> {
    let v = paging::map_mmio(p).map_err(|_| InitError::MmioUnavailable)?;
    Ok(unsafe { ptr::read_volatile(v as *const u32) })
}
fn mmio_write8(p: u64, x: u8) -> Result<(), InitError> {
    let v = paging::map_mmio(p).map_err(|_| InitError::MmioUnavailable)?;
    unsafe { ptr::write_volatile(v as *mut u8, x) };
    Ok(())
}
fn mmio_write16(p: u64, x: u16) -> Result<(), InitError> {
    let v = paging::map_mmio(p).map_err(|_| InitError::MmioUnavailable)?;
    unsafe { ptr::write_volatile(v as *mut u16, x) };
    Ok(())
}
fn mmio_write32(p: u64, x: u32) -> Result<(), InitError> {
    let v = paging::map_mmio(p).map_err(|_| InitError::MmioUnavailable)?;
    unsafe { ptr::write_volatile(v as *mut u32, x) };
    Ok(())
}
pub fn self_test() -> bool {
    let gcap: u16 = (4 << 12) | (2 << 8) | (1 << 3) | 1;
    let command = (2u32 << 28) | (3u32 << 20) | (0x0f00u32 << 8) | 4;
    ((gcap >> 12) & 15) == 4
        && ((gcap >> 8) & 15) == 2
        && ((gcap >> 3) & 31) == 1
        && gcap & 1 != 0
        && command == 0x203f0004
}

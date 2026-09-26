use crate::{device, driver, hda};

pub const DEVICE_NAME: &str = "hda0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Controller(hda::InitError),
    DeviceRegistration,
    DriverRegistration,
    DriverBinding,
    UnsupportedDirection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub playback_streams: u8,
    pub capture_streams: u8,
    pub bidirectional_streams: u8,
    pub dma_64bit: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Playback,
    Capture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamFormat {
    pub sample_rate_hz: u32,
    pub channels: u8,
    pub bits_per_sample: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamDescriptor {
    pub direction: Direction,
    pub format: StreamFormat,
    pub hardware_stream: u8,
    pub converter_node: u8,
    pub bytes_transferred: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MixerState {
    pub pin_node: u8,
    pub amplifier_node: u8,
    pub gain_steps: u8,
    pub offset: u8,
    pub step_size: u8,
    pub mute_supported: bool,
    pub current_gain: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntegrationSummary {
    pub capabilities: Capabilities,
    pub playback: StreamDescriptor,
    pub capture: StreamDescriptor,
    pub mixer: MixerState,
}

impl From<hda::ControllerSummary> for Capabilities {
    fn from(summary: hda::ControllerSummary) -> Self {
        Self {
            playback_streams: summary.output_streams,
            capture_streams: summary.input_streams,
            bidirectional_streams: summary.bidirectional_streams,
            dma_64bit: summary.supports_64bit,
        }
    }
}

pub fn init() -> Result<Capabilities, Error> {
    let summary = hda::init().map_err(Error::Controller)?;
    if device::find(DEVICE_NAME).is_none() {
        device::register(device::Device {
            name: DEVICE_NAME,
            kind: device::DeviceKind::Audio,
            irq: None,
        })
        .map_err(|_| Error::DeviceRegistration)?;
    }
    if !driver::register(DEVICE_NAME, device::DeviceKind::Audio)
        && device::find(DEVICE_NAME).is_none()
    {
        return Err(Error::DriverRegistration);
    }
    if !driver::bind(DEVICE_NAME) {
        return Err(Error::DriverBinding);
    }
    Ok(summary.into())
}

pub fn default_format() -> StreamFormat {
    StreamFormat {
        sample_rate_hz: 48_000,
        channels: 2,
        bits_per_sample: 16,
    }
}

pub fn playback_once() -> Result<StreamDescriptor, Error> {
    let summary = hda::playback_smoke_test().map_err(Error::Controller)?;
    Ok(StreamDescriptor {
        direction: Direction::Playback,
        format: default_format(),
        hardware_stream: summary.stream_index,
        converter_node: summary.converter_node,
        bytes_transferred: summary.position_after.wrapping_sub(summary.position_before),
    })
}

pub fn capture_once() -> Result<StreamDescriptor, Error> {
    let summary = hda::capture_smoke_test().map_err(Error::Controller)?;
    Ok(StreamDescriptor {
        direction: Direction::Capture,
        format: default_format(),
        hardware_stream: summary.stream_index,
        converter_node: summary.converter_node,
        bytes_transferred: summary.changed_bytes,
    })
}

pub fn mixer_state() -> Result<MixerState, Error> {
    let summary = hda::mixer_smoke_test().map_err(Error::Controller)?;
    Ok(MixerState {
        pin_node: summary.pin_node,
        amplifier_node: summary.amp_node,
        gain_steps: summary.gain_steps,
        offset: summary.offset,
        step_size: summary.step_size,
        mute_supported: summary.mute_supported,
        current_gain: summary.test_gain,
    })
}

pub fn integration_smoke_test() -> Result<IntegrationSummary, Error> {
    let capabilities = init()?;
    if capabilities.playback_streams == 0 || capabilities.capture_streams == 0 {
        return Err(Error::UnsupportedDirection);
    }

    let mixer = mixer_state()?;
    let playback = playback_once()?;
    let capture = capture_once()?;

    Ok(IntegrationSummary {
        capabilities,
        playback,
        capture,
        mixer,
    })
}

pub fn self_test() -> bool {
    let summary = hda::ControllerSummary {
        vendor_id: 0x8086,
        device_id: 0x2668,
        bus: 0,
        device: 1,
        function: 0,
        input_streams: 2,
        output_streams: 4,
        bidirectional_streams: 1,
        supports_64bit: true,
    };
    let caps: Capabilities = summary.into();
    let format = default_format();
    caps.playback_streams == 4
        && caps.capture_streams == 2
        && caps.bidirectional_streams == 1
        && caps.dma_64bit
        && format.sample_rate_hz == 48_000
        && format.channels == 2
        && format.bits_per_sample == 16
}

use crate::{device, driver, hda};

pub const DEVICE_NAME: &str = "hda0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Controller(hda::InitError),
    DeviceRegistration,
    DriverRegistration,
    DriverBinding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub playback_streams: u8,
    pub capture_streams: u8,
    pub bidirectional_streams: u8,
    pub dma_64bit: bool,
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
    caps.playback_streams == 4
        && caps.capture_streams == 2
        && caps.bidirectional_streams == 1
        && caps.dma_64bit
}

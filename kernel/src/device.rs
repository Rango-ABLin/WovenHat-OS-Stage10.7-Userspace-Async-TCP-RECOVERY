use spin::Mutex;

use crate::config::MAX_DEVICES;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Console,
    Serial,
    Timer,
    Keyboard,
    Block,
}

#[derive(Clone, Copy)]
pub struct Device {
    pub name: &'static str,
    pub kind: DeviceKind,
    pub irq: Option<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RegisterError {
    Full,
    DuplicateName,
    DuplicateIrq,
}

struct Registry {
    devices: [Option<Device>; MAX_DEVICES],
}

impl Registry {
    const fn new() -> Self {
        Self {
            devices: [None; MAX_DEVICES],
        }
    }

    fn register(&mut self, device: Device) -> Result<(), RegisterError> {
        if self
            .devices
            .iter()
            .flatten()
            .any(|registered| registered.name == device.name)
        {
            return Err(RegisterError::DuplicateName);
        }
        if device.irq.is_some()
            && self
                .devices
                .iter()
                .flatten()
                .any(|registered| registered.irq == device.irq)
        {
            return Err(RegisterError::DuplicateIrq);
        }
        let slot = self
            .devices
            .iter_mut()
            .find(|registered| registered.is_none())
            .ok_or(RegisterError::Full)?;
        *slot = Some(device);
        Ok(())
    }
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry::new());

pub fn register(device: Device) -> Result<(), RegisterError> {
    REGISTRY.lock().register(device)
}

pub fn find(name: &str) -> Option<Device> {
    REGISTRY
        .lock()
        .devices
        .iter()
        .flatten()
        .copied()
        .find(|device| device.name == name)
}

pub fn count() -> usize {
    REGISTRY.lock().devices.iter().flatten().count()
}

pub fn count_kind(kind: DeviceKind) -> usize {
    REGISTRY
        .lock()
        .devices
        .iter()
        .flatten()
        .filter(|device| device.kind == kind)
        .count()
}

pub fn self_test(block_present: bool) -> bool {
    let mut scratch = Registry::new();
    let first = Device {
        name: "test-timer",
        kind: DeviceKind::Timer,
        irq: Some(9),
    };
    let duplicate_name = Device {
        name: "test-timer",
        kind: DeviceKind::Serial,
        irq: None,
    };
    let duplicate_irq = Device {
        name: "test-keyboard",
        kind: DeviceKind::Block,
        irq: Some(9),
    };
    let uniqueness_valid = scratch.register(first).is_ok()
        && scratch.register(duplicate_name) == Err(RegisterError::DuplicateName)
        && scratch.register(duplicate_irq) == Err(RegisterError::DuplicateIrq);

    let expected_count = if block_present { 5 } else { 4 };
    uniqueness_valid
        && count() == expected_count
        && count_kind(DeviceKind::Console) == 1
        && count_kind(DeviceKind::Serial) == 1
        && count_kind(DeviceKind::Timer) == 1
        && count_kind(DeviceKind::Keyboard) == 1
        && count_kind(DeviceKind::Block) == usize::from(block_present)
        && (block_present == find("ata0").is_some())
        && find("pit").is_some_and(|device| device.irq == Some(crate::timer::IRQ))
        && find("ps2-keyboard").is_some_and(|device| device.irq == Some(crate::keyboard::IRQ))
}

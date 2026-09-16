use core::arch::asm;

use spin::Mutex;

const CONFIG_ADDRESS: u16 = 0x0cf8;
const CONFIG_DATA: u16 = 0x0cfc;
const MAX_DEVICES: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Device {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
}

#[derive(Clone, Copy, Default)]
pub struct Summary {
    pub discovered: u16,
    pub recorded: u8,
    pub storage: u16,
    pub network: u16,
    pub display: u16,
    pub bridges: u16,
    pub truncated: bool,
}

struct Inventory {
    devices: [Option<Device>; MAX_DEVICES],
    summary: Summary,
}

impl Inventory {
    const fn new() -> Self {
        Self {
            devices: [None; MAX_DEVICES],
            summary: Summary {
                discovered: 0,
                recorded: 0,
                storage: 0,
                network: 0,
                display: 0,
                bridges: 0,
                truncated: false,
            },
        }
    }

    fn record(&mut self, device: Device) {
        self.summary.discovered = self.summary.discovered.saturating_add(1);
        match device.class {
            0x01 => self.summary.storage = self.summary.storage.saturating_add(1),
            0x02 => self.summary.network = self.summary.network.saturating_add(1),
            0x03 => self.summary.display = self.summary.display.saturating_add(1),
            0x06 => self.summary.bridges = self.summary.bridges.saturating_add(1),
            _ => {}
        }
        if let Some(slot) = self.devices.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(device);
            self.summary.recorded = self.summary.recorded.saturating_add(1);
        } else {
            self.summary.truncated = true;
        }
    }
}

static INVENTORY: Mutex<Inventory> = Mutex::new(Inventory::new());

pub fn discover() -> Summary {
    let mut inventory = Inventory::new();
    for bus in 0_u16..=255 {
        for device in 0_u8..32 {
            let vendor = read_config(bus as u8, device, 0, 0) as u16;
            if vendor == 0xffff {
                continue;
            }
            let header = (read_config(bus as u8, device, 0, 0x0c) >> 16) as u8;
            let functions = if header & 0x80 != 0 { 8 } else { 1 };
            for function in 0..functions {
                if let Some(found) = probe(bus as u8, device, function) {
                    inventory.record(found);
                }
            }
        }
    }
    let summary = inventory.summary;
    *INVENTORY.lock() = inventory;
    summary
}

pub fn device(index: usize) -> Option<Device> {
    INVENTORY.lock().devices.get(index).copied().flatten()
}

#[allow(dead_code)]
pub fn read_config_dword(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    read_config(bus, device, function, offset)
}

pub fn write_config_dword(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    unsafe {
        outl(
            CONFIG_ADDRESS,
            config_address(bus, device, function, offset),
        );
        outl(CONFIG_DATA, value);
    }
}

pub fn enable_io_bus_master(bus: u8, device: u8, function: u8) {
    let value = read_config(bus, device, function, 0x04);
    // PCI command: bit0 I/O space, bit2 bus master. Preserve status/high bits.
    write_config_dword(bus, device, function, 0x04, value | 0x0000_0005);
}

pub fn bar0_io_base(bus: u8, device: u8, function: u8) -> Option<u16> {
    let bar = read_config(bus, device, function, 0x10);
    if bar & 1 == 0 {
        return None;
    }
    let base = bar & 0xffff_fffc;
    u16::try_from(base).ok().filter(|base| *base != 0)
}

fn probe(bus: u8, device: u8, function: u8) -> Option<Device> {
    let identity = read_config(bus, device, function, 0);
    let vendor_id = identity as u16;
    if vendor_id == 0xffff {
        return None;
    }
    let class = read_config(bus, device, function, 0x08);
    Some(Device {
        bus,
        device,
        function,
        vendor_id,
        device_id: (identity >> 16) as u16,
        class: (class >> 24) as u8,
        subclass: (class >> 16) as u8,
    })
}

const fn config_address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    1 << 31
        | (bus as u32) << 16
        | (device as u32) << 11
        | (function as u32) << 8
        | (offset as u32 & 0xfc)
}

fn read_config(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    unsafe {
        outl(
            CONFIG_ADDRESS,
            config_address(bus, device, function, offset),
        );
        inl(CONFIG_DATA)
    }
}

pub fn self_test() -> bool {
    config_address(2, 3, 4, 0x0b) == 0x8002_1c08
        && INVENTORY.lock().summary.recorded as usize <= MAX_DEVICES
        && device(MAX_DEVICES).is_none()
}

unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    asm!("in eax, dx", in("dx") port, out("eax") value, options(nomem, nostack, preserves_flags));
    value
}

unsafe fn outl(port: u16, value: u32) {
    asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
}

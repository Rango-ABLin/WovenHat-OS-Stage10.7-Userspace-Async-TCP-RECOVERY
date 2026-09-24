use core::{arch::asm, ptr};

use crate::{hal::acpi::McfgAllocation, irq_lock::IrqMutex as Mutex};

const CONFIG_ADDRESS: u16 = 0x0cf8;
const CONFIG_DATA: u16 = 0x0cfc;
const MAX_DEVICES: usize = 64;
const MAX_ECAM_REGIONS: usize = crate::hal::acpi::MAX_MCFG_ALLOCATIONS;
const MAX_CAPABILITY_STEPS: usize = 48;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Address {
    pub segment: u16,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum BarKind {
    #[default]
    Memory32,
    Memory64,
    Io,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Bar {
    pub valid: bool,
    pub kind: BarKind,
    pub address: u64,
    pub prefetchable: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub power_management: bool,
    pub msi: bool,
    pub msix: bool,
    pub pcie: bool,
    pub malformed: bool,
    pub msi_offset: u16,
    pub msix_offset: u16,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Device {
    pub segment: u16,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub revision: u8,
    pub prog_if: u8,
    pub class: u8,
    pub subclass: u8,
    pub header_type: u8,
    pub command: u16,
    pub status: u16,
    pub bars: [Bar; 6],
    pub capabilities: Capabilities,
}

#[derive(Clone, Copy, Default)]
pub struct Summary {
    pub discovered: u16,
    pub recorded: u8,
    pub storage: u16,
    pub network: u16,
    pub display: u16,
    pub bridges: u16,
    pub segments: u8,
    pub ecam: bool,
    pub truncated: bool,
}

#[derive(Clone, Copy, Default)]
struct ConfigState {
    ecam: [Option<McfgAllocation>; MAX_ECAM_REGIONS],
    ecam_count: usize,
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
                segments: 0,
                ecam: false,
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

/// Serializes the legacy CONFIG_ADDRESS/CONFIG_DATA transaction and PCI config
/// read/modify/write operations across CPUs. Never sleep while this lock is held.
static CONFIG_LOCK: Mutex<()> = Mutex::with_rank((), 10);
static CONFIG: Mutex<ConfigState> = Mutex::with_rank(
    ConfigState {
        ecam: [None; MAX_ECAM_REGIONS],
        ecam_count: 0,
    },
    10,
);
static INVENTORY: Mutex<Inventory> = Mutex::with_rank(Inventory::new(), 10);

pub fn configure(allocations: &[McfgAllocation]) {
    let mut state = CONFIG.lock();
    state.ecam = [None; MAX_ECAM_REGIONS];
    state.ecam_count = core::cmp::min(allocations.len(), MAX_ECAM_REGIONS);
    for (slot, allocation) in state.ecam.iter_mut().zip(allocations.iter().copied()) {
        *slot = Some(allocation);
    }
}

pub fn discover() -> Summary {
    let config = *CONFIG.lock();
    let mut inventory = Inventory::new();
    inventory.summary.ecam = config.ecam_count != 0;

    if config.ecam_count == 0 {
        scan_bus_range(&mut inventory, 0, 0, u8::MAX);
        inventory.summary.segments = 1;
    } else {
        let mut seen_segments = [None; MAX_ECAM_REGIONS];
        let mut seen_count = 0usize;
        for allocation in config.ecam[..config.ecam_count].iter().flatten().copied() {
            scan_bus_range(
                &mut inventory,
                allocation.segment_group,
                allocation.start_bus,
                allocation.end_bus,
            );
            if !seen_segments[..seen_count].contains(&Some(allocation.segment_group))
                && seen_count < seen_segments.len()
            {
                seen_segments[seen_count] = Some(allocation.segment_group);
                seen_count += 1;
            }
        }
        inventory.summary.segments = u8::try_from(seen_count).unwrap_or(u8::MAX);
    }

    let summary = inventory.summary;
    *INVENTORY.lock() = inventory;
    summary
}

fn scan_bus_range(inventory: &mut Inventory, segment: u16, start_bus: u8, end_bus: u8) {
    for bus in u16::from(start_bus)..=u16::from(end_bus) {
        let bus = bus as u8;
        for device in 0_u8..32 {
            let address = Address {
                segment,
                bus,
                device,
                function: 0,
            };
            let Some(identity) = read_config(address, 0) else {
                continue;
            };
            if identity as u16 == 0xffff {
                continue;
            }
            let header = read_config(address, 0x0c).unwrap_or(u32::MAX);
            let functions = if ((header >> 16) as u8) & 0x80 != 0 {
                8
            } else {
                1
            };
            for function in 0..functions {
                let address = Address {
                    function,
                    ..address
                };
                if let Some(found) = probe(address) {
                    inventory.record(found);
                }
            }
        }
    }
}

pub fn device(index: usize) -> Option<Device> {
    INVENTORY.lock().devices.get(index).copied().flatten()
}

#[allow(dead_code)]
pub fn read_config_dword(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    read_config(
        Address {
            segment: 0,
            bus,
            device,
            function,
        },
        u16::from(offset),
    )
    .unwrap_or(u32::MAX)
}

pub fn enable_io_bus_master(bus: u8, device: u8, function: u8) {
    let address = Address {
        segment: 0,
        bus,
        device,
        function,
    };
    let _guard = CONFIG_LOCK.lock();
    if let Some(value) = read_config_unlocked(address, 0x04) {
        // PCI command: bit0 I/O space, bit2 bus master. Preserve status/high bits.
        let _ = write_config_unlocked(address, 0x04, value | 0x0000_0005);
    }
}

/// Enable MMIO decoding and DMA bus mastering for a PCI/PCIe function.
/// Returns false if the configuration transaction cannot be completed.
#[cfg(any(
    feature = "stage13-3-test",
    feature = "stage13-4-test",
    feature = "stage13-5-test",
    feature = "stage13-6-test",
    feature = "stage13-7-test",
    feature = "stage13-8-test",
    feature = "stage13-9-test"
))]
pub fn enable_memory_bus_master(address: Address) -> bool {
    let _guard = CONFIG_LOCK.lock();
    let Some(value) = read_config_unlocked(address, 0x04) else {
        return false;
    };
    // PCI command: bit1 memory space, bit2 bus master. Preserve all other bits.
    write_config_unlocked(address, 0x04, value | 0x0000_0006)
}

pub fn bar0_io_base(bus: u8, device: u8, function: u8) -> Option<u16> {
    let bar = read_config(
        Address {
            segment: 0,
            bus,
            device,
            function,
        },
        0x10,
    )?;
    if bar & 1 == 0 {
        return None;
    }
    let base = bar & 0xffff_fffc;
    u16::try_from(base).ok().filter(|base| *base != 0)
}

fn probe(address: Address) -> Option<Device> {
    let identity = read_config(address, 0)?;
    let vendor_id = identity as u16;
    if vendor_id == 0xffff {
        return None;
    }
    let class_revision = read_config(address, 0x08)?;
    let header = read_config(address, 0x0c)?;
    let command_status = read_config(address, 0x04)?;
    let header_type = (header >> 16) as u8;
    Some(Device {
        segment: address.segment,
        bus: address.bus,
        device: address.device,
        function: address.function,
        vendor_id,
        device_id: (identity >> 16) as u16,
        revision: class_revision as u8,
        prog_if: (class_revision >> 8) as u8,
        subclass: (class_revision >> 16) as u8,
        class: (class_revision >> 24) as u8,
        header_type,
        command: command_status as u16,
        status: (command_status >> 16) as u16,
        bars: read_bars(address, header_type),
        capabilities: read_capabilities(address, (command_status >> 16) as u16, header_type),
    })
}

fn read_bars(address: Address, header_type: u8) -> [Bar; 6] {
    let mut bars = [Bar::default(); 6];
    let count = if header_type & 0x7f == 0x00 {
        6
    } else if header_type & 0x7f == 0x01 {
        2
    } else {
        0
    };
    let mut index = 0usize;
    while index < count {
        let offset = 0x10 + (index as u16 * 4);
        let Some(low) = read_config(address, offset) else {
            break;
        };
        if low == 0 || low == u32::MAX {
            index += 1;
            continue;
        }
        if low & 1 != 0 {
            bars[index] = Bar {
                valid: true,
                kind: BarKind::Io,
                address: u64::from(low & !3),
                prefetchable: false,
            };
            index += 1;
            continue;
        }
        let memory_type = (low >> 1) & 3;
        let prefetchable = low & 8 != 0;
        if memory_type == 2 && index + 1 < count {
            if let Some(high) = read_config(address, offset + 4) {
                bars[index] = Bar {
                    valid: true,
                    kind: BarKind::Memory64,
                    address: (u64::from(high) << 32) | u64::from(low & !0xf),
                    prefetchable,
                };
                index += 2;
                continue;
            }
        }
        if memory_type == 0 {
            bars[index] = Bar {
                valid: true,
                kind: BarKind::Memory32,
                address: u64::from(low & !0xf),
                prefetchable,
            };
        }
        index += 1;
    }
    bars
}

fn read_capabilities(address: Address, status: u16, header_type: u8) -> Capabilities {
    let mut capabilities = Capabilities::default();
    if status & (1 << 4) == 0 {
        return capabilities;
    }
    let pointer_register = if header_type & 0x7f == 0x02 {
        0x14
    } else {
        0x34
    };
    let Some(mut pointer) =
        read_config(address, pointer_register).map(|value| (value & 0xfc) as u16)
    else {
        capabilities.malformed = true;
        return capabilities;
    };
    let mut visited = [0_u8; MAX_CAPABILITY_STEPS];
    let mut visited_count = 0usize;
    while pointer != 0 {
        if !(0x40..=0xfc).contains(&pointer)
            || pointer & 3 != 0
            || visited[..visited_count].contains(&(pointer as u8))
        {
            capabilities.malformed = true;
            break;
        }
        if visited_count == visited.len() {
            capabilities.malformed = true;
            break;
        }
        visited[visited_count] = pointer as u8;
        visited_count += 1;
        let Some(value) = read_config(address, pointer) else {
            capabilities.malformed = true;
            break;
        };
        match value as u8 {
            0x01 => capabilities.power_management = true,
            0x05 => {
                capabilities.msi = true;
                capabilities.msi_offset = pointer;
            }
            0x10 => capabilities.pcie = true,
            0x11 => {
                capabilities.msix = true;
                capabilities.msix_offset = pointer;
            }
            _ => {}
        }
        pointer = ((value >> 8) & 0xfc) as u16;
    }
    capabilities
}

// MSI programming currently belongs to the feature-gated AX200 candidate.
#[cfg(feature = "stage13-9-test")]
pub use msi::*;
#[cfg(feature = "stage13-9-test")]
mod msi {
    use super::*;
    pub const PCI_CAP_ID_MSI: u8 = 0x05;
    pub const PCI_MSI_ENABLE: u16 = 1 << 0;
    pub const PCI_MSI_64BIT_CAPABLE: u16 = 1 << 7;
    pub const PCI_MSI_PER_VECTOR_MASKING: u16 = 1 << 8;

    /// x86 MSI message-address base for local APIC fixed delivery.
    pub const X86_MSI_ADDRESS_BASE: u32 = 0xfee0_0000;
    pub const X86_MSI_DESTINATION_SHIFT: u32 = 12;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum MsiError {
        NoCapability,
        MalformedCapability,
        UnsupportedDestination,
        ConfigRead,
        ConfigWrite,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct MsiCapability {
        pub offset: u16,
        pub control: u16,
        pub is_64_bit: bool,
        pub per_vector_masking: bool,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct MsiMessage {
        pub address_low: u32,
        pub address_high: u32,
        pub data: u16,
    }

    impl MsiMessage {
        pub fn fixed(destination_apic_id: u32, vector: u8) -> Result<Self, MsiError> {
            if destination_apic_id > u8::MAX as u32 {
                return Err(MsiError::UnsupportedDestination);
            }
            Ok(Self {
                address_low: X86_MSI_ADDRESS_BASE
                    | (destination_apic_id << X86_MSI_DESTINATION_SHIFT),
                address_high: 0,
                data: u16::from(vector),
            })
        }
    }

    pub fn msi_capability(device: Device) -> Result<MsiCapability, MsiError> {
        if !device.capabilities.msi || device.capabilities.msi_offset == 0 {
            return Err(MsiError::NoCapability);
        }
        let address = Address {
            segment: device.segment,
            bus: device.bus,
            device: device.device,
            function: device.function,
        };
        let header =
            read_config(address, device.capabilities.msi_offset).ok_or(MsiError::ConfigRead)?;
        if header as u8 != PCI_CAP_ID_MSI {
            return Err(MsiError::MalformedCapability);
        }
        let control = (header >> 16) as u16;
        Ok(MsiCapability {
            offset: device.capabilities.msi_offset,
            control,
            is_64_bit: control & PCI_MSI_64BIT_CAPABLE != 0,
            per_vector_masking: control & PCI_MSI_PER_VECTOR_MASKING != 0,
        })
    }

    /// Program one MSI vector while keeping the capability disabled until all
    /// address/data fields are valid. WovenHat currently uses one fixed-delivery
    /// vector and does not enable multiple-message MSI.
    pub fn program_msi(
        device: Device,
        destination_apic_id: u32,
        vector: u8,
    ) -> Result<MsiCapability, MsiError> {
        let capability = msi_capability(device)?;
        let message = MsiMessage::fixed(destination_apic_id, vector)?;
        let address = Address {
            segment: device.segment,
            bus: device.bus,
            device: device.device,
            function: device.function,
        };

        let _guard = CONFIG_LOCK.lock();
        let base = capability.offset;

        let Some(header) = read_config_unlocked(address, base) else {
            return Err(MsiError::ConfigRead);
        };
        let mut control = (header >> 16) as u16;
        control &= !PCI_MSI_ENABLE;
        // Multiple-message enable bits [6:4] stay zero: one vector only.
        control &= !(0b111 << 4);
        let disabled_header = (header & 0x0000_ffff) | (u32::from(control) << 16);
        if !write_config_unlocked(address, base, disabled_header) {
            return Err(MsiError::ConfigWrite);
        }

        if !write_config_unlocked(address, base + 4, message.address_low) {
            return Err(MsiError::ConfigWrite);
        }

        let data_dword_offset = if capability.is_64_bit {
            if !write_config_unlocked(address, base + 8, message.address_high) {
                return Err(MsiError::ConfigWrite);
            }
            base + 12
        } else {
            base + 8
        };

        let Some(old_data_dword) = read_config_unlocked(address, data_dword_offset) else {
            return Err(MsiError::ConfigRead);
        };
        let new_data_dword = (old_data_dword & 0xffff_0000) | u32::from(message.data);
        if !write_config_unlocked(address, data_dword_offset, new_data_dword) {
            return Err(MsiError::ConfigWrite);
        }

        control |= PCI_MSI_ENABLE;
        let enabled_header = (header & 0x0000_ffff) | (u32::from(control) << 16);
        if !write_config_unlocked(address, base, enabled_header) {
            return Err(MsiError::ConfigWrite);
        }

        Ok(MsiCapability {
            control,
            ..capability
        })
    }

    pub fn disable_msi(device: Device) -> Result<(), MsiError> {
        let capability = msi_capability(device)?;
        let address = Address {
            segment: device.segment,
            bus: device.bus,
            device: device.device,
            function: device.function,
        };
        let _guard = CONFIG_LOCK.lock();
        let Some(header) = read_config_unlocked(address, capability.offset) else {
            return Err(MsiError::ConfigRead);
        };
        let control = ((header >> 16) as u16) & !PCI_MSI_ENABLE;
        let new_header = (header & 0x0000_ffff) | (u32::from(control) << 16);
        if !write_config_unlocked(address, capability.offset, new_header) {
            return Err(MsiError::ConfigWrite);
        }
        Ok(())
    }

    /// Pure acceptance test for x86 MSI encoding. Physical configuration-space
    /// writes are intentionally deferred until a real supported PCI function is
    /// explicitly bound by the Wi-Fi transport.
    pub fn stage13_10aa_msi_self_test() -> bool {
        let Ok(message) = MsiMessage::fixed(0x2a, crate::interrupts::WIFI_DEVICE_VECTOR) else {
            return false;
        };
        message.address_low == 0xfee2_a000
            && message.address_high == 0
            && message.data == u16::from(crate::interrupts::WIFI_DEVICE_VECTOR)
            && MsiMessage::fixed(0x100, crate::interrupts::WIFI_DEVICE_VECTOR)
                == Err(MsiError::UnsupportedDestination)
    }
}

fn read_config(address: Address, offset: u16) -> Option<u32> {
    let _guard = CONFIG_LOCK.lock();
    read_config_unlocked(address, offset)
}

fn read_config_unlocked(address: Address, offset: u16) -> Option<u32> {
    if offset & 3 != 0 || offset > 0xffc || address.device >= 32 || address.function >= 8 {
        return None;
    }
    if let Some(physical) = ecam_physical(address, offset) {
        let virtual_address = crate::paging::map_mmio(physical).ok()?;
        return Some(unsafe { ptr::read_volatile(virtual_address as *const u32) });
    }
    if address.segment != 0 || offset > 0xfc {
        return None;
    }
    unsafe {
        outl(
            CONFIG_ADDRESS,
            config_address(address.bus, address.device, address.function, offset as u8),
        );
        Some(inl(CONFIG_DATA))
    }
}

fn write_config_unlocked(address: Address, offset: u16, value: u32) -> bool {
    if offset & 3 != 0 || offset > 0xffc || address.device >= 32 || address.function >= 8 {
        return false;
    }
    if let Some(physical) = ecam_physical(address, offset) {
        let Ok(virtual_address) = crate::paging::map_mmio(physical) else {
            return false;
        };
        unsafe { ptr::write_volatile(virtual_address as *mut u32, value) };
        return true;
    }
    if address.segment != 0 || offset > 0xfc {
        return false;
    }
    unsafe {
        outl(
            CONFIG_ADDRESS,
            config_address(address.bus, address.device, address.function, offset as u8),
        );
        outl(CONFIG_DATA, value);
    }
    true
}

fn ecam_physical(address: Address, offset: u16) -> Option<u64> {
    let state = CONFIG.lock();
    let allocation = state.ecam[..state.ecam_count]
        .iter()
        .flatten()
        .find(|allocation| {
            allocation.segment_group == address.segment
                && (allocation.start_bus..=allocation.end_bus).contains(&address.bus)
        })?;
    let bus = u64::from(address.bus - allocation.start_bus);
    allocation
        .base_address
        .checked_add(bus << 20)?
        .checked_add(u64::from(address.device) << 15)?
        .checked_add(u64::from(address.function) << 12)?
        .checked_add(u64::from(offset))
}

const fn config_address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    1 << 31
        | (bus as u32) << 16
        | (device as u32) << 11
        | (function as u32) << 8
        | (offset as u32 & 0xfc)
}

pub fn self_test() -> bool {
    let allocation = McfgAllocation {
        base_address: 0xe000_0000,
        segment_group: 0,
        start_bus: 0x20,
        end_bus: 0x2f,
    };
    let ecam_math = ecam_address_for(
        allocation,
        Address {
            segment: 0,
            bus: 0x21,
            device: 3,
            function: 4,
        },
        0x100,
    ) == Some(0xe000_0000 + (1 << 20) + (3 << 15) + (4 << 12) + 0x100);
    config_address(2, 3, 4, 0x0b) == 0x8002_1c08
        && ecam_math
        && INVENTORY.lock().summary.recorded as usize <= MAX_DEVICES
        && device(MAX_DEVICES).is_none()
}

fn ecam_address_for(allocation: McfgAllocation, address: Address, offset: u16) -> Option<u64> {
    if allocation.segment_group != address.segment
        || !(allocation.start_bus..=allocation.end_bus).contains(&address.bus)
        || address.device >= 32
        || address.function >= 8
        || offset > 0xfff
    {
        return None;
    }
    allocation
        .base_address
        .checked_add(u64::from(address.bus - allocation.start_bus) << 20)?
        .checked_add(u64::from(address.device) << 15)?
        .checked_add(u64::from(address.function) << 12)?
        .checked_add(u64::from(offset))
}

unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    unsafe {
        asm!("in eax, dx", in("dx") port, out("eax") value, options(nomem, nostack, preserves_flags));
    }
    value
}

unsafe fn outl(port: u16, value: u32) {
    unsafe {
        asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
    }
}

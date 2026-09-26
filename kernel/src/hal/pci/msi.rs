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
            address_low: X86_MSI_ADDRESS_BASE | (destination_apic_id << X86_MSI_DESTINATION_SHIFT),
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

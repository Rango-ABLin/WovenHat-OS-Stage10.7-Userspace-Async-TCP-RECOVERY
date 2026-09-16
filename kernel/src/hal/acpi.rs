use bootloader_api::info::MemoryRegion;

const RSDP_V1_LENGTH: usize = 20;
const RSDP_V2_LENGTH: usize = 36;
const SDT_HEADER_LENGTH: usize = 36;
const MAX_TABLE_LENGTH: usize = 64 * 1024;
const MAX_TABLES: usize = 256;
const MADT_HEADER_LENGTH: usize = SDT_HEADER_LENGTH + 8;
const MAX_MADT_ENTRIES: usize = 256;
const MAX_MADT_ENTRY_LENGTH: usize = u8::MAX as usize;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Missing,
    OutOfRange,
    InvalidSignature,
    InvalidChecksum,
    InvalidLength,
    AddressOverflow,
}

#[derive(Clone, Copy, Default)]
pub struct Summary {
    pub revision: u8,
    pub tables: u16,
    pub apic: bool,
    pub local_apic_address: u64,
    pub enabled_processors: u16,
    pub processor_ids: [u32; 16],
    pub processor_count: usize,
    pub io_apic_address: u32,
    pub io_apic_gsi_base: u32,
    pub isa_gsi: [Option<(u32, u16)>; 16],
    pub io_apics: u16,
    pub interrupt_overrides: u16,
    pub madt_entries: u16,
    pub fadt: bool,
    pub hpet: bool,
    pub mcfg: bool,
    pub truncated: bool,
}

pub fn discover(
    physical_offset: u64,
    rsdp_address: Option<u64>,
    regions: &[MemoryRegion],
) -> Result<Summary, Error> {
    let rsdp_address = rsdp_address.ok_or(Error::Missing)?;
    let mut rsdp = [0_u8; RSDP_V2_LENGTH];
    read_physical(
        physical_offset,
        rsdp_address,
        &mut rsdp[..RSDP_V1_LENGTH],
        regions,
    )?;
    validate_rsdp_v1(&rsdp[..RSDP_V1_LENGTH])?;

    let revision = rsdp[15];
    let (root_address, entry_size, expected_signature) = if revision >= 2 {
        read_physical(physical_offset, rsdp_address, &mut rsdp, regions)?;
        validate_rsdp_v2(&rsdp)?;
        let xsdt = read_u64(&rsdp, 24);
        if xsdt != 0 {
            (xsdt, 8, *b"XSDT")
        } else {
            (u64::from(read_u32(&rsdp, 16)), 4, *b"RSDT")
        }
    } else {
        (u64::from(read_u32(&rsdp, 16)), 4, *b"RSDT")
    };
    if root_address == 0 {
        return Err(Error::OutOfRange);
    }

    let root = read_sdt_header(physical_offset, root_address, regions)?;
    if root.signature != expected_signature {
        return Err(Error::InvalidSignature);
    }
    validate_sdt_checksum(physical_offset, root_address, root.length, regions)?;
    let payload = root.length - SDT_HEADER_LENGTH;
    if !payload.is_multiple_of(entry_size) {
        return Err(Error::InvalidLength);
    }

    let total_tables = payload / entry_size;
    let scanned = core::cmp::min(total_tables, MAX_TABLES);
    let mut summary = Summary {
        revision,
        truncated: total_tables > MAX_TABLES,
        ..Summary::default()
    };
    for index in 0..scanned {
        let entry_address = root_address
            .checked_add((SDT_HEADER_LENGTH + index * entry_size) as u64)
            .ok_or(Error::AddressOverflow)?;
        let mut bytes = [0_u8; 8];
        read_physical(
            physical_offset,
            entry_address,
            &mut bytes[..entry_size],
            regions,
        )?;
        let table_address = if entry_size == 8 {
            read_u64(&bytes, 0)
        } else {
            u64::from(read_u32(&bytes, 0))
        };
        let table = read_sdt_header(physical_offset, table_address, regions)?;
        validate_sdt_checksum(physical_offset, table_address, table.length, regions)?;
        summary.tables = summary.tables.saturating_add(1);
        match &table.signature {
            b"APIC" => {
                parse_madt(
                    physical_offset,
                    table_address,
                    table.length,
                    regions,
                    &mut summary,
                )?;
                summary.apic = true;
            }
            b"FACP" => summary.fadt = true,
            b"HPET" => summary.hpet = true,
            b"MCFG" => summary.mcfg = true,
            _ => {}
        }
    }
    Ok(summary)
}

fn parse_madt(
    physical_offset: u64,
    address: u64,
    length: usize,
    regions: &[MemoryRegion],
    summary: &mut Summary,
) -> Result<(), Error> {
    if length < MADT_HEADER_LENGTH {
        return Err(Error::InvalidLength);
    }
    let mut fixed = [0_u8; 8];
    read_physical(
        physical_offset,
        address
            .checked_add(SDT_HEADER_LENGTH as u64)
            .ok_or(Error::AddressOverflow)?,
        &mut fixed,
        regions,
    )?;
    summary.local_apic_address = u64::from(read_u32(&fixed, 0));

    let mut offset = MADT_HEADER_LENGTH;
    let mut entries = 0_usize;
    while offset < length {
        if entries == MAX_MADT_ENTRIES {
            summary.truncated = true;
            break;
        }
        let entry_address = address
            .checked_add(offset as u64)
            .ok_or(Error::AddressOverflow)?;
        let mut header = [0_u8; 2];
        read_physical(physical_offset, entry_address, &mut header, regions)?;
        let entry_length = header[1] as usize;
        if entry_length < 2
            || offset
                .checked_add(entry_length)
                .is_none_or(|end| end > length)
        {
            return Err(Error::InvalidLength);
        }
        let mut entry = [0_u8; MAX_MADT_ENTRY_LENGTH];
        read_physical(
            physical_offset,
            entry_address,
            &mut entry[..entry_length],
            regions,
        )?;
        update_madt_summary(&entry[..entry_length], summary)?;
        entries += 1;
        offset += entry_length;
    }
    summary.madt_entries = entries as u16;
    Ok(())
}

fn update_madt_summary(entry: &[u8], summary: &mut Summary) -> Result<(), Error> {
    if entry.len() < 2 || entry[1] as usize != entry.len() {
        return Err(Error::InvalidLength);
    }
    match entry[0] {
        0 => {
            if entry.len() < 8 {
                return Err(Error::InvalidLength);
            }
            if read_u32(entry, 4) & 3 != 0 {
                summary.enabled_processors = summary.enabled_processors.saturating_add(1);
                let id = if entry[0] == 0 {
                    u32::from(entry[3])
                } else {
                    read_u32(entry, 4)
                };
                if summary.processor_ids[..summary.processor_count].contains(&id) {
                    return Err(Error::InvalidLength);
                }
                if summary.processor_count < summary.processor_ids.len() {
                    summary.processor_ids[summary.processor_count] = id;
                    summary.processor_count += 1;
                } else {
                    summary.truncated = true;
                }
            }
        }
        1 => {
            if entry.len() < 12 {
                return Err(Error::InvalidLength);
            }
            summary.io_apics = summary.io_apics.saturating_add(1);
            if summary.io_apics == 1 {
                summary.io_apic_address = read_u32(entry, 4);
                summary.io_apic_gsi_base = read_u32(entry, 8);
            }
        }
        2 => {
            if entry.len() < 10 {
                return Err(Error::InvalidLength);
            }
            summary.interrupt_overrides = summary.interrupt_overrides.saturating_add(1);
            if entry[2] == 0 && entry[3] < 16 {
                summary.isa_gsi[entry[3] as usize] =
                    Some((read_u32(entry, 4), u16::from_le_bytes([entry[8], entry[9]])));
            }
        }
        5 => {
            if entry.len() < 12 {
                return Err(Error::InvalidLength);
            }
            summary.local_apic_address = read_u64(entry, 4);
        }
        9 => {
            if entry.len() < 16 {
                return Err(Error::InvalidLength);
            }
            if read_u32(entry, 8) & 3 != 0 {
                summary.enabled_processors = summary.enabled_processors.saturating_add(1);
                let id = if entry[0] == 0 {
                    u32::from(entry[3])
                } else {
                    read_u32(entry, 4)
                };
                if summary.processor_ids[..summary.processor_count].contains(&id) {
                    return Err(Error::InvalidLength);
                }
                if summary.processor_count < summary.processor_ids.len() {
                    summary.processor_ids[summary.processor_count] = id;
                    summary.processor_count += 1;
                } else {
                    summary.truncated = true;
                }
            }
        }
        _ => {}
    }
    Ok(())
}
struct SdtHeader {
    signature: [u8; 4],
    length: usize,
}

fn read_sdt_header(
    physical_offset: u64,
    address: u64,
    regions: &[MemoryRegion],
) -> Result<SdtHeader, Error> {
    let mut header = [0_u8; SDT_HEADER_LENGTH];
    read_physical(physical_offset, address, &mut header, regions)?;
    let length = read_u32(&header, 4) as usize;
    if !(SDT_HEADER_LENGTH..=MAX_TABLE_LENGTH).contains(&length) {
        return Err(Error::InvalidLength);
    }
    let mut signature = [0_u8; 4];
    signature.copy_from_slice(&header[..4]);
    Ok(SdtHeader { signature, length })
}

fn validate_sdt_checksum(
    physical_offset: u64,
    address: u64,
    length: usize,
    regions: &[MemoryRegion],
) -> Result<(), Error> {
    validate_range(address, length, regions)?;
    let virtual_address = physical_offset
        .checked_add(address)
        .ok_or(Error::AddressOverflow)?;
    let mut sum = 0_u8;
    for index in 0..length {
        let pointer = (virtual_address as *const u8).wrapping_add(index);
        sum = sum.wrapping_add(unsafe { pointer.read_volatile() });
    }
    if sum == 0 {
        Ok(())
    } else {
        Err(Error::InvalidChecksum)
    }
}

fn read_physical(
    physical_offset: u64,
    address: u64,
    output: &mut [u8],
    regions: &[MemoryRegion],
) -> Result<(), Error> {
    validate_range(address, output.len(), regions)?;
    let virtual_address = physical_offset
        .checked_add(address)
        .ok_or(Error::AddressOverflow)?;
    for (index, byte) in output.iter_mut().enumerate() {
        let pointer = (virtual_address as *const u8).wrapping_add(index);
        *byte = unsafe { pointer.read_volatile() };
    }
    Ok(())
}

fn validate_range(address: u64, length: usize, regions: &[MemoryRegion]) -> Result<(), Error> {
    let end = address
        .checked_add(length as u64)
        .ok_or(Error::AddressOverflow)?;
    if length == 0
        || !regions
            .iter()
            .any(|region| address >= region.start && end <= region.end)
    {
        return Err(Error::OutOfRange);
    }
    Ok(())
}

fn validate_rsdp_v1(bytes: &[u8]) -> Result<(), Error> {
    if bytes.len() < RSDP_V1_LENGTH || &bytes[..8] != b"RSD PTR " {
        return Err(Error::InvalidSignature);
    }
    checksum(&bytes[..RSDP_V1_LENGTH])
}

fn validate_rsdp_v2(bytes: &[u8; RSDP_V2_LENGTH]) -> Result<(), Error> {
    if read_u32(bytes, 20) as usize != RSDP_V2_LENGTH {
        return Err(Error::InvalidLength);
    }
    checksum(bytes)
}

fn checksum(bytes: &[u8]) -> Result<(), Error> {
    if bytes.iter().fold(0_u8, |sum, byte| sum.wrapping_add(*byte)) == 0 {
        Ok(())
    } else {
        Err(Error::InvalidChecksum)
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap_or([0; 4]))
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap_or([0; 8]))
}

pub fn self_test() -> bool {
    let mut rsdp = [0_u8; RSDP_V2_LENGTH];
    rsdp[..8].copy_from_slice(b"RSD PTR ");
    rsdp[15] = 2;
    rsdp[20..24].copy_from_slice(&(RSDP_V2_LENGTH as u32).to_le_bytes());
    rsdp[24..32].copy_from_slice(&0x1234_5000_u64.to_le_bytes());
    rsdp[8] = 0_u8.wrapping_sub(
        rsdp[..RSDP_V1_LENGTH]
            .iter()
            .fold(0_u8, |sum, byte| sum.wrapping_add(*byte)),
    );
    rsdp[32] = 0_u8.wrapping_sub(rsdp.iter().fold(0_u8, |sum, byte| sum.wrapping_add(*byte)));
    let valid =
        validate_rsdp_v1(&rsdp[..RSDP_V1_LENGTH]).is_ok() && validate_rsdp_v2(&rsdp).is_ok();
    rsdp[9] ^= 1;
    let checksum_rejected =
        validate_rsdp_v1(&rsdp[..RSDP_V1_LENGTH]) == Err(Error::InvalidChecksum);

    let mut topology = Summary {
        local_apic_address: 0xfee0_0000,
        ..Summary::default()
    };
    let local_apic = [0_u8, 8, 0, 1, 1, 0, 0, 0];
    let io_apic = [1_u8, 12, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let interrupt_override = [2_u8, 10, 0, 1, 1, 0, 0, 0, 0, 0];
    let mut address_override = [0_u8; 12];
    address_override[0] = 5;
    address_override[1] = 12;
    address_override[4..12].copy_from_slice(&0xfee0_1000_u64.to_le_bytes());
    let topology_valid = update_madt_summary(&local_apic, &mut topology).is_ok()
        && update_madt_summary(&io_apic, &mut topology).is_ok()
        && update_madt_summary(&interrupt_override, &mut topology).is_ok()
        && update_madt_summary(&address_override, &mut topology).is_ok()
        && topology.enabled_processors == 1
        && topology.io_apics == 1
        && topology.interrupt_overrides == 1
        && topology.local_apic_address == 0xfee0_1000;
    let malformed_rejected =
        update_madt_summary(&[0, 7, 0, 0, 0, 0, 0], &mut topology) == Err(Error::InvalidLength);

    valid && checksum_rejected && topology_valid && malformed_rejected
}

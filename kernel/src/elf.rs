use crate::config::MAX_ELF_SEGMENTS as MAX_LOAD_SEGMENTS;

const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;
const PT_LOAD: u32 = 1;
const PF_EXECUTE: u32 = 1;
const PF_WRITE: u32 = 2;
const ELF_MACHINE_X86_64: u16 = 0x3e;
const USER_ADDRESS_LIMIT: u64 = 0x0000_8000_0000_0000;
const PAGE_SIZE: u64 = 4096;

#[derive(Clone, Copy)]
pub struct LoadSegment {
    pub file_offset: usize,
    pub file_size: usize,
    pub virtual_address: u64,
    pub memory_size: usize,
    pub mapping_start: u64,
    pub mapping_size: usize,
    pub writable: bool,
    pub executable: bool,
}

pub struct Image {
    pub entry: u64,
    segments: [Option<LoadSegment>; MAX_LOAD_SEGMENTS],
    segment_count: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Truncated,
    InvalidHeader,
    Unsupported,
    InvalidSegment,
    OverlappingSegments,
    InvalidEntry,
    TooManySegments,
}

impl Image {
    pub fn segments(&self) -> impl Iterator<Item = LoadSegment> + '_ {
        self.segments[..self.segment_count]
            .iter()
            .filter_map(|segment| *segment)
    }

    pub const fn segment_count(&self) -> usize {
        self.segment_count
    }
}

pub fn parse(bytes: &[u8]) -> Result<Image, Error> {
    if bytes.len() < ELF_HEADER_SIZE {
        return Err(Error::Truncated);
    }
    if &bytes[..4] != b"\x7fELF"
        || bytes[4] != 2
        || bytes[5] != 1
        || bytes[6] != 1
        || read_u16(bytes, 16)? != 2
        || read_u16(bytes, 18)? != ELF_MACHINE_X86_64
        || read_u32(bytes, 20)? != 1
        || read_u16(bytes, 52)? as usize != ELF_HEADER_SIZE
    {
        return Err(Error::InvalidHeader);
    }

    let entry = read_u64(bytes, 24)?;
    let program_offset = usize::try_from(read_u64(bytes, 32)?).map_err(|_| Error::Unsupported)?;
    let program_entry_size = read_u16(bytes, 54)? as usize;
    let program_count = read_u16(bytes, 56)? as usize;
    if program_entry_size < PROGRAM_HEADER_SIZE || program_count == 0 {
        return Err(Error::Unsupported);
    }
    let table_size = program_entry_size
        .checked_mul(program_count)
        .ok_or(Error::Truncated)?;
    if program_offset
        .checked_add(table_size)
        .filter(|end| *end <= bytes.len())
        .is_none()
    {
        return Err(Error::Truncated);
    }

    let mut image = Image {
        entry,
        segments: [None; MAX_LOAD_SEGMENTS],
        segment_count: 0,
    };
    let mut entry_is_executable = false;

    for index in 0..program_count {
        let offset = program_offset + index * program_entry_size;
        if read_u32(bytes, offset)? != PT_LOAD {
            continue;
        }
        if image.segment_count == MAX_LOAD_SEGMENTS {
            return Err(Error::TooManySegments);
        }

        let flags = read_u32(bytes, offset + 4)?;
        let file_offset =
            usize::try_from(read_u64(bytes, offset + 8)?).map_err(|_| Error::InvalidSegment)?;
        let virtual_address = read_u64(bytes, offset + 16)?;
        let file_size =
            usize::try_from(read_u64(bytes, offset + 32)?).map_err(|_| Error::InvalidSegment)?;
        let memory_size =
            usize::try_from(read_u64(bytes, offset + 40)?).map_err(|_| Error::InvalidSegment)?;
        let alignment = read_u64(bytes, offset + 48)?;
        let writable = flags & PF_WRITE != 0;
        let executable = flags & PF_EXECUTE != 0;

        if memory_size == 0
            || file_size > memory_size
            || writable && executable
            || alignment == 0
            || !alignment.is_power_of_two()
            || virtual_address % alignment != file_offset as u64 % alignment
        {
            return Err(Error::InvalidSegment);
        }
        let file_end = file_offset
            .checked_add(file_size)
            .filter(|end| *end <= bytes.len())
            .ok_or(Error::Truncated)?;
        let memory_end = virtual_address
            .checked_add(memory_size as u64)
            .filter(|end| *end < USER_ADDRESS_LIMIT)
            .ok_or(Error::InvalidSegment)?;
        let mapping_start = align_down(virtual_address);
        let mapping_end = align_up(memory_end).ok_or(Error::InvalidSegment)?;
        let mapping_size =
            usize::try_from(mapping_end - mapping_start).map_err(|_| Error::InvalidSegment)?;

        for existing in image.segments() {
            let existing_end = existing.mapping_start + existing.mapping_size as u64;
            if mapping_start < existing_end && existing.mapping_start < mapping_end {
                return Err(Error::OverlappingSegments);
            }
        }

        if executable && entry >= virtual_address && entry < memory_end {
            entry_is_executable = true;
        }
        let _ = file_end;
        image.segments[image.segment_count] = Some(LoadSegment {
            file_offset,
            file_size,
            virtual_address,
            memory_size,
            mapping_start,
            mapping_size,
            writable,
            executable,
        });
        image.segment_count += 1;
    }

    if image.segment_count == 0 || !entry_is_executable {
        return Err(Error::InvalidEntry);
    }
    Ok(image)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, Error> {
    let value = bytes.get(offset..offset + 2).ok_or(Error::Truncated)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    let value = bytes.get(offset..offset + 4).ok_or(Error::Truncated)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, Error> {
    let value = bytes.get(offset..offset + 8).ok_or(Error::Truncated)?;
    Ok(u64::from_le_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]))
}

const fn align_down(value: u64) -> u64 {
    value & !(PAGE_SIZE - 1)
}

fn align_up(value: u64) -> Option<u64> {
    value.checked_add(PAGE_SIZE - 1).map(align_down)
}

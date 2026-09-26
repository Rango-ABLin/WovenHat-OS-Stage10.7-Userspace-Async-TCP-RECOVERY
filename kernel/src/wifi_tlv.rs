//! Allocation-free Intel firmware container validation.
//!
//! Format references: Linux v6.12 iwlwifi/fw/file.h, fw/img.h and iwl-drv.c.
//! PAGING is size metadata, not a downloadable section. CPU/paging separators
//! remain explicit records for the chipset loader to interpret before DMA.
//! Parsing a container does not authenticate it or establish ABI compatibility.

pub const INTEL_TLV_UCODE_MAGIC: u32 = 0x0a4c_5749;
pub const INTEL_TLV_UCODE_HEADER_SIZE: usize = 88;
pub const INTEL_TLV_HEADER_SIZE: usize = 8;
pub const INTEL_TLV_SEC_RT: u32 = 19;
pub const INTEL_TLV_SEC_INIT: u32 = 20;
pub const INTEL_TLV_SECURE_SEC_RT: u32 = 24;
pub const INTEL_TLV_SECURE_SEC_INIT: u32 = 25;
pub const INTEL_TLV_PAGING: u32 = 32;
pub const INTEL_CPU_SEPARATOR: u32 = 0xffff_cccc;
pub const INTEL_PAGING_SEPARATOR: u32 = 0xaaaa_bbbb;
pub const INTEL_MAX_IMAGE_SIZE: usize = 4 * 1024 * 1024;
pub const INTEL_MAX_PAGING_SIZE: u32 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelFirmwareImageKind {
    Runtime,
    Init,
    // Kept for manually constructed staging descriptors. The PAGING TLV must
    // never create one: paging data lives in SEC records after a separator.
    Paging,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntelFirmwareSection<'a> {
    pub image: IntelFirmwareImageKind,
    pub device_offset: u32,
    pub bytes: &'a [u8],
}

impl IntelFirmwareSection<'_> {
    pub const fn is_separator(&self) -> bool {
        matches!(
            self.device_offset,
            INTEL_CPU_SEPARATOR | INTEL_PAGING_SEPARATOR
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelTlvError {
    TooShort,
    ImageTooLarge,
    BadZeroPrefix,
    BadMagic,
    LengthOverflow,
    TruncatedTlv,
    InvalidPadding,
    SectionTooShort,
    EmptySection,
    InvalidPagingSize,
    DuplicatePaging,
}

pub struct IntelTlvFirmware<'a> {
    bytes: &'a [u8],
    section_count: usize,
    version: u32,
    build: u32,
    paging_size: Option<u32>,
}

fn read_le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes(slice.try_into().ok()?))
}

fn next_tlv<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
) -> Result<Option<(u32, &'a [u8])>, IntelTlvError> {
    if *cursor == bytes.len() {
        return Ok(None);
    }
    let data_start = cursor
        .checked_add(INTEL_TLV_HEADER_SIZE)
        .ok_or(IntelTlvError::LengthOverflow)?;
    if data_start > bytes.len() {
        return Err(IntelTlvError::TruncatedTlv);
    }
    let kind = read_le_u32(bytes, *cursor).ok_or(IntelTlvError::TruncatedTlv)?;
    let size = read_le_u32(bytes, *cursor + 4).ok_or(IntelTlvError::TruncatedTlv)? as usize;
    let data_end = data_start
        .checked_add(size)
        .ok_or(IntelTlvError::LengthOverflow)?;
    let payload = bytes
        .get(data_start..data_end)
        .ok_or(IntelTlvError::TruncatedTlv)?;
    let aligned = size.checked_add(3).ok_or(IntelTlvError::LengthOverflow)? & !3;
    let next = data_start
        .checked_add(aligned)
        .ok_or(IntelTlvError::LengthOverflow)?;
    if next > bytes.len() {
        return Err(IntelTlvError::InvalidPadding);
    }
    *cursor = next;
    Ok(Some((kind, payload)))
}

fn section_kind(kind: u32) -> Option<IntelFirmwareImageKind> {
    match kind {
        INTEL_TLV_SEC_RT | INTEL_TLV_SECURE_SEC_RT => Some(IntelFirmwareImageKind::Runtime),
        INTEL_TLV_SEC_INIT | INTEL_TLV_SECURE_SEC_INIT => Some(IntelFirmwareImageKind::Init),
        _ => None,
    }
}

impl<'a> IntelTlvFirmware<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, IntelTlvError> {
        if bytes.len() < INTEL_TLV_UCODE_HEADER_SIZE {
            return Err(IntelTlvError::TooShort);
        }
        if bytes.len() > INTEL_MAX_IMAGE_SIZE {
            return Err(IntelTlvError::ImageTooLarge);
        }
        if read_le_u32(bytes, 0) != Some(0) {
            return Err(IntelTlvError::BadZeroPrefix);
        }
        if read_le_u32(bytes, 4) != Some(INTEL_TLV_UCODE_MAGIC) {
            return Err(IntelTlvError::BadMagic);
        }
        let mut parsed = Self {
            bytes,
            section_count: 0,
            version: read_le_u32(bytes, 72).ok_or(IntelTlvError::TooShort)?,
            build: read_le_u32(bytes, 76).ok_or(IntelTlvError::TooShort)?,
            paging_size: None,
        };
        let mut cursor = INTEL_TLV_UCODE_HEADER_SIZE;
        while let Some((kind, payload)) = next_tlv(bytes, &mut cursor)? {
            if section_kind(kind).is_some() {
                let offset = read_le_u32(payload, 0).ok_or(IntelTlvError::SectionTooShort)?;
                if payload.len() == 4
                    && !matches!(offset, INTEL_CPU_SEPARATOR | INTEL_PAGING_SEPARATOR)
                {
                    return Err(IntelTlvError::EmptySection);
                }
                parsed.section_count += 1;
            } else if kind == INTEL_TLV_PAGING {
                if parsed.paging_size.is_some() {
                    return Err(IntelTlvError::DuplicatePaging);
                }
                if payload.len() != 4 {
                    return Err(IntelTlvError::InvalidPagingSize);
                }
                let size = read_le_u32(payload, 0).ok_or(IntelTlvError::InvalidPagingSize)?;
                if size > INTEL_MAX_PAGING_SIZE || size & 4095 != 0 {
                    return Err(IntelTlvError::InvalidPagingSize);
                }
                parsed.paging_size = Some(size);
            }
        }
        Ok(parsed)
    }

    pub const fn version(&self) -> u32 {
        self.version
    }
    pub const fn build(&self) -> u32 {
        self.build
    }
    pub const fn section_count(&self) -> usize {
        self.section_count
    }
    pub const fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
    pub const fn paging_size(&self) -> Option<u32> {
        self.paging_size
    }

    /// Stream validated immutable records without a stack-sized descriptor table.
    /// Hardware ring/map limits are enforced by the loader, not this container.
    pub fn sections(&self) -> IntelSections<'a> {
        IntelSections {
            bytes: self.bytes,
            cursor: INTEL_TLV_UCODE_HEADER_SIZE,
        }
    }

    pub fn section(&self, index: usize) -> Option<IntelFirmwareSection<'a>> {
        if index >= self.section_count {
            return None;
        }
        self.sections().nth(index)
    }
}

pub struct IntelSections<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Iterator for IntelSections<'a> {
    type Item = IntelFirmwareSection<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        // Only a successfully validated, immutably borrowed image creates this
        // iterator. Length/alignment errors cannot appear between iterations.
        while let Some((kind, payload)) = next_tlv(self.bytes, &mut self.cursor).ok()? {
            if let Some(image) = section_kind(kind) {
                return Some(IntelFirmwareSection {
                    image,
                    device_offset: read_le_u32(payload, 0)?,
                    bytes: payload.get(4..)?,
                });
            }
        }
        None
    }
}

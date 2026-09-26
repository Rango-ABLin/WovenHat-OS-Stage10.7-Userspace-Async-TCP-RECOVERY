//! Bounded, allocation-free Intel firmware container parsing.
//! Format references and scope: docs/audit-stage13-10ac-firmware-parser.md.

pub const MAX_FIRMWARE_IMAGE_SIZE: usize = 4 * 1024 * 1024;
pub const MAX_FIRMWARE_SECTIONS: usize = 16;
pub const MAX_FIRMWARE_SECTION_SIZE: usize = 1024 * 1024;
// Matches the existing Intel22000DmaContextInfo owned-buffer capacity. The
// generic FirmwareImage limit stays at 16; parsing does not allocate DMA.
pub const MAX_INTEL_TLV_SECTIONS: usize = 64;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelFirmwareImageKind {
    Runtime,
    Init,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelFirmwareSectionGroup {
    Lmac,
    Umac,
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
    BadZeroPrefix,
    BadMagic,
    LengthOverflow,
    TruncatedTlv,
    InvalidPadding,
    SectionTooShort,
    EmptySection,
    TooManySections,
    ImageTooLarge,
    SectionTooLarge,
    AddressOverflow,
    InvalidPagingSize,
    DuplicatePagingSize,
    InvalidSeparator,
    NoSections,
}

pub struct IntelTlvFirmware<'a> {
    bytes: &'a [u8],
    sections: [Option<IntelFirmwareSection<'a>>; MAX_INTEL_TLV_SECTIONS],
    section_count: usize,
    groups: [IntelFirmwareSectionGroup; MAX_INTEL_TLV_SECTIONS],
    image_groups: [IntelFirmwareSectionGroup; 2],
    paging_size: Option<u32>,
    version: u32,
    build: u32,
}

#[derive(Clone, Copy)]
pub struct IntelSections<'a> {
    sections: [Option<IntelFirmwareSection<'a>>; MAX_INTEL_TLV_SECTIONS],
    section_count: usize,
    index: usize,
}

impl<'a> IntelTlvFirmware<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, IntelTlvError> {
        if bytes.len() > MAX_FIRMWARE_IMAGE_SIZE {
            return Err(IntelTlvError::ImageTooLarge);
        }
        if bytes.len() < INTEL_TLV_UCODE_HEADER_SIZE {
            return Err(IntelTlvError::TooShort);
        }
        if read_le_u32(bytes, 0).ok_or(IntelTlvError::TooShort)? != 0 {
            return Err(IntelTlvError::BadZeroPrefix);
        }
        if read_le_u32(bytes, 4).ok_or(IntelTlvError::TooShort)? != INTEL_TLV_UCODE_MAGIC {
            return Err(IntelTlvError::BadMagic);
        }

        let version = read_le_u32(bytes, 72).ok_or(IntelTlvError::TooShort)?;
        let build = read_le_u32(bytes, 76).ok_or(IntelTlvError::TooShort)?;
        let mut parsed = Self {
            bytes,
            sections: [None; MAX_INTEL_TLV_SECTIONS],
            section_count: 0,
            groups: [IntelFirmwareSectionGroup::Lmac; MAX_INTEL_TLV_SECTIONS],
            image_groups: [IntelFirmwareSectionGroup::Lmac; 2],
            paging_size: None,
            version,
            build,
        };

        let mut cursor = INTEL_TLV_UCODE_HEADER_SIZE;
        // Runtime and init records may be interleaved; delimiters affect only
        // their own image. Delimiters never become DMA payloads.
        let mut image_groups = [IntelFirmwareSectionGroup::Lmac; 2];
        let mut group_has_data = [false; 2];
        while cursor < bytes.len() {
            let header_end = cursor
                .checked_add(INTEL_TLV_HEADER_SIZE)
                .ok_or(IntelTlvError::LengthOverflow)?;
            if header_end > bytes.len() {
                return Err(IntelTlvError::TruncatedTlv);
            }

            let tlv_type = read_le_u32(bytes, cursor).ok_or(IntelTlvError::TruncatedTlv)?;
            let tlv_len = read_le_u32(bytes, cursor + 4).ok_or(IntelTlvError::TruncatedTlv)?;
            let tlv_len = usize::try_from(tlv_len).map_err(|_| IntelTlvError::LengthOverflow)?;

            let data_start = header_end;
            let data_end = data_start
                .checked_add(tlv_len)
                .ok_or(IntelTlvError::LengthOverflow)?;
            if data_end > bytes.len() {
                return Err(IntelTlvError::TruncatedTlv);
            }

            let aligned_len = tlv_len
                .checked_add(3)
                .ok_or(IntelTlvError::LengthOverflow)?
                & !3usize;
            let next = data_start
                .checked_add(aligned_len)
                .ok_or(IntelTlvError::LengthOverflow)?;
            if next > bytes.len() {
                return Err(IntelTlvError::InvalidPadding);
            }

            if tlv_type == INTEL_TLV_PAGING {
                if tlv_len != 4 {
                    return Err(IntelTlvError::InvalidPagingSize);
                }
                let size =
                    read_le_u32(bytes, data_start).ok_or(IntelTlvError::InvalidPagingSize)?;
                if size > MAX_FIRMWARE_SECTION_SIZE as u32 || size % 4096 != 0 {
                    return Err(IntelTlvError::InvalidPagingSize);
                }
                if parsed.paging_size.replace(size).is_some() {
                    return Err(IntelTlvError::DuplicatePagingSize);
                }
            }
            let image = match tlv_type {
                INTEL_TLV_SEC_RT | INTEL_TLV_SECURE_SEC_RT => Some(IntelFirmwareImageKind::Runtime),
                INTEL_TLV_SEC_INIT | INTEL_TLV_SECURE_SEC_INIT => {
                    Some(IntelFirmwareImageKind::Init)
                }
                _ => None,
            };

            if let Some(image) = image {
                if tlv_len < 4 {
                    return Err(IntelTlvError::SectionTooShort);
                }
                let device_offset =
                    read_le_u32(bytes, data_start).ok_or(IntelTlvError::SectionTooShort)?;
                let payload = &bytes[data_start + 4..data_end];
                let image_index = match image {
                    IntelFirmwareImageKind::Runtime => 0,
                    IntelFirmwareImageKind::Init => 1,
                };
                if matches!(device_offset, INTEL_CPU_SEPARATOR | INTEL_PAGING_SEPARATOR) {
                    // Intel emits a zero marker word; also accept an offset-only
                    // delimiter. Never discard arbitrary executable data here.
                    if !payload.is_empty() && payload != [0, 0, 0, 0] {
                        return Err(IntelTlvError::InvalidSeparator);
                    }
                    let next_group = match (image_groups[image_index], device_offset) {
                        (IntelFirmwareSectionGroup::Lmac, INTEL_CPU_SEPARATOR) => {
                            IntelFirmwareSectionGroup::Umac
                        }
                        (IntelFirmwareSectionGroup::Umac, INTEL_PAGING_SEPARATOR) => {
                            IntelFirmwareSectionGroup::Paging
                        }
                        _ => return Err(IntelTlvError::InvalidSeparator),
                    };
                    if !group_has_data[image_index] {
                        return Err(IntelTlvError::InvalidSeparator);
                    }
                    image_groups[image_index] = next_group;
                    group_has_data[image_index] = false;
                    cursor = next;
                    continue;
                }
                if payload.is_empty() {
                    return Err(IntelTlvError::EmptySection);
                }
                if payload.len() > MAX_FIRMWARE_SECTION_SIZE {
                    return Err(IntelTlvError::SectionTooLarge);
                }
                device_offset
                    .checked_add(payload.len() as u32)
                    .ok_or(IntelTlvError::AddressOverflow)?;
                if parsed.section_count >= MAX_INTEL_TLV_SECTIONS {
                    return Err(IntelTlvError::TooManySections);
                }
                parsed.groups[parsed.section_count] = image_groups[image_index];
                group_has_data[image_index] = true;
                parsed.sections[parsed.section_count] = Some(IntelFirmwareSection {
                    image,
                    device_offset,
                    bytes: payload,
                });
                parsed.section_count += 1;
            }

            cursor = next;
        }

        if parsed.section_count == 0 {
            return Err(IntelTlvError::NoSections);
        }
        for index in 0..2 {
            if !group_has_data[index] && image_groups[index] == IntelFirmwareSectionGroup::Umac {
                return Err(IntelTlvError::InvalidSeparator);
            }
        }
        parsed.image_groups = image_groups;
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

    pub fn section(&self, index: usize) -> Option<IntelFirmwareSection<'a>> {
        if index >= self.section_count {
            return None;
        }
        self.sections[index]
    }

    pub const fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    pub const fn paging_size(&self) -> Option<u32> {
        self.paging_size
    }

    pub fn section_group(&self, index: usize) -> Option<IntelFirmwareSectionGroup> {
        (index < self.section_count).then(|| self.groups[index])
    }

    pub fn image_group(&self, image: IntelFirmwareImageKind) -> Option<IntelFirmwareSectionGroup> {
        let index = match image {
            IntelFirmwareImageKind::Runtime => 0,
            IntelFirmwareImageKind::Init => 1,
        };
        Some(self.image_groups[index])
    }

    pub fn sections(&self) -> IntelSections<'a> {
        IntelSections {
            sections: self.sections,
            section_count: self.section_count,
            index: 0,
        }
    }
}

impl<'a> Iterator for IntelSections<'a> {
    type Item = IntelFirmwareSection<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.section_count {
            return None;
        }
        let section = self.sections[self.index];
        self.index += 1;
        section
    }
}

fn read_le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let slice = bytes.get(offset..end)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

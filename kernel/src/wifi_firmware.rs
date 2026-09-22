//! WovenWiFi Stage 13.10K â€” firmware image ownership/validation foundation.
//!
//! This module deliberately does not parse Intel's real TLV firmware format or
//! write firmware to hardware yet. It establishes the fail-closed, bounded
//! representation that later Intel firmware parsing/loading must use.

pub const MAX_FIRMWARE_IMAGE_SIZE: usize = 4 * 1024 * 1024;
pub const MAX_FIRMWARE_SECTIONS: usize = 16;
pub const MAX_FIRMWARE_SECTION_SIZE: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareSectionKind {
    Instruction,
    Data,
    Paging,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmwareSection<'a> {
    pub kind: FirmwareSectionKind,
    pub device_offset: u32,
    pub bytes: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareError {
    EmptyImage,
    ImageTooLarge,
    NoSections,
    TooManySections,
    EmptySection,
    SectionTooLarge,
    SectionOutsideImage,
    AddressOverflow,
    OverlappingDeviceRange,
    InvalidState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareLoadState {
    Empty,
    Validated,
    Loading,
    Loaded,
    Failed,
}

/// Borrowed, validated firmware image.
///
/// Stage K does not allocate or copy firmware. The image and every section are
/// borrowed, so the caller must keep the source bytes alive for this value's
/// lifetime. Later hardware-loading stages can add owned/staged DMA buffers at
/// the boundary where device transfer actually begins.
pub struct FirmwareImage<'a> {
    bytes: &'a [u8],
    sections: &'a [FirmwareSection<'a>],
}

impl<'a> FirmwareImage<'a> {
    pub fn validate(
        bytes: &'a [u8],
        sections: &'a [FirmwareSection<'a>],
    ) -> Result<Self, FirmwareError> {
        if bytes.is_empty() {
            return Err(FirmwareError::EmptyImage);
        }
        if bytes.len() > MAX_FIRMWARE_IMAGE_SIZE {
            return Err(FirmwareError::ImageTooLarge);
        }
        if sections.is_empty() {
            return Err(FirmwareError::NoSections);
        }
        if sections.len() > MAX_FIRMWARE_SECTIONS {
            return Err(FirmwareError::TooManySections);
        }

        let image_start = bytes.as_ptr() as usize;
        let image_end = image_start
            .checked_add(bytes.len())
            .ok_or(FirmwareError::SectionOutsideImage)?;

        for (index, section) in sections.iter().enumerate() {
            if section.bytes.is_empty() {
                return Err(FirmwareError::EmptySection);
            }
            if section.bytes.len() > MAX_FIRMWARE_SECTION_SIZE {
                return Err(FirmwareError::SectionTooLarge);
            }

            let section_start = section.bytes.as_ptr() as usize;
            let section_end = section_start
                .checked_add(section.bytes.len())
                .ok_or(FirmwareError::SectionOutsideImage)?;
            if section_start < image_start || section_end > image_end {
                return Err(FirmwareError::SectionOutsideImage);
            }

            let length =
                u32::try_from(section.bytes.len()).map_err(|_| FirmwareError::AddressOverflow)?;
            let section_device_end = section
                .device_offset
                .checked_add(length)
                .ok_or(FirmwareError::AddressOverflow)?;

            for previous in &sections[..index] {
                let previous_length = u32::try_from(previous.bytes.len())
                    .map_err(|_| FirmwareError::AddressOverflow)?;
                let previous_end = previous
                    .device_offset
                    .checked_add(previous_length)
                    .ok_or(FirmwareError::AddressOverflow)?;
                if section.device_offset < previous_end
                    && previous.device_offset < section_device_end
                {
                    return Err(FirmwareError::OverlappingDeviceRange);
                }
            }
        }

        Ok(Self { bytes, sections })
    }

    pub const fn len(&self) -> usize {
        self.bytes.len()
    }

    pub const fn section_count(&self) -> usize {
        self.sections.len()
    }

    pub fn section(&self, index: usize) -> Option<FirmwareSection<'a>> {
        self.sections.get(index).copied()
    }
}

pub struct FirmwareLoadSession<'a> {
    image: FirmwareImage<'a>,
    state: FirmwareLoadState,
    next_section: usize,
}

impl<'a> FirmwareLoadSession<'a> {
    pub fn new(image: FirmwareImage<'a>) -> Self {
        Self {
            image,
            state: FirmwareLoadState::Validated,
            next_section: 0,
        }
    }

    pub const fn state(&self) -> FirmwareLoadState {
        self.state
    }

    pub const fn next_section_index(&self) -> usize {
        self.next_section
    }

    pub fn begin(&mut self) -> Result<(), FirmwareError> {
        if self.state != FirmwareLoadState::Validated {
            return Err(FirmwareError::InvalidState);
        }
        self.state = FirmwareLoadState::Loading;
        Ok(())
    }

    /// Mark one already-transferred section complete.
    ///
    /// This method does not perform I/O. A later stage must call it only after
    /// the chipset-specific transfer primitive has completed successfully.
    pub fn complete_section(&mut self) -> Result<(), FirmwareError> {
        if self.state != FirmwareLoadState::Loading {
            return Err(FirmwareError::InvalidState);
        }
        if self.next_section >= self.image.section_count() {
            self.state = FirmwareLoadState::Failed;
            return Err(FirmwareError::InvalidState);
        }

        self.next_section += 1;
        if self.next_section == self.image.section_count() {
            self.state = FirmwareLoadState::Loaded;
        }
        Ok(())
    }

    pub fn fail(&mut self) {
        self.state = FirmwareLoadState::Failed;
    }
}

pub fn stage13_10k_self_test() -> bool {
    let bytes = [0x11u8; 96];
    let sections = [
        FirmwareSection {
            kind: FirmwareSectionKind::Instruction,
            device_offset: 0x1000,
            bytes: &bytes[0..32],
        },
        FirmwareSection {
            kind: FirmwareSectionKind::Data,
            device_offset: 0x2000,
            bytes: &bytes[32..64],
        },
        FirmwareSection {
            kind: FirmwareSectionKind::Paging,
            device_offset: 0x3000,
            bytes: &bytes[64..96],
        },
    ];

    let Ok(image) = FirmwareImage::validate(&bytes, &sections) else {
        return false;
    };
    if image.len() != 96
        || image.section_count() != 3
        || image.section(0) != Some(sections[0])
        || image.section(3).is_some()
    {
        return false;
    }

    let mut load = FirmwareLoadSession::new(image);
    if load.state() != FirmwareLoadState::Validated
        || load.complete_section() != Err(FirmwareError::InvalidState)
        || load.begin().is_err()
        || load.state() != FirmwareLoadState::Loading
        || load.complete_section().is_err()
        || load.next_section_index() != 1
        || load.complete_section().is_err()
        || load.next_section_index() != 2
        || load.complete_section().is_err()
        || load.state() != FirmwareLoadState::Loaded
        || load.complete_section() != Err(FirmwareError::InvalidState)
    {
        return false;
    }

    let overlap = [
        FirmwareSection {
            kind: FirmwareSectionKind::Instruction,
            device_offset: 0x1000,
            bytes: &bytes[0..32],
        },
        FirmwareSection {
            kind: FirmwareSectionKind::Data,
            device_offset: 0x1010,
            bytes: &bytes[32..64],
        },
    ];
    if !matches!(
        FirmwareImage::validate(&bytes, &overlap),
        Err(FirmwareError::OverlappingDeviceRange)
    ) {
        return false;
    }

    let external = [0x55u8; 8];
    let outside = [FirmwareSection {
        kind: FirmwareSectionKind::Data,
        device_offset: 0x4000,
        bytes: &external,
    }];
    if !matches!(
        FirmwareImage::validate(&bytes, &outside),
        Err(FirmwareError::SectionOutsideImage)
    ) {
        return false;
    }

    let empty_section = [FirmwareSection {
        kind: FirmwareSectionKind::Data,
        device_offset: 0x5000,
        bytes: &bytes[0..0],
    }];
    if !matches!(
        FirmwareImage::validate(&bytes, &empty_section),
        Err(FirmwareError::EmptySection)
    ) {
        return false;
    }

    let one = [FirmwareSection {
        kind: FirmwareSectionKind::Data,
        device_offset: 0xffff_fff0,
        bytes: &bytes[0..32],
    }];
    matches!(
        FirmwareImage::validate(&bytes, &one),
        Err(FirmwareError::AddressOverflow)
    )
}
pub const INTEL_TLV_UCODE_MAGIC: u32 = 0x0a4c_5749;
pub const INTEL_TLV_UCODE_HEADER_SIZE: usize = 88;
pub const INTEL_TLV_HEADER_SIZE: usize = 8;
pub const INTEL_TLV_SEC_RT: u32 = 19;
pub const INTEL_TLV_SEC_INIT: u32 = 20;
pub const INTEL_TLV_PAGING: u32 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelFirmwareImageKind {
    Runtime,
    Init,
    Paging,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntelFirmwareSection<'a> {
    pub image: IntelFirmwareImageKind,
    pub device_offset: u32,
    pub bytes: &'a [u8],
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
}

pub struct IntelTlvFirmware<'a> {
    bytes: &'a [u8],
    sections: [Option<IntelFirmwareSection<'a>>; MAX_FIRMWARE_SECTIONS],
    section_count: usize,
    version: u32,
    build: u32,
}

impl<'a> IntelTlvFirmware<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, IntelTlvError> {
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
            sections: [None; MAX_FIRMWARE_SECTIONS],
            section_count: 0,
            version,
            build,
        };

        let mut cursor = INTEL_TLV_UCODE_HEADER_SIZE;
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

            let image = match tlv_type {
                INTEL_TLV_SEC_RT => Some(IntelFirmwareImageKind::Runtime),
                INTEL_TLV_SEC_INIT => Some(IntelFirmwareImageKind::Init),
                INTEL_TLV_PAGING => Some(IntelFirmwareImageKind::Paging),
                _ => None,
            };

            if let Some(image) = image {
                if tlv_len < 4 {
                    return Err(IntelTlvError::SectionTooShort);
                }
                if parsed.section_count >= MAX_FIRMWARE_SECTIONS {
                    return Err(IntelTlvError::TooManySections);
                }
                let device_offset =
                    read_le_u32(bytes, data_start).ok_or(IntelTlvError::SectionTooShort)?;
                let payload = &bytes[data_start + 4..data_end];
                if payload.is_empty() {
                    return Err(IntelTlvError::EmptySection);
                }
                parsed.sections[parsed.section_count] = Some(IntelFirmwareSection {
                    image,
                    device_offset,
                    bytes: payload,
                });
                parsed.section_count += 1;
            }

            cursor = next;
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

    pub fn section(&self, index: usize) -> Option<IntelFirmwareSection<'a>> {
        if index >= self.section_count {
            return None;
        }
        self.sections[index]
    }

    pub const fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
}

fn read_le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let slice = bytes.get(offset..end)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

pub fn stage13_10l_self_test() -> bool {
    let mut blob = [0u8; 128];
    blob[4..8].copy_from_slice(&INTEL_TLV_UCODE_MAGIC.to_le_bytes());
    blob[72..76].copy_from_slice(&0x1122_3344u32.to_le_bytes());
    blob[76..80].copy_from_slice(&7u32.to_le_bytes());

    // Runtime SEC TLV. Intel section payload begins with a LE device offset.
    let mut cursor = INTEL_TLV_UCODE_HEADER_SIZE;
    blob[cursor..cursor + 4].copy_from_slice(&INTEL_TLV_SEC_RT.to_le_bytes());
    blob[cursor + 4..cursor + 8].copy_from_slice(&12u32.to_le_bytes());
    blob[cursor + 8..cursor + 12].copy_from_slice(&0x2000u32.to_le_bytes());
    blob[cursor + 12..cursor + 20].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    cursor += 20;

    // Unknown TLV must be safely skipped, including 4-byte alignment padding.
    blob[cursor..cursor + 4].copy_from_slice(&0xfeedu32.to_le_bytes());
    blob[cursor + 4..cursor + 8].copy_from_slice(&3u32.to_le_bytes());
    blob[cursor + 8..cursor + 11].copy_from_slice(&[9, 8, 7]);
    cursor += 12;

    let Ok(parsed) = IntelTlvFirmware::parse(&blob[..cursor]) else {
        return false;
    };
    let Some(section) = parsed.section(0) else {
        return false;
    };
    if parsed.version() != 0x1122_3344
        || parsed.build() != 7
        || parsed.section_count() != 1
        || parsed.section(1).is_some()
        || section.image != IntelFirmwareImageKind::Runtime
        || section.device_offset != 0x2000
        || section.bytes != [1, 2, 3, 4, 5, 6, 7, 8]
        || parsed.bytes().len() != cursor
    {
        return false;
    }

    let mut bad_magic = blob;
    bad_magic[4..8].copy_from_slice(&0u32.to_le_bytes());
    if !matches!(
        IntelTlvFirmware::parse(&bad_magic[..cursor]),
        Err(IntelTlvError::BadMagic)
    ) {
        return false;
    }

    let mut truncated = blob;
    truncated[INTEL_TLV_UCODE_HEADER_SIZE + 4..INTEL_TLV_UCODE_HEADER_SIZE + 8]
        .copy_from_slice(&0x1000u32.to_le_bytes());
    if !matches!(
        IntelTlvFirmware::parse(&truncated[..cursor]),
        Err(IntelTlvError::TruncatedTlv)
    ) {
        return false;
    }

    let mut too_short_section = [0u8; 100];
    too_short_section[4..8].copy_from_slice(&INTEL_TLV_UCODE_MAGIC.to_le_bytes());
    too_short_section[88..92].copy_from_slice(&INTEL_TLV_SEC_INIT.to_le_bytes());
    too_short_section[92..96].copy_from_slice(&3u32.to_le_bytes());
    if !matches!(
        IntelTlvFirmware::parse(&too_short_section),
        Err(IntelTlvError::SectionTooShort)
    ) {
        return false;
    }

    true
}
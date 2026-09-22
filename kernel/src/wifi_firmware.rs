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
pub const INTEL_FIRMWARE_STAGE_CHUNK_SIZE: usize = crate::wifi_hw::DMA_PAGE_SIZE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareStageError {
    InvalidState,
    EmptySection,
    SectionTooLarge,
    AddressOverflow,
    Dma,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareStageState {
    Ready,
    Staging,
    Staged,
    Published,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmwareTransferDescriptor {
    pub device_offset: u32,
    pub physical_address: u64,
    pub length: u16,
}

pub struct IntelFirmwareStager<'a> {
    section: IntelFirmwareSection<'a>,
    state: FirmwareStageState,
    source_offset: usize,
    active: Option<crate::wifi_hw::OwnedDmaBuffer>,
    descriptor: Option<FirmwareTransferDescriptor>,
}

impl<'a> IntelFirmwareStager<'a> {
    pub fn new(section: IntelFirmwareSection<'a>) -> Result<Self, FirmwareStageError> {
        if section.bytes.is_empty() {
            return Err(FirmwareStageError::EmptySection);
        }
        if section.bytes.len() > MAX_FIRMWARE_SECTION_SIZE {
            return Err(FirmwareStageError::SectionTooLarge);
        }
        let length =
            u32::try_from(section.bytes.len()).map_err(|_| FirmwareStageError::AddressOverflow)?;
        section
            .device_offset
            .checked_add(length)
            .ok_or(FirmwareStageError::AddressOverflow)?;

        Ok(Self {
            section,
            state: FirmwareStageState::Ready,
            source_offset: 0,
            active: None,
            descriptor: None,
        })
    }

    pub const fn state(&self) -> FirmwareStageState {
        self.state
    }

    pub const fn source_offset(&self) -> usize {
        self.source_offset
    }

    pub const fn is_complete(&self) -> bool {
        self.source_offset == self.section.bytes.len()
            && matches!(self.state, FirmwareStageState::Completed)
    }

    pub fn stage_next(&mut self) -> Result<FirmwareTransferDescriptor, FirmwareStageError> {
        if self.state != FirmwareStageState::Ready && self.state != FirmwareStageState::Completed {
            return Err(FirmwareStageError::InvalidState);
        }
        if self.source_offset >= self.section.bytes.len() {
            return Err(FirmwareStageError::InvalidState);
        }

        self.state = FirmwareStageState::Staging;
        let remaining = self.section.bytes.len() - self.source_offset;
        let chunk_len = remaining.min(INTEL_FIRMWARE_STAGE_CHUNK_SIZE);
        let length = u16::try_from(chunk_len).map_err(|_| FirmwareStageError::SectionTooLarge)?;
        let source_end = self
            .source_offset
            .checked_add(chunk_len)
            .ok_or(FirmwareStageError::AddressOverflow)?;
        let device_offset = self
            .section
            .device_offset
            .checked_add(
                u32::try_from(self.source_offset)
                    .map_err(|_| FirmwareStageError::AddressOverflow)?,
            )
            .ok_or(FirmwareStageError::AddressOverflow)?;

        let mut buffer = crate::wifi_hw::OwnedDmaBuffer::allocate(length, 0)
            .map_err(|_| FirmwareStageError::Dma)?;
        buffer
            .write_bytes(0, &self.section.bytes[self.source_offset..source_end])
            .map_err(|_| FirmwareStageError::Dma)?;

        let descriptor = FirmwareTransferDescriptor {
            device_offset,
            physical_address: buffer.physical_address(),
            length,
        };
        self.active = Some(buffer);
        self.descriptor = Some(descriptor);
        self.state = FirmwareStageState::Staged;
        Ok(descriptor)
    }

    /// Publish the staged chunk to the future chipset-specific transfer layer.
    ///
    /// Stage M models the ownership boundary but intentionally performs no
    /// Intel FH/PRPH register writes and rings no hardware doorbell.
    pub fn publish(&mut self) -> Result<FirmwareTransferDescriptor, FirmwareStageError> {
        if self.state != FirmwareStageState::Staged {
            return Err(FirmwareStageError::InvalidState);
        }
        let descriptor = self.descriptor.ok_or(FirmwareStageError::InvalidState)?;
        crate::wifi_hw::dma_publish();
        self.state = FirmwareStageState::Published;
        Ok(descriptor)
    }

    /// Complete a chunk only after the future hardware backend reports success.
    pub fn complete(&mut self) -> Result<(), FirmwareStageError> {
        if self.state != FirmwareStageState::Published {
            return Err(FirmwareStageError::InvalidState);
        }
        let descriptor = self.descriptor.ok_or(FirmwareStageError::InvalidState)?;
        crate::wifi_hw::dma_consume();
        self.source_offset = self
            .source_offset
            .checked_add(usize::from(descriptor.length))
            .ok_or(FirmwareStageError::AddressOverflow)?;
        self.active = None;
        self.descriptor = None;
        self.state = FirmwareStageState::Completed;
        Ok(())
    }

    pub fn staged_byte(&self, offset: usize) -> Result<u8, FirmwareStageError> {
        if self.state != FirmwareStageState::Staged && self.state != FirmwareStageState::Published {
            return Err(FirmwareStageError::InvalidState);
        }
        self.active
            .as_ref()
            .ok_or(FirmwareStageError::InvalidState)?
            .read_byte(offset)
            .map_err(|_| FirmwareStageError::Dma)
    }

    pub fn fail(&mut self) {
        self.active = None;
        self.descriptor = None;
        self.state = FirmwareStageState::Failed;
    }
}

pub fn stage13_10m_self_test() -> bool {
    let mut bytes = [0u8; 5000];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = (index & 0xff) as u8;
    }
    let section = IntelFirmwareSection {
        image: IntelFirmwareImageKind::Runtime,
        device_offset: 0x8000,
        bytes: &bytes,
    };
    let Ok(mut stager) = IntelFirmwareStager::new(section) else {
        return false;
    };

    if stager.state() != FirmwareStageState::Ready
        || stager.publish() != Err(FirmwareStageError::InvalidState)
    {
        return false;
    }

    let Ok(first) = stager.stage_next() else {
        return false;
    };
    if first.device_offset != 0x8000
        || usize::from(first.length) != INTEL_FIRMWARE_STAGE_CHUNK_SIZE
        || first.physical_address == 0
        || stager.state() != FirmwareStageState::Staged
        || stager.staged_byte(0) != Ok(bytes[0])
        || stager.staged_byte(4095) != Ok(bytes[4095])
        || stager.complete() != Err(FirmwareStageError::InvalidState)
    {
        return false;
    }
    if stager.publish() != Ok(first)
        || stager.state() != FirmwareStageState::Published
        || stager.complete().is_err()
        || stager.source_offset() != 4096
    {
        return false;
    }

    let Ok(second) = stager.stage_next() else {
        return false;
    };
    if second.device_offset != 0x9000
        || usize::from(second.length) != 904
        || stager.staged_byte(0) != Ok(bytes[4096])
        || stager.staged_byte(903) != Ok(bytes[4999])
        || stager.publish() != Ok(second)
        || stager.complete().is_err()
        || !stager.is_complete()
        || stager.stage_next() != Err(FirmwareStageError::InvalidState)
    {
        return false;
    }

    let overflow_section = IntelFirmwareSection {
        image: IntelFirmwareImageKind::Runtime,
        device_offset: 0xffff_fff0,
        bytes: &bytes[..32],
    };
    if !matches!(
        IntelFirmwareStager::new(overflow_section),
        Err(FirmwareStageError::AddressOverflow)
    ) {
        return false;
    }

    let small = [0x5au8; 32];
    let fail_section = IntelFirmwareSection {
        image: IntelFirmwareImageKind::Runtime,
        device_offset: 0x1000,
        bytes: &small,
    };
    let Ok(mut failed) = IntelFirmwareStager::new(fail_section) else {
        return false;
    };
    if failed.stage_next().is_err() {
        return false;
    }
    failed.fail();
    failed.state() == FirmwareStageState::Failed
        && failed.publish() == Err(FirmwareStageError::InvalidState)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareTransferExecutionState {
    Idle,
    Prepared,
    Started,
    Waiting,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareTransferExecutionError {
    InvalidState,
    InvalidDescriptor,
    BackendRejected,
    Timeout,
    HardwareError,
}

pub trait FirmwareTransferBackend {
    fn prepare(
        &mut self,
        descriptor: FirmwareTransferDescriptor,
    ) -> Result<(), FirmwareTransferExecutionError>;

    fn start(&mut self) -> Result<(), FirmwareTransferExecutionError>;

    fn completed(&mut self) -> Result<bool, FirmwareTransferExecutionError>;
}

pub struct IntelFirmwareTransferExecutor<B> {
    backend: B,
    state: FirmwareTransferExecutionState,
    descriptor: Option<FirmwareTransferDescriptor>,
}

impl<B: FirmwareTransferBackend> IntelFirmwareTransferExecutor<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            state: FirmwareTransferExecutionState::Idle,
            descriptor: None,
        }
    }

    pub const fn state(&self) -> FirmwareTransferExecutionState {
        self.state
    }

    pub fn prepare(
        &mut self,
        descriptor: FirmwareTransferDescriptor,
    ) -> Result<(), FirmwareTransferExecutionError> {
        if self.state != FirmwareTransferExecutionState::Idle {
            return Err(FirmwareTransferExecutionError::InvalidState);
        }

        if descriptor.physical_address == 0 || descriptor.length == 0 {
            self.state = FirmwareTransferExecutionState::Failed;
            return Err(FirmwareTransferExecutionError::InvalidDescriptor);
        }

        if let Err(error) = self.backend.prepare(descriptor) {
            self.state = FirmwareTransferExecutionState::Failed;
            return Err(error);
        }

        self.descriptor = Some(descriptor);
        self.state = FirmwareTransferExecutionState::Prepared;
        Ok(())
    }

    pub fn start(&mut self) -> Result<(), FirmwareTransferExecutionError> {
        if self.state != FirmwareTransferExecutionState::Prepared {
            return Err(FirmwareTransferExecutionError::InvalidState);
        }

        if let Err(error) = self.backend.start() {
            self.state = FirmwareTransferExecutionState::Failed;
            return Err(error);
        }

        self.state = FirmwareTransferExecutionState::Started;
        Ok(())
    }

    pub fn wait_bounded(&mut self, limit: usize) -> Result<(), FirmwareTransferExecutionError> {
        if self.state != FirmwareTransferExecutionState::Started {
            return Err(FirmwareTransferExecutionError::InvalidState);
        }

        if limit == 0 {
            self.state = FirmwareTransferExecutionState::Failed;
            return Err(FirmwareTransferExecutionError::Timeout);
        }

        self.state = FirmwareTransferExecutionState::Waiting;

        for _ in 0..limit {
            match self.backend.completed() {
                Ok(true) => {
                    self.state = FirmwareTransferExecutionState::Completed;
                    return Ok(());
                }
                Ok(false) => {}
                Err(error) => {
                    self.state = FirmwareTransferExecutionState::Failed;
                    return Err(error);
                }
            }
        }

        self.state = FirmwareTransferExecutionState::Failed;
        Err(FirmwareTransferExecutionError::Timeout)
    }

    pub fn into_backend(self) -> B {
        self.backend
    }
}

#[derive(Clone, Copy)]
struct Stage13_10nSyntheticBackend {
    descriptor: Option<FirmwareTransferDescriptor>,
    started: bool,
    polls_before_completion: usize,
    polls: usize,
    fail_start: bool,
}

impl Stage13_10nSyntheticBackend {
    const fn new(polls_before_completion: usize) -> Self {
        Self {
            descriptor: None,
            started: false,
            polls_before_completion,
            polls: 0,
            fail_start: false,
        }
    }
}

impl FirmwareTransferBackend for Stage13_10nSyntheticBackend {
    fn prepare(
        &mut self,
        descriptor: FirmwareTransferDescriptor,
    ) -> Result<(), FirmwareTransferExecutionError> {
        self.descriptor = Some(descriptor);
        Ok(())
    }

    fn start(&mut self) -> Result<(), FirmwareTransferExecutionError> {
        if self.descriptor.is_none() || self.fail_start {
            return Err(FirmwareTransferExecutionError::BackendRejected);
        }

        self.started = true;
        Ok(())
    }

    fn completed(&mut self) -> Result<bool, FirmwareTransferExecutionError> {
        if !self.started {
            return Err(FirmwareTransferExecutionError::InvalidState);
        }

        self.polls = self.polls.saturating_add(1);
        Ok(self.polls >= self.polls_before_completion)
    }
}

pub fn stage13_10n_self_test() -> bool {
    let bytes = [0x5au8; 128];
    let section = IntelFirmwareSection {
        image: IntelFirmwareImageKind::Runtime,
        device_offset: 0x8000,
        bytes: &bytes,
    };

    let Ok(mut stager) = IntelFirmwareStager::new(section) else {
        return false;
    };
    let Ok(descriptor) = stager.stage_next() else {
        return false;
    };
    if stager.publish() != Ok(descriptor) {
        return false;
    }

    let backend = Stage13_10nSyntheticBackend::new(3);
    let mut executor = IntelFirmwareTransferExecutor::new(backend);

    if executor.start() != Err(FirmwareTransferExecutionError::InvalidState)
        || executor.prepare(descriptor).is_err()
        || executor.state() != FirmwareTransferExecutionState::Prepared
        || executor.start().is_err()
        || executor.state() != FirmwareTransferExecutionState::Started
        || executor.wait_bounded(8).is_err()
        || executor.state() != FirmwareTransferExecutionState::Completed
    {
        return false;
    }

    if stager.complete().is_err() || !stager.is_complete() {
        return false;
    }

    let timeout_descriptor = FirmwareTransferDescriptor {
        device_offset: 0x9000,
        physical_address: 0x1000,
        length: 64,
    };
    let timeout_backend = Stage13_10nSyntheticBackend::new(usize::MAX);
    let mut timeout_executor = IntelFirmwareTransferExecutor::new(timeout_backend);

    if timeout_executor.prepare(timeout_descriptor).is_err()
        || timeout_executor.start().is_err()
        || timeout_executor.wait_bounded(4) != Err(FirmwareTransferExecutionError::Timeout)
        || timeout_executor.state() != FirmwareTransferExecutionState::Failed
    {
        return false;
    }

    let invalid_descriptor = FirmwareTransferDescriptor {
        device_offset: 0x1000,
        physical_address: 0,
        length: 32,
    };
    let invalid_backend = Stage13_10nSyntheticBackend::new(1);
    let mut invalid_executor = IntelFirmwareTransferExecutor::new(invalid_backend);

    invalid_executor.prepare(invalid_descriptor)
        == Err(FirmwareTransferExecutionError::InvalidDescriptor)
        && invalid_executor.state() == FirmwareTransferExecutionState::Failed
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intel22000TransferStatus {
    Idle,
    Busy,
    Complete,
    Error,
}

/// Narrow hardware contract for Intel 22000-family firmware DMA transport.
///
/// This is intentionally below `IntelFirmwareTransferExecutor`: the executor
/// owns generic prepare/start/wait policy, while this trait owns the eventual
/// generation-specific register programming.
pub trait Intel22000TransferIo {
    fn program_source(
        &mut self,
        physical_address: u64,
    ) -> Result<(), FirmwareTransferExecutionError>;
    fn program_destination(
        &mut self,
        device_offset: u32,
    ) -> Result<(), FirmwareTransferExecutionError>;
    fn program_length(&mut self, length: u16) -> Result<(), FirmwareTransferExecutionError>;
    fn trigger(&mut self) -> Result<(), FirmwareTransferExecutionError>;
    fn status(&mut self) -> Result<Intel22000TransferStatus, FirmwareTransferExecutionError>;
}

pub struct Intel22000FirmwareTransferBackend<I> {
    io: I,
    prepared: bool,
    started: bool,
}

impl<I: Intel22000TransferIo> Intel22000FirmwareTransferBackend<I> {
    pub const fn new(io: I) -> Self {
        Self {
            io,
            prepared: false,
            started: false,
        }
    }

    pub fn into_io(self) -> I {
        self.io
    }
}

impl<I: Intel22000TransferIo> FirmwareTransferBackend for Intel22000FirmwareTransferBackend<I> {
    fn prepare(
        &mut self,
        descriptor: FirmwareTransferDescriptor,
    ) -> Result<(), FirmwareTransferExecutionError> {
        if self.prepared || self.started {
            return Err(FirmwareTransferExecutionError::InvalidState);
        }
        if descriptor.physical_address == 0 || descriptor.length == 0 {
            return Err(FirmwareTransferExecutionError::InvalidDescriptor);
        }

        self.io.program_source(descriptor.physical_address)?;
        self.io.program_destination(descriptor.device_offset)?;
        self.io.program_length(descriptor.length)?;
        self.prepared = true;
        Ok(())
    }

    fn start(&mut self) -> Result<(), FirmwareTransferExecutionError> {
        if !self.prepared || self.started {
            return Err(FirmwareTransferExecutionError::InvalidState);
        }
        self.io.trigger()?;
        self.started = true;
        Ok(())
    }

    fn completed(&mut self) -> Result<bool, FirmwareTransferExecutionError> {
        if !self.started {
            return Err(FirmwareTransferExecutionError::InvalidState);
        }

        match self.io.status()? {
            Intel22000TransferStatus::Complete => Ok(true),
            Intel22000TransferStatus::Busy => Ok(false),
            Intel22000TransferStatus::Idle => Err(FirmwareTransferExecutionError::HardwareError),
            Intel22000TransferStatus::Error => Err(FirmwareTransferExecutionError::HardwareError),
        }
    }
}

#[derive(Clone, Copy)]
struct Stage13_10oSyntheticIntel22000Io {
    source: u64,
    destination: u32,
    length: u16,
    triggered: bool,
    polls: usize,
    complete_after: usize,
    force_error: bool,
}

impl Stage13_10oSyntheticIntel22000Io {
    const fn new(complete_after: usize) -> Self {
        Self {
            source: 0,
            destination: 0,
            length: 0,
            triggered: false,
            polls: 0,
            complete_after,
            force_error: false,
        }
    }

    const fn programmed(&self) -> bool {
        self.source != 0 && self.length != 0
    }
}

impl Intel22000TransferIo for Stage13_10oSyntheticIntel22000Io {
    fn program_source(
        &mut self,
        physical_address: u64,
    ) -> Result<(), FirmwareTransferExecutionError> {
        if physical_address == 0 {
            return Err(FirmwareTransferExecutionError::InvalidDescriptor);
        }
        self.source = physical_address;
        Ok(())
    }

    fn program_destination(
        &mut self,
        device_offset: u32,
    ) -> Result<(), FirmwareTransferExecutionError> {
        self.destination = device_offset;
        Ok(())
    }

    fn program_length(&mut self, length: u16) -> Result<(), FirmwareTransferExecutionError> {
        if length == 0 {
            return Err(FirmwareTransferExecutionError::InvalidDescriptor);
        }
        self.length = length;
        Ok(())
    }

    fn trigger(&mut self) -> Result<(), FirmwareTransferExecutionError> {
        if !self.programmed() {
            return Err(FirmwareTransferExecutionError::BackendRejected);
        }
        self.triggered = true;
        Ok(())
    }

    fn status(&mut self) -> Result<Intel22000TransferStatus, FirmwareTransferExecutionError> {
        if self.force_error {
            return Ok(Intel22000TransferStatus::Error);
        }
        if !self.triggered {
            return Ok(Intel22000TransferStatus::Idle);
        }
        self.polls = self.polls.saturating_add(1);
        if self.polls >= self.complete_after {
            Ok(Intel22000TransferStatus::Complete)
        } else {
            Ok(Intel22000TransferStatus::Busy)
        }
    }
}

pub fn stage13_10o_self_test() -> bool {
    let descriptor = FirmwareTransferDescriptor {
        device_offset: 0x8000,
        physical_address: 0x0020_0000,
        length: 4096,
    };

    let io = Stage13_10oSyntheticIntel22000Io::new(3);
    let backend = Intel22000FirmwareTransferBackend::new(io);
    let mut executor = IntelFirmwareTransferExecutor::new(backend);

    if executor.prepare(descriptor).is_err()
        || executor.start().is_err()
        || executor.wait_bounded(8).is_err()
        || executor.state() != FirmwareTransferExecutionState::Completed
    {
        return false;
    }

    let backend = executor.into_backend();
    let io = backend.into_io();
    if io.source != descriptor.physical_address
        || io.destination != descriptor.device_offset
        || io.length != descriptor.length
        || !io.triggered
        || io.polls != 3
    {
        return false;
    }

    // A 22000-family transport error must propagate through the generic
    // executor and leave the transfer failed rather than reclaiming DMA.
    let mut error_io = Stage13_10oSyntheticIntel22000Io::new(1);
    error_io.force_error = true;
    let error_backend = Intel22000FirmwareTransferBackend::new(error_io);
    let mut error_executor = IntelFirmwareTransferExecutor::new(error_backend);

    error_executor.prepare(descriptor).is_ok()
        && error_executor.start().is_ok()
        && error_executor.wait_bounded(2) == Err(FirmwareTransferExecutionError::HardwareError)
        && error_executor.state() == FirmwareTransferExecutionState::Failed
}

pub const INTEL_CONTEXT_INFO_MAX_DRAM_ENTRIES: usize = 64;
pub const INTEL_CONTEXT_INFO_MAX_DRAM_CHUNK_SIZE: usize = 32 * 1024;
pub const INTEL_CSR_CONTEXT_INFO_BASE: u32 = 0x40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelContextInfoError {
    ZeroPhysicalAddress,
    EmptySection,
    SectionTooLarge,
    TooManySections,
    MissingQueueAddress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntelContextInfoDramEntry {
    pub physical_address: u64,
    pub length: u32,
}

impl IntelContextInfoDramEntry {
    pub const EMPTY: Self = Self {
        physical_address: 0,
        length: 0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelContextInfoImageKind {
    Lmac,
    Umac,
    Paging,
}

/// AX200/22000-generation context-info DRAM map.
///
/// Linux iwlwifi uses a context-info self-load path for device family 22000:
/// firmware sections are copied to coherent host DRAM and the context-info
/// structure publishes their physical addresses to the device.  This model
/// deliberately keeps the host-side manifest separate from the MMIO kick.
pub struct Intel22000ContextInfoManifest {
    lmac: [IntelContextInfoDramEntry; INTEL_CONTEXT_INFO_MAX_DRAM_ENTRIES],
    umac: [IntelContextInfoDramEntry; INTEL_CONTEXT_INFO_MAX_DRAM_ENTRIES],
    paging: [IntelContextInfoDramEntry; INTEL_CONTEXT_INFO_MAX_DRAM_ENTRIES],
    lmac_count: usize,
    umac_count: usize,
    paging_count: usize,
    free_rbd_address: u64,
    used_rbd_address: u64,
    status_write_pointer: u64,
    command_queue_address: u64,
    command_queue_size: u8,
}

impl Intel22000ContextInfoManifest {
    pub const fn new() -> Self {
        Self {
            lmac: [IntelContextInfoDramEntry::EMPTY; INTEL_CONTEXT_INFO_MAX_DRAM_ENTRIES],
            umac: [IntelContextInfoDramEntry::EMPTY; INTEL_CONTEXT_INFO_MAX_DRAM_ENTRIES],
            paging: [IntelContextInfoDramEntry::EMPTY; INTEL_CONTEXT_INFO_MAX_DRAM_ENTRIES],
            lmac_count: 0,
            umac_count: 0,
            paging_count: 0,
            free_rbd_address: 0,
            used_rbd_address: 0,
            status_write_pointer: 0,
            command_queue_address: 0,
            command_queue_size: 0,
        }
    }

    pub fn set_rx_queue(
        &mut self,
        free_rbd_address: u64,
        used_rbd_address: u64,
        status_write_pointer: u64,
    ) -> Result<(), IntelContextInfoError> {
        if free_rbd_address == 0 || used_rbd_address == 0 || status_write_pointer == 0 {
            return Err(IntelContextInfoError::MissingQueueAddress);
        }
        self.free_rbd_address = free_rbd_address;
        self.used_rbd_address = used_rbd_address;
        self.status_write_pointer = status_write_pointer;
        Ok(())
    }

    pub fn set_command_queue(
        &mut self,
        address: u64,
        size: u8,
    ) -> Result<(), IntelContextInfoError> {
        if address == 0 || size == 0 {
            return Err(IntelContextInfoError::MissingQueueAddress);
        }
        self.command_queue_address = address;
        self.command_queue_size = size;
        Ok(())
    }

    pub fn push(
        &mut self,
        kind: IntelContextInfoImageKind,
        entry: IntelContextInfoDramEntry,
    ) -> Result<(), IntelContextInfoError> {
        if entry.physical_address == 0 {
            return Err(IntelContextInfoError::ZeroPhysicalAddress);
        }
        if entry.length == 0 {
            return Err(IntelContextInfoError::EmptySection);
        }
        if usize::try_from(entry.length).map_err(|_| IntelContextInfoError::SectionTooLarge)?
            > INTEL_CONTEXT_INFO_MAX_DRAM_CHUNK_SIZE
        {
            return Err(IntelContextInfoError::SectionTooLarge);
        }

        let (entries, count) = match kind {
            IntelContextInfoImageKind::Lmac => (&mut self.lmac, &mut self.lmac_count),
            IntelContextInfoImageKind::Umac => (&mut self.umac, &mut self.umac_count),
            IntelContextInfoImageKind::Paging => (&mut self.paging, &mut self.paging_count),
        };

        if *count >= entries.len() {
            return Err(IntelContextInfoError::TooManySections);
        }
        entries[*count] = entry;
        *count += 1;
        Ok(())
    }

    pub const fn lmac_count(&self) -> usize {
        self.lmac_count
    }

    pub const fn umac_count(&self) -> usize {
        self.umac_count
    }

    pub const fn paging_count(&self) -> usize {
        self.paging_count
    }

    pub const fn queues_ready(&self) -> bool {
        self.free_rbd_address != 0
            && self.used_rbd_address != 0
            && self.status_write_pointer != 0
            && self.command_queue_address != 0
            && self.command_queue_size != 0
    }

    pub fn entry(
        &self,
        kind: IntelContextInfoImageKind,
        index: usize,
    ) -> Option<IntelContextInfoDramEntry> {
        let (entries, count) = match kind {
            IntelContextInfoImageKind::Lmac => (&self.lmac, self.lmac_count),
            IntelContextInfoImageKind::Umac => (&self.umac, self.umac_count),
            IntelContextInfoImageKind::Paging => (&self.paging, self.paging_count),
        };
        if index >= count {
            None
        } else {
            Some(entries[index])
        }
    }
}

pub fn stage13_10p_self_test() -> bool {
    let mut manifest = Intel22000ContextInfoManifest::new();

    if manifest.queues_ready()
        || manifest
            .set_rx_queue(0x0010_0000, 0x0011_0000, 0x0012_0000)
            .is_err()
        || manifest.set_command_queue(0x0013_0000, 32).is_err()
        || !manifest.queues_ready()
    {
        return false;
    }

    let lmac = IntelContextInfoDramEntry {
        physical_address: 0x0020_0000,
        length: 4096,
    };
    let umac = IntelContextInfoDramEntry {
        physical_address: 0x0021_0000,
        length: 8192,
    };
    let paging = IntelContextInfoDramEntry {
        physical_address: 0x0022_0000,
        length: 32 * 1024,
    };

    if manifest
        .push(IntelContextInfoImageKind::Lmac, lmac)
        .is_err()
        || manifest
            .push(IntelContextInfoImageKind::Umac, umac)
            .is_err()
        || manifest
            .push(IntelContextInfoImageKind::Paging, paging)
            .is_err()
        || manifest.lmac_count() != 1
        || manifest.umac_count() != 1
        || manifest.paging_count() != 1
        || manifest.entry(IntelContextInfoImageKind::Lmac, 0) != Some(lmac)
        || manifest.entry(IntelContextInfoImageKind::Umac, 0) != Some(umac)
        || manifest.entry(IntelContextInfoImageKind::Paging, 0) != Some(paging)
    {
        return false;
    }

    let too_large = IntelContextInfoDramEntry {
        physical_address: 0x0030_0000,
        length: (INTEL_CONTEXT_INFO_MAX_DRAM_CHUNK_SIZE as u32) + 1,
    };
    let zero_address = IntelContextInfoDramEntry {
        physical_address: 0,
        length: 4096,
    };

    manifest.push(IntelContextInfoImageKind::Lmac, too_large)
        == Err(IntelContextInfoError::SectionTooLarge)
        && manifest.push(IntelContextInfoImageKind::Lmac, zero_address)
            == Err(IntelContextInfoError::ZeroPhysicalAddress)
        && INTEL_CSR_CONTEXT_INFO_BASE == 0x40
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelContextInfoBuildError {
    Manifest(IntelContextInfoError),
    Dma,
    LengthOverflow,
}

pub struct Intel22000DmaContextInfo {
    manifest: Intel22000ContextInfoManifest,
    firmware_dma: [Option<crate::wifi_hw::OwnedDmaBuffer>; INTEL_CONTEXT_INFO_MAX_DRAM_ENTRIES],
    firmware_dma_count: usize,
}

impl Intel22000DmaContextInfo {
    pub fn new() -> Self {
        Self {
            manifest: Intel22000ContextInfoManifest::new(),
            firmware_dma: [const { None }; INTEL_CONTEXT_INFO_MAX_DRAM_ENTRIES],
            firmware_dma_count: 0,
        }
    }
    pub fn manifest(&self) -> &Intel22000ContextInfoManifest {
        &self.manifest
    }
    pub fn set_rx_queue(
        &mut self,
        free: u64,
        used: u64,
        status: u64,
    ) -> Result<(), IntelContextInfoBuildError> {
        self.manifest
            .set_rx_queue(free, used, status)
            .map_err(IntelContextInfoBuildError::Manifest)
    }
    pub fn set_command_queue(
        &mut self,
        address: u64,
        size: u8,
    ) -> Result<(), IntelContextInfoBuildError> {
        self.manifest
            .set_command_queue(address, size)
            .map_err(IntelContextInfoBuildError::Manifest)
    }
    pub fn stage_firmware_chunk(
        &mut self,
        kind: IntelContextInfoImageKind,
        bytes: &[u8],
    ) -> Result<IntelContextInfoDramEntry, IntelContextInfoBuildError> {
        if bytes.is_empty() {
            return Err(IntelContextInfoBuildError::Manifest(
                IntelContextInfoError::EmptySection,
            ));
        }
        if bytes.len() > INTEL_CONTEXT_INFO_MAX_DRAM_CHUNK_SIZE {
            return Err(IntelContextInfoBuildError::Manifest(
                IntelContextInfoError::SectionTooLarge,
            ));
        }
        let length =
            u16::try_from(bytes.len()).map_err(|_| IntelContextInfoBuildError::LengthOverflow)?;
        let manifest_length =
            u32::try_from(bytes.len()).map_err(|_| IntelContextInfoBuildError::LengthOverflow)?;
        let mut buffer = crate::wifi_hw::OwnedDmaBuffer::allocate(length, 0)
            .map_err(|_| IntelContextInfoBuildError::Dma)?;
        buffer
            .write_bytes(0, bytes)
            .map_err(|_| IntelContextInfoBuildError::Dma)?;
        let entry = IntelContextInfoDramEntry {
            physical_address: buffer.physical_address(),
            length: manifest_length,
        };
        if self.firmware_dma_count >= self.firmware_dma.len() {
            return Err(IntelContextInfoBuildError::Manifest(
                IntelContextInfoError::TooManySections,
            ));
        }
        self.manifest
            .push(kind, entry)
            .map_err(IntelContextInfoBuildError::Manifest)?;
        self.firmware_dma[self.firmware_dma_count] = Some(buffer);
        self.firmware_dma_count += 1;
        Ok(entry)
    }
    pub fn firmware_buffer_count(&self) -> usize {
        self.firmware_dma_count
    }
    pub fn staged_byte(
        &self,
        buffer_index: usize,
        offset: usize,
    ) -> Result<u8, IntelContextInfoBuildError> {
        self.firmware_dma
            .get(buffer_index)
            .and_then(Option::as_ref)
            .ok_or(IntelContextInfoBuildError::Dma)?
            .read_byte(offset)
            .map_err(|_| IntelContextInfoBuildError::Dma)
    }
    pub fn ready_for_publication(&self) -> bool {
        self.manifest.queues_ready() && self.firmware_dma_count != 0
    }
}

// === Stage 13.10R: Intel 22000/AX200 context-info hardware ABI ===
pub const INTEL_CONTEXT_INFO_WIRE_SIZE: usize = 1792;
pub const INTEL_CONTEXT_INFO_WIRE_DWORDS: u16 = 448;
pub const INTEL_CONTEXT_INFO_DRAM_UMAC_OFFSET: usize = 192;
pub const INTEL_CONTEXT_INFO_DRAM_LMAC_OFFSET: usize = 704;
pub const INTEL_CONTEXT_INFO_DRAM_PAGING_OFFSET: usize = 1216;
pub const INTEL_CONTEXT_INFO_DEFAULT_CONTROL_FLAGS: u32 = 0x0100 | (8 << 4) | (4 << 9);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelContextInfoAbiError {
    NotReady,
    Dma,
    TooManySections,
    Publication,
}
pub struct Intel22000HardwareContextInfo {
    dma: crate::wifi_hw::OwnedDmaBuffer,
}
impl Intel22000HardwareContextInfo {
    pub const fn physical_address(&self) -> u64 {
        self.dma.physical_address()
    }
    pub fn byte(&self, o: usize) -> Result<u8, IntelContextInfoAbiError> {
        self.dma
            .read_byte(o)
            .map_err(|_| IntelContextInfoAbiError::Dma)
    }
}
fn ci16(x: &mut [u8; INTEL_CONTEXT_INFO_WIRE_SIZE], o: usize, v: u16) {
    x[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn ci32(x: &mut [u8; INTEL_CONTEXT_INFO_WIRE_SIZE], o: usize, v: u32) {
    x[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn ci64(x: &mut [u8; INTEL_CONTEXT_INFO_WIRE_SIZE], o: usize, v: u64) {
    x[o..o + 8].copy_from_slice(&v.to_le_bytes());
}
impl Intel22000DmaContextInfo {
    pub fn build_hardware_context(
        &self,
        mac_id: u16,
    ) -> Result<Intel22000HardwareContextInfo, IntelContextInfoAbiError> {
        if !self.ready_for_publication() {
            return Err(IntelContextInfoAbiError::NotReady);
        }
        let mut x = [0u8; INTEL_CONTEXT_INFO_WIRE_SIZE];
        ci16(&mut x, 0, mac_id);
        ci16(&mut x, 4, INTEL_CONTEXT_INFO_WIRE_DWORDS);
        ci32(&mut x, 8, INTEL_CONTEXT_INFO_DEFAULT_CONTROL_FLAGS);
        ci64(&mut x, 24, self.manifest.free_rbd_address);
        ci64(&mut x, 32, self.manifest.used_rbd_address);
        ci64(&mut x, 40, self.manifest.status_write_pointer);
        ci64(&mut x, 48, self.manifest.command_queue_address);
        x[56] = self.manifest.command_queue_size;
        for (kind, base, count) in [
            (
                IntelContextInfoImageKind::Umac,
                INTEL_CONTEXT_INFO_DRAM_UMAC_OFFSET,
                self.manifest.umac_count(),
            ),
            (
                IntelContextInfoImageKind::Lmac,
                INTEL_CONTEXT_INFO_DRAM_LMAC_OFFSET,
                self.manifest.lmac_count(),
            ),
            (
                IntelContextInfoImageKind::Paging,
                INTEL_CONTEXT_INFO_DRAM_PAGING_OFFSET,
                self.manifest.paging_count(),
            ),
        ] {
            if count > 64 {
                return Err(IntelContextInfoAbiError::TooManySections);
            }
            for i in 0..count {
                let e = self
                    .manifest
                    .entry(kind, i)
                    .ok_or(IntelContextInfoAbiError::TooManySections)?;
                ci64(&mut x, base + i * 8, e.physical_address);
            }
        }
        let mut dma =
            crate::wifi_hw::OwnedDmaBuffer::allocate(INTEL_CONTEXT_INFO_WIRE_SIZE as u16, 0)
                .map_err(|_| IntelContextInfoAbiError::Dma)?;
        dma.write_bytes(0, &x)
            .map_err(|_| IntelContextInfoAbiError::Dma)?;
        Ok(Intel22000HardwareContextInfo { dma })
    }
}
pub trait IntelContextInfoPublicationIo {
    fn write_context_info_base(&mut self, o: u32, a: u64) -> Result<(), IntelContextInfoAbiError>;
}
pub fn publish_hardware_context<I: IntelContextInfoPublicationIo>(
    c: &Intel22000HardwareContextInfo,
    io: &mut I,
) -> Result<(), IntelContextInfoAbiError> {
    crate::wifi_hw::dma_publish();
    io.write_context_info_base(INTEL_CSR_CONTEXT_INFO_BASE, c.physical_address())
        .map_err(|_| IntelContextInfoAbiError::Publication)
}
struct Stage13_10rMockIo {
    offset: u32,
    address: u64,
    writes: usize,
}
impl IntelContextInfoPublicationIo for Stage13_10rMockIo {
    fn write_context_info_base(&mut self, o: u32, a: u64) -> Result<(), IntelContextInfoAbiError> {
        self.offset = o;
        self.address = a;
        self.writes += 1;
        Ok(())
    }
}
// Stage 13.10S
pub const INTEL_UREG_CPU_INIT_RUN: u32 = 0x00a0_5c44;
pub const INTEL_CPU_INIT_RUN_VALUE: u32 = 1;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelFirmwareStartState {
    Reset,
    ContextPublished,
    CpuRunIssued,
    AwaitingAlive,
    FirmwareRunning,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelFirmwareStartError {
    InvalidState,
    Publication(IntelContextInfoAbiError),
    Prph,
}
pub trait IntelFirmwareStartupIo: IntelContextInfoPublicationIo {
    fn write_prph(&mut self, r: u32, v: u32) -> Result<(), IntelFirmwareStartError>;
}
pub struct Intel22000FirmwareStartup<I> {
    io: I,
    state: IntelFirmwareStartState,
}
impl<I: IntelFirmwareStartupIo> Intel22000FirmwareStartup<I> {
    pub fn new(io: I) -> Self {
        Self {
            io,
            state: IntelFirmwareStartState::Reset,
        }
    }
    pub const fn state(&self) -> IntelFirmwareStartState {
        self.state
    }
    pub fn publish_context(
        &mut self,
        c: &Intel22000HardwareContextInfo,
    ) -> Result<(), IntelFirmwareStartError> {
        if self.state != IntelFirmwareStartState::Reset {
            return Err(IntelFirmwareStartError::InvalidState);
        }
        publish_hardware_context(c, &mut self.io).map_err(IntelFirmwareStartError::Publication)?;
        self.state = IntelFirmwareStartState::ContextPublished;
        Ok(())
    }
    pub fn issue_cpu_init_run(&mut self) -> Result<(), IntelFirmwareStartError> {
        if self.state != IntelFirmwareStartState::ContextPublished {
            return Err(IntelFirmwareStartError::InvalidState);
        }
        if self
            .io
            .write_prph(INTEL_UREG_CPU_INIT_RUN, INTEL_CPU_INIT_RUN_VALUE)
            .is_err()
        {
            self.state = IntelFirmwareStartState::Failed;
            return Err(IntelFirmwareStartError::Prph);
        }
        self.state = IntelFirmwareStartState::CpuRunIssued;
        Ok(())
    }
    pub fn begin_alive_wait(&mut self) -> Result<(), IntelFirmwareStartError> {
        if self.state != IntelFirmwareStartState::CpuRunIssued {
            return Err(IntelFirmwareStartError::InvalidState);
        }
        self.state = IntelFirmwareStartState::AwaitingAlive;
        Ok(())
    }
    pub fn into_inner(self) -> I {
        self.io
    }
}
struct Stage13_10sMockIo {
    context_offset: u32,
    context_address: u64,
    context_writes: usize,
    prph_register: u32,
    prph_value: u32,
    prph_writes: usize,
    fail_prph: bool,
}
impl IntelContextInfoPublicationIo for Stage13_10sMockIo {
    fn write_context_info_base(&mut self, o: u32, a: u64) -> Result<(), IntelContextInfoAbiError> {
        self.context_offset = o;
        self.context_address = a;
        self.context_writes += 1;
        Ok(())
    }
}
impl IntelFirmwareStartupIo for Stage13_10sMockIo {
    fn write_prph(&mut self, r: u32, v: u32) -> Result<(), IntelFirmwareStartError> {
        if self.fail_prph {
            return Err(IntelFirmwareStartError::Prph);
        }
        self.prph_register = r;
        self.prph_value = v;
        self.prph_writes += 1;
        Ok(())
    }
}
// === Stage 13.10T: Intel 22000/AX200 ALIVE notification validation ===
pub const INTEL_UCODE_ALIVE_NTFY: u8 = 0x01;
pub const INTEL_ALIVE_STATUS_OK: u16 = 0xCAFE;
pub const INTEL_ALIVE_STATUS_ERR: u16 = 0xDEAD;
pub const INTEL_ALIVE_V3_SIZE: usize = 68;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IntelAliveDiagnostics {
    pub lmac_error_event_table: u32,
    pub lmac_log_event_table: u32,
    pub lmac_cpu_register: u32,
    pub lmac_dbgm_config: u32,
    pub lmac_alive_counter: u32,
    pub lmac_scd_base: u32,
    pub lmac_store_forward_address: u32,
    pub lmac_store_forward_size: u32,
    pub umac_error_info: u32,
    pub umac_debug_print_buffer: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntelAliveInfo {
    pub status: u16,
    pub flags: u16,
    pub lmac_ucode_major: u32,
    pub lmac_ucode_minor: u32,
    pub lmac_ver_subtype: u8,
    pub lmac_ver_type: u8,
    pub lmac_mac: u8,
    pub lmac_opt: u8,
    pub lmac_timestamp: u32,
    pub umac_major: u32,
    pub umac_minor: u32,
    pub diagnostics: IntelAliveDiagnostics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelAliveError {
    InvalidState,
    WrongCommand,
    UnsupportedLength,
    FirmwareRejected(u16),
}

fn alive_u16(payload: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes([
        *payload.get(offset)?,
        *payload.get(offset + 1)?,
    ]))
}
fn alive_u32(payload: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *payload.get(offset)?,
        *payload.get(offset + 1)?,
        *payload.get(offset + 2)?,
        *payload.get(offset + 3)?,
    ]))
}

pub fn parse_intel_alive_v3(payload: &[u8]) -> Result<IntelAliveInfo, IntelAliveError> {
    if payload.len() != INTEL_ALIVE_V3_SIZE {
        return Err(IntelAliveError::UnsupportedLength);
    }
    let status = alive_u16(payload, 0).ok_or(IntelAliveError::UnsupportedLength)?;
    let flags = alive_u16(payload, 2).ok_or(IntelAliveError::UnsupportedLength)?;
    let diagnostics = IntelAliveDiagnostics {
        lmac_error_event_table: alive_u32(payload, 20).ok_or(IntelAliveError::UnsupportedLength)?,
        lmac_log_event_table: alive_u32(payload, 24).ok_or(IntelAliveError::UnsupportedLength)?,
        lmac_cpu_register: alive_u32(payload, 28).ok_or(IntelAliveError::UnsupportedLength)?,
        lmac_dbgm_config: alive_u32(payload, 32).ok_or(IntelAliveError::UnsupportedLength)?,
        lmac_alive_counter: alive_u32(payload, 36).ok_or(IntelAliveError::UnsupportedLength)?,
        lmac_scd_base: alive_u32(payload, 40).ok_or(IntelAliveError::UnsupportedLength)?,
        lmac_store_forward_address: alive_u32(payload, 44)
            .ok_or(IntelAliveError::UnsupportedLength)?,
        lmac_store_forward_size: alive_u32(payload, 48)
            .ok_or(IntelAliveError::UnsupportedLength)?,
        umac_error_info: alive_u32(payload, 60).ok_or(IntelAliveError::UnsupportedLength)?,
        umac_debug_print_buffer: alive_u32(payload, 64)
            .ok_or(IntelAliveError::UnsupportedLength)?,
    };
    Ok(IntelAliveInfo {
        status,
        flags,
        lmac_ucode_major: alive_u32(payload, 4).ok_or(IntelAliveError::UnsupportedLength)?,
        lmac_ucode_minor: alive_u32(payload, 8).ok_or(IntelAliveError::UnsupportedLength)?,
        lmac_ver_subtype: payload[12],
        lmac_ver_type: payload[13],
        lmac_mac: payload[14],
        lmac_opt: payload[15],
        lmac_timestamp: alive_u32(payload, 16).ok_or(IntelAliveError::UnsupportedLength)?,
        umac_major: alive_u32(payload, 52).ok_or(IntelAliveError::UnsupportedLength)?,
        umac_minor: alive_u32(payload, 56).ok_or(IntelAliveError::UnsupportedLength)?,
        diagnostics,
    })
}

impl<I: IntelFirmwareStartupIo> Intel22000FirmwareStartup<I> {
    pub fn handle_alive_notification(
        &mut self,
        command: u8,
        payload: &[u8],
    ) -> Result<IntelAliveInfo, IntelAliveError> {
        if self.state != IntelFirmwareStartState::AwaitingAlive {
            return Err(IntelAliveError::InvalidState);
        }
        if command != INTEL_UCODE_ALIVE_NTFY {
            return Err(IntelAliveError::WrongCommand);
        }
        let info = match parse_intel_alive_v3(payload) {
            Ok(info) => info,
            Err(err) => {
                self.state = IntelFirmwareStartState::Failed;
                return Err(err);
            }
        };
        if info.status != INTEL_ALIVE_STATUS_OK {
            self.state = IntelFirmwareStartState::Failed;
            return Err(IntelAliveError::FirmwareRejected(info.status));
        }
        self.state = IntelFirmwareStartState::FirmwareRunning;
        Ok(info)
    }
}

fn stage13_10t_put16(payload: &mut [u8; INTEL_ALIVE_V3_SIZE], offset: usize, value: u16) {
    payload[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn stage13_10t_put32(payload: &mut [u8; INTEL_ALIVE_V3_SIZE], offset: usize, value: u32) {
    payload[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn stage13_10t_valid_payload(status: u16) -> [u8; INTEL_ALIVE_V3_SIZE] {
    let mut payload = [0u8; INTEL_ALIVE_V3_SIZE];
    stage13_10t_put16(&mut payload, 0, status);
    stage13_10t_put16(&mut payload, 2, 1);
    stage13_10t_put32(&mut payload, 4, 0x1122_3344);
    stage13_10t_put32(&mut payload, 8, 0x5566_7788);
    payload[12] = 9;
    payload[13] = 0;
    payload[14] = 1;
    payload[15] = 2;
    stage13_10t_put32(&mut payload, 16, 0x0102_0304);
    stage13_10t_put32(&mut payload, 20, 0x1000_1000);
    stage13_10t_put32(&mut payload, 24, 0x1000_2000);
    stage13_10t_put32(&mut payload, 28, 0x1000_3000);
    stage13_10t_put32(&mut payload, 32, 0x1000_4000);
    stage13_10t_put32(&mut payload, 36, 0x1000_5000);
    stage13_10t_put32(&mut payload, 40, 0x1000_6000);
    stage13_10t_put32(&mut payload, 44, 0x1000_7000);
    stage13_10t_put32(&mut payload, 48, 0x800);
    stage13_10t_put32(&mut payload, 52, 0xa1a2_a3a4);
    stage13_10t_put32(&mut payload, 56, 0xb1b2_b3b4);
    stage13_10t_put32(&mut payload, 60, 0x2000_1000);
    stage13_10t_put32(&mut payload, 64, 0x2000_2000);
    payload
}

pub fn stage13_10t_self_test() -> bool {
    let ok = stage13_10t_valid_payload(INTEL_ALIVE_STATUS_OK);
    let Ok(parsed) = parse_intel_alive_v3(&ok) else {
        return false;
    };
    if parsed.status != INTEL_ALIVE_STATUS_OK
        || parsed.flags != 1
        || parsed.lmac_ucode_major != 0x1122_3344
        || parsed.lmac_ucode_minor != 0x5566_7788
        || parsed.lmac_ver_subtype != 9
        || parsed.lmac_timestamp != 0x0102_0304
        || parsed.umac_major != 0xa1a2_a3a4
        || parsed.umac_minor != 0xb1b2_b3b4
        || parsed.diagnostics.lmac_error_event_table != 0x1000_1000
        || parsed.diagnostics.lmac_scd_base != 0x1000_6000
        || parsed.diagnostics.umac_error_info != 0x2000_1000
        || parse_intel_alive_v3(&ok[..67]) != Err(IntelAliveError::UnsupportedLength)
    {
        return false;
    }

    let mut owner = Intel22000DmaContextInfo::new();
    if owner.set_rx_queue(0x100000, 0x110000, 0x120000).is_err()
        || owner.set_command_queue(0x130000, 32).is_err()
        || owner
            .stage_firmware_chunk(IntelContextInfoImageKind::Lmac, &[0x31; 512])
            .is_err()
        || owner
            .stage_firmware_chunk(IntelContextInfoImageKind::Umac, &[0x42; 256])
            .is_err()
    {
        return false;
    }
    let Ok(context) = owner.build_hardware_context(0x2200) else {
        return false;
    };

    let io = Stage13_10sMockIo {
        context_offset: 0,
        context_address: 0,
        context_writes: 0,
        prph_register: 0,
        prph_value: 0,
        prph_writes: 0,
        fail_prph: false,
    };
    let mut s = Intel22000FirmwareStartup::new(io);
    if s.publish_context(&context).is_err()
        || s.issue_cpu_init_run().is_err()
        || s.begin_alive_wait().is_err()
        || s.handle_alive_notification(INTEL_UCODE_ALIVE_NTFY, &ok)
            .is_err()
        || s.state() != IntelFirmwareStartState::FirmwareRunning
    {
        return false;
    }

    let io2 = Stage13_10sMockIo {
        context_offset: 0,
        context_address: 0,
        context_writes: 0,
        prph_register: 0,
        prph_value: 0,
        prph_writes: 0,
        fail_prph: false,
    };
    let mut wrong = Intel22000FirmwareStartup::new(io2);
    if wrong.publish_context(&context).is_err()
        || wrong.issue_cpu_init_run().is_err()
        || wrong.begin_alive_wait().is_err()
        || wrong.handle_alive_notification(0x7f, &ok) != Err(IntelAliveError::WrongCommand)
        || wrong.state() != IntelFirmwareStartState::AwaitingAlive
    {
        return false;
    }

    let io3 = Stage13_10sMockIo {
        context_offset: 0,
        context_address: 0,
        context_writes: 0,
        prph_register: 0,
        prph_value: 0,
        prph_writes: 0,
        fail_prph: false,
    };
    let mut rejected = Intel22000FirmwareStartup::new(io3);
    let bad = stage13_10t_valid_payload(INTEL_ALIVE_STATUS_ERR);
    rejected.publish_context(&context).is_ok()
        && rejected.issue_cpu_init_run().is_ok()
        && rejected.begin_alive_wait().is_ok()
        && rejected.handle_alive_notification(INTEL_UCODE_ALIVE_NTFY, &bad)
            == Err(IntelAliveError::FirmwareRejected(INTEL_ALIVE_STATUS_ERR))
        && rejected.state() == IntelFirmwareStartState::Failed
}

pub fn stage13_10s_self_test() -> bool {
    let mut owner = Intel22000DmaContextInfo::new();
    if owner.set_rx_queue(0x100000, 0x110000, 0x120000).is_err()
        || owner.set_command_queue(0x130000, 32).is_err()
        || owner
            .stage_firmware_chunk(IntelContextInfoImageKind::Lmac, &[0x31; 512])
            .is_err()
        || owner
            .stage_firmware_chunk(IntelContextInfoImageKind::Umac, &[0x42; 256])
            .is_err()
    {
        return false;
    }
    let Ok(context) = owner.build_hardware_context(0x2200) else {
        return false;
    };
    let io = Stage13_10sMockIo {
        context_offset: 0,
        context_address: 0,
        context_writes: 0,
        prph_register: 0,
        prph_value: 0,
        prph_writes: 0,
        fail_prph: false,
    };
    let mut s = Intel22000FirmwareStartup::new(io);
    if s.issue_cpu_init_run() != Err(IntelFirmwareStartError::InvalidState)
        || s.publish_context(&context).is_err()
        || s.issue_cpu_init_run().is_err()
        || s.begin_alive_wait().is_err()
        || s.state() != IntelFirmwareStartState::AwaitingAlive
    {
        return false;
    }
    let io = s.into_inner();
    if io.context_writes != 1
        || io.context_offset != INTEL_CSR_CONTEXT_INFO_BASE
        || io.context_address != context.physical_address()
        || io.prph_writes != 1
        || io.prph_register != INTEL_UREG_CPU_INIT_RUN
        || io.prph_value != 1
    {
        return false;
    }
    let fio = Stage13_10sMockIo {
        context_offset: 0,
        context_address: 0,
        context_writes: 0,
        prph_register: 0,
        prph_value: 0,
        prph_writes: 0,
        fail_prph: true,
    };
    let mut f = Intel22000FirmwareStartup::new(fio);
    f.publish_context(&context).is_ok()
        && f.issue_cpu_init_run() == Err(IntelFirmwareStartError::Prph)
        && f.state() == IntelFirmwareStartState::Failed
}

pub fn stage13_10r_self_test() -> bool {
    let mut c = Intel22000DmaContextInfo::new();
    if c.set_rx_queue(0x100000, 0x110000, 0x120000).is_err()
        || c.set_command_queue(0x130000, 32).is_err()
        || c.stage_firmware_chunk(IntelContextInfoImageKind::Lmac, &[0x11; 256])
            .is_err()
        || c.stage_firmware_chunk(IntelContextInfoImageKind::Umac, &[0x22; 128])
            .is_err()
    {
        return false;
    }
    let Ok(h) = c.build_hardware_context(0x1234) else {
        return false;
    };
    let r16 = |o| Some(u16::from_le_bytes([h.byte(o).ok()?, h.byte(o + 1).ok()?]));
    let r64 = |o| {
        let mut b = [0; 8];
        for (i, v) in b.iter_mut().enumerate() {
            *v = h.byte(o + i).ok()?;
        }
        Some(u64::from_le_bytes(b))
    };
    if r16(0) != Some(0x1234)
        || r16(4) != Some(INTEL_CONTEXT_INFO_WIRE_DWORDS)
        || r64(24) != Some(0x100000)
        || r64(48) != Some(0x130000)
    {
        return false;
    }
    let mut io = Stage13_10rMockIo {
        offset: 0,
        address: 0,
        writes: 0,
    };
    publish_hardware_context(&h, &mut io).is_ok()
        && io.offset == 0x40
        && io.address == h.physical_address()
        && io.address != 0
        && io.writes == 1
}

pub fn stage13_10q_self_test() -> bool {
    let mut context = Intel22000DmaContextInfo::new();
    if context
        .set_rx_queue(0x0010_0000, 0x0011_0000, 0x0012_0000)
        .is_err()
        || context.set_command_queue(0x0013_0000, 32).is_err()
        || context.ready_for_publication()
    {
        return false;
    }

    let mut lmac = [0u8; 4096];
    for (index, byte) in lmac.iter_mut().enumerate() {
        *byte = (index & 0xff) as u8;
    }
    let umac = [0xa5u8; 1024];
    let Ok(lmac_entry) = context.stage_firmware_chunk(IntelContextInfoImageKind::Lmac, &lmac)
    else {
        return false;
    };
    let Ok(umac_entry) = context.stage_firmware_chunk(IntelContextInfoImageKind::Umac, &umac)
    else {
        return false;
    };

    if lmac_entry.physical_address == 0
        || umac_entry.physical_address == 0
        || lmac_entry.physical_address == umac_entry.physical_address
        || lmac_entry.length != 4096
        || umac_entry.length != 1024
        || context.firmware_buffer_count() != 2
        || context.manifest().lmac_count() != 1
        || context.manifest().umac_count() != 1
        || context.manifest().entry(IntelContextInfoImageKind::Lmac, 0) != Some(lmac_entry)
        || context.manifest().entry(IntelContextInfoImageKind::Umac, 0) != Some(umac_entry)
        || context.staged_byte(0, 0) != Ok(lmac[0])
        || context.staged_byte(0, 4095) != Ok(lmac[4095])
        || context.staged_byte(1, 0) != Ok(0xa5)
        || !context.ready_for_publication()
    {
        return false;
    }

    let large = [0x6cu8; INTEL_CONTEXT_INFO_MAX_DRAM_CHUNK_SIZE];
    let Ok(large_entry) = context.stage_firmware_chunk(IntelContextInfoImageKind::Paging, &large)
    else {
        return false;
    };
    if large_entry.length != INTEL_CONTEXT_INFO_MAX_DRAM_CHUNK_SIZE as u32
        || context.staged_byte(2, 0) != Ok(0x6c)
        || context.staged_byte(2, INTEL_CONTEXT_INFO_MAX_DRAM_CHUNK_SIZE - 1) != Ok(0x6c)
    {
        return false;
    }

    let too_large = [0u8; INTEL_CONTEXT_INFO_MAX_DRAM_CHUNK_SIZE + 1];
    context.stage_firmware_chunk(IntelContextInfoImageKind::Paging, &too_large)
        == Err(IntelContextInfoBuildError::Manifest(
            IntelContextInfoError::SectionTooLarge,
        ))
        && context.firmware_buffer_count() == 3
        && context.manifest().paging_count() == 1
}

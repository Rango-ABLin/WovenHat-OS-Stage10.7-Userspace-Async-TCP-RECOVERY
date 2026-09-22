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
//! Preflight the AX200 runtime image before any DMA allocation/publication.
//! Section ordering follows the 22000 context-info format (Linux v6.12
//! iwlwifi/pcie/ctxt-info.c). This is layout validation, not authentication.

use super::tlv::{
    IntelFirmwareImageKind, IntelFirmwareSectionGroup, IntelTlvFirmware,
};

pub const AX200_MAX_IMAGE_BUFFERS: usize = 64;
pub const AX200_MAX_IMAGE_CHUNK: usize = 32 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ax200ImageRegion {
    Lmac,
    Umac,
    Paging,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ax200ImageError {
    SeparatorOrder,
    EmptyRegion,
    MissingSeparator,
    SectionTooLarge,
    TooManySections,
    PagingMetadata,
}

pub struct Ax200RuntimeImage<'a> {
    firmware: &'a IntelTlvFirmware<'a>,
    counts: [usize; 3],
}

impl<'a> Ax200RuntimeImage<'a> {
    pub fn validate(firmware: &'a IntelTlvFirmware<'a>) -> Result<Self, Ax200ImageError> {
        let mut region = 0usize;
        let mut counts = [0usize; 3];
        let mut total = 0;
        for index in 0..firmware.section_count() {
            let Some(section) = firmware.section(index) else {
                continue;
            };
            if section.image != IntelFirmwareImageKind::Runtime {
                continue;
            }
            let Some(group) = firmware.section_group(index) else {
                return Err(Ax200ImageError::SeparatorOrder);
            };
            let group_region = match group {
                IntelFirmwareSectionGroup::Lmac => 0,
                IntelFirmwareSectionGroup::Umac => 1,
                IntelFirmwareSectionGroup::Paging => 2,
            };
            if group_region < region || group_region > region + 1 {
                return Err(Ax200ImageError::SeparatorOrder);
            }
            if group_region > region {
                if counts[region] == 0 {
                    return Err(Ax200ImageError::EmptyRegion);
                }
                region = group_region;
            }
            if section.bytes.len() > AX200_MAX_IMAGE_CHUNK {
                return Err(Ax200ImageError::SectionTooLarge);
            }
            total += 1;
            if total > AX200_MAX_IMAGE_BUFFERS {
                return Err(Ax200ImageError::TooManySections);
            }
            counts[region] += 1;
        }
        if region != 2 {
            return Err(Ax200ImageError::MissingSeparator);
        }
        if (firmware.paging_size().unwrap_or(0) != 0) != (counts[2] != 0) {
            return Err(Ax200ImageError::PagingMetadata);
        }
        Ok(Self { firmware, counts })
    }

    pub const fn counts(&self) -> [usize; 3] {
        self.counts
    }

    pub fn payloads(self) -> Ax200Payloads<'a> {
        Ax200Payloads {
            firmware: self.firmware,
            index: 0,
        }
    }
}

pub struct Ax200Payloads<'a> {
    firmware: &'a IntelTlvFirmware<'a>,
    index: usize,
}

impl<'a> Iterator for Ax200Payloads<'a> {
    type Item = (Ax200ImageRegion, &'a [u8]);
    fn next(&mut self) -> Option<Self::Item> {
        while self.index < self.firmware.section_count() {
            let index = self.index;
            self.index += 1;
            let section = self.firmware.section(index)?;
            if section.image != IntelFirmwareImageKind::Runtime {
                continue;
            }
            let group = self.firmware.section_group(index)?;
            let region = match group {
                IntelFirmwareSectionGroup::Lmac => Ax200ImageRegion::Lmac,
                IntelFirmwareSectionGroup::Umac => Ax200ImageRegion::Umac,
                IntelFirmwareSectionGroup::Paging => Ax200ImageRegion::Paging,
            };
            return Some((region, section.bytes));
        }
        None
    }
}

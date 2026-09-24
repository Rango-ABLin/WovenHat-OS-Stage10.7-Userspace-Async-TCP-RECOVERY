//! Preflight the AX200 runtime image before any DMA allocation/publication.
//! Section ordering follows the 22000 context-info format (Linux v6.12
//! iwlwifi/pcie/ctxt-info.c). This is layout validation, not authentication.

use super::tlv::{
    IntelFirmwareImageKind, IntelSections, IntelTlvFirmware, INTEL_CPU_SEPARATOR,
    INTEL_PAGING_SEPARATOR,
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
    sections: IntelSections<'a>,
    counts: [usize; 3],
}

impl<'a> Ax200RuntimeImage<'a> {
    pub fn validate(firmware: &IntelTlvFirmware<'a>) -> Result<Self, Ax200ImageError> {
        let mut region = 0;
        let mut counts = [0usize; 3];
        let mut total = 0;
        for section in firmware
            .sections()
            .filter(|s| s.image == IntelFirmwareImageKind::Runtime)
        {
            match section.device_offset {
                INTEL_CPU_SEPARATOR => {
                    if region != 0 {
                        return Err(Ax200ImageError::SeparatorOrder);
                    }
                    if counts[0] == 0 {
                        return Err(Ax200ImageError::EmptyRegion);
                    }
                    region = 1;
                }
                INTEL_PAGING_SEPARATOR => {
                    if region != 1 {
                        return Err(Ax200ImageError::SeparatorOrder);
                    }
                    if counts[1] == 0 {
                        return Err(Ax200ImageError::EmptyRegion);
                    }
                    region = 2;
                }
                _ => {
                    if section.bytes.len() > AX200_MAX_IMAGE_CHUNK {
                        return Err(Ax200ImageError::SectionTooLarge);
                    }
                    total += 1;
                    if total > AX200_MAX_IMAGE_BUFFERS {
                        return Err(Ax200ImageError::TooManySections);
                    }
                    counts[region] += 1;
                }
            }
        }
        if region != 2 {
            return Err(Ax200ImageError::MissingSeparator);
        }
        if (firmware.paging_size().unwrap_or(0) != 0) != (counts[2] != 0) {
            return Err(Ax200ImageError::PagingMetadata);
        }
        Ok(Self {
            sections: firmware.sections(),
            counts,
        })
    }

    pub const fn counts(&self) -> [usize; 3] {
        self.counts
    }

    pub fn payloads(self) -> Ax200Payloads<'a> {
        Ax200Payloads {
            sections: self.sections,
            region: Ax200ImageRegion::Lmac,
        }
    }
}

pub struct Ax200Payloads<'a> {
    sections: IntelSections<'a>,
    region: Ax200ImageRegion,
}

impl<'a> Iterator for Ax200Payloads<'a> {
    type Item = (Ax200ImageRegion, &'a [u8]);
    fn next(&mut self) -> Option<Self::Item> {
        for section in self.sections.by_ref() {
            if section.image != IntelFirmwareImageKind::Runtime {
                continue;
            }
            match section.device_offset {
                INTEL_CPU_SEPARATOR => self.region = Ax200ImageRegion::Umac,
                INTEL_PAGING_SEPARATOR => self.region = Ax200ImageRegion::Paging,
                _ => return Some((self.region, section.bytes)),
            }
        }
        None
    }
}

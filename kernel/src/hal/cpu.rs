use core::arch::x86_64::__cpuid;

use super::{CpuFeatures, CpuVendor};

const FEATURE_INFO: u32 = 1;
const EXTENDED_FEATURE_INFO: u32 = 0x8000_0001;
const TSC: u32 = 1 << 4;
const SSE4_2: u32 = 1 << 20;
const AES_NI: u32 = 1 << 25;
const AVX: u32 = 1 << 28;
const RDRAND: u32 = 1 << 30;
const PAE: u32 = 1 << 6;

pub fn detect_vendor() -> CpuVendor {
    let vendor = vendor_string();
    match vendor.as_slice() {
        b"GenuineIntel" => CpuVendor::Intel,
        b"AuthenticAMD" => CpuVendor::Amd,
        _ => CpuVendor::Unknown,
    }
}

pub fn detect_features() -> CpuFeatures {
    let basic = __cpuid(FEATURE_INFO);
    let extended = __cpuid(EXTENDED_FEATURE_INFO);

    CpuFeatures {
        has_tsc: basic.edx & TSC != 0,
        has_rdrand: basic.ecx & RDRAND != 0,
        has_aes_ni: basic.ecx & AES_NI != 0,
        has_avx: basic.ecx & AVX != 0,
        has_pae: extended.edx & PAE != 0,
        has_sse4_2: basic.ecx & SSE4_2 != 0,
    }
}

pub fn count_logical_cpus() -> u32 {
    let basic = __cpuid(FEATURE_INFO);
    let count = ((basic.ebx >> 16) & 0xff) as u32;
    count.max(1)
}

fn vendor_string() -> [u8; 12] {
    let result = __cpuid(0);
    let mut vendor = [0; 12];
    vendor[0..4].copy_from_slice(&result.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&result.edx.to_le_bytes());
    vendor[8..12].copy_from_slice(&result.ecx.to_le_bytes());
    vendor
}

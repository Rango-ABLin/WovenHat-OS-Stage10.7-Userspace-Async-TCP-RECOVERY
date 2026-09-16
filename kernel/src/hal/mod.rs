pub mod acpi;
pub mod cpu;
pub mod pci;

#[derive(Clone, Copy)]
pub struct HardwareInfo {
    pub cpu_vendor: CpuVendor,
    pub cpu_features: CpuFeatures,
    pub logical_cpus: u32,
    pub pci: pci::Summary,
}

#[derive(Clone, Copy)]
pub enum CpuVendor {
    Intel,
    Amd,
    Unknown,
}

#[derive(Clone, Copy, Default)]
pub struct CpuFeatures {
    pub has_tsc: bool,
    pub has_rdrand: bool,
    pub has_aes_ni: bool,
    pub has_avx: bool,
    pub has_pae: bool,
    pub has_sse4_2: bool,
}

pub fn init() -> HardwareInfo {
    HardwareInfo {
        cpu_vendor: cpu::detect_vendor(),
        cpu_features: cpu::detect_features(),
        logical_cpus: cpu::count_logical_cpus(),
        pci: pci::discover(),
    }
}

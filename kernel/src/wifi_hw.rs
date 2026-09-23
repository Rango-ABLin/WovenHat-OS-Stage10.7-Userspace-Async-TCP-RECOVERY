//! WovenWiFi - physical PCI Wi-Fi backend foundation.
//!
//! Establishes the hardware-facing ownership boundary without claiming support
//! for a specific chipset yet. A valid candidate must be a PCI network
//! controller in the wireless/other subclass, expose an MMIO BAR, and be
//! prepared for memory-space decoding + DMA bus mastering.
//!
//! Register layouts, firmware protocols, DMA ring formats, interrupts, channel
//! control, and RF behavior remain chipset-specific work for later stages.

use core::sync::atomic::{fence, Ordering};

use x86_64::{
    structures::paging::{PhysFrame, Size4KiB},
    PhysAddr,
};

use crate::{hal::pci, memory, paging};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HwBindError {
    NotWireless,
    NoMmioBar,
    ConfigWriteFailed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct WifiPciFunction {
    pub address: pci::Address,
    pub vendor_id: u16,
    pub device_id: u16,
    pub revision: u8,
    pub mmio_bar: u8,
    pub mmio_base: u64,
    pub msix: bool,
    pub msi: bool,
    pub pcie: bool,
}

pub const fn is_wireless_candidate(device: &pci::Device) -> bool {
    device.class == 0x02 && device.subclass == 0x80
}

fn first_mmio_bar(device: &pci::Device) -> Option<(u8, u64)> {
    let mut index = 0usize;
    while index < device.bars.len() {
        let bar = device.bars[index];
        if bar.valid
            && matches!(bar.kind, pci::BarKind::Memory32 | pci::BarKind::Memory64)
            && bar.address != 0
        {
            return Some((index as u8, bar.address));
        }
        index += 1;
    }
    None
}

pub fn descriptor_from_device(device: pci::Device) -> Result<WifiPciFunction, HwBindError> {
    if !is_wireless_candidate(&device) {
        return Err(HwBindError::NotWireless);
    }
    let Some((bar, base)) = first_mmio_bar(&device) else {
        return Err(HwBindError::NoMmioBar);
    };
    Ok(WifiPciFunction {
        address: pci::Address {
            segment: device.segment,
            bus: device.bus,
            device: device.device,
            function: device.function,
        },
        vendor_id: device.vendor_id,
        device_id: device.device_id,
        revision: device.revision,
        mmio_bar: bar,
        mmio_base: base,
        msix: device.capabilities.msix,
        msi: device.capabilities.msi,
        pcie: device.capabilities.pcie,
    })
}

/// Discovery-only PCI Wi-Fi candidate enumeration.
///
/// This deliberately does not enable PCI memory decoding or bus mastering.
/// Generic class/subclass discovery is not sufficient authority to activate a
/// DMA-capable device. Only the reviewed supported-chipset path below may
/// cross that boundary.
pub fn discover_first() -> Option<WifiPciFunction> {
    for index in 0..64 {
        let Some(device) = pci::device(index) else {
            continue;
        };
        if is_wireless_candidate(&device) {
            if let Ok(candidate) = descriptor_from_device(device) {
                return Some(candidate);
            }
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiDriverFamily {
    IntelIwlwifi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SupportedWifiDevice {
    pub vendor_id: u16,
    pub device_id: u16,
    pub family: WifiDriverFamily,
    pub name: &'static str,
}

const SUPPORTED_WIFI_DEVICES: &[SupportedWifiDevice] = &[SupportedWifiDevice {
    vendor_id: 0x8086,
    device_id: 0x2723,
    family: WifiDriverFamily::IntelIwlwifi,
    name: "Intel Wi-Fi 6 AX200",
}];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupportedBindError {
    NotWireless,
    UnsupportedDevice,
    NoMmioBar,
    ConfigWriteFailed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SupportedWifiFunction {
    pub pci: WifiPciFunction,
    pub family: WifiDriverFamily,
    pub name: &'static str,
}

pub fn supported_device(vendor_id: u16, device_id: u16) -> Option<&'static SupportedWifiDevice> {
    SUPPORTED_WIFI_DEVICES
        .iter()
        .find(|entry| entry.vendor_id == vendor_id && entry.device_id == device_id)
}

pub fn classify_supported(
    device: pci::Device,
) -> Result<SupportedWifiFunction, SupportedBindError> {
    if !is_wireless_candidate(&device) {
        return Err(SupportedBindError::NotWireless);
    }
    let Some(supported) = supported_device(device.vendor_id, device.device_id) else {
        return Err(SupportedBindError::UnsupportedDevice);
    };
    let pci = descriptor_from_device(device).map_err(|error| match error {
        HwBindError::NotWireless => SupportedBindError::NotWireless,
        HwBindError::NoMmioBar => SupportedBindError::NoMmioBar,
        HwBindError::ConfigWriteFailed => SupportedBindError::ConfigWriteFailed,
    })?;
    Ok(SupportedWifiFunction {
        pci,
        family: supported.family,
        name: supported.name,
    })
}

pub fn bind_supported(device: pci::Device) -> Result<SupportedWifiFunction, SupportedBindError> {
    let function = classify_supported(device)?;
    if !pci::enable_memory_bus_master(function.pci.address) {
        return Err(SupportedBindError::ConfigWriteFailed);
    }
    Ok(function)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ax200MsiBindError {
    UnsupportedDevice,
    MissingMsi,
    MissingApicDestination,
    Msi(pci::MsiError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ax200MsiBinding {
    pub vendor_id: u16,
    pub device_id: u16,
    pub destination_apic_id: u32,
    pub vector: u8,
    pub capability_offset: u16,
    pub is_64_bit: bool,
}

/// Validate the exact physical-device authority boundary for AX200 MSI.
///
/// This function performs no PCI writes. It confirms that the already-reviewed
/// WovenHat supported-device classification and the generic PCI MSI layer agree
/// on the same Intel AX200 function before hardware activation is permitted.
pub fn ax200_msi_binding_contract(
    device: pci::Device,
    destination_apic_id: u32,
) -> Result<Ax200MsiBinding, Ax200MsiBindError> {
    if device.vendor_id != 0x8086 || device.device_id != 0x2723 || !is_wireless_candidate(&device) {
        return Err(Ax200MsiBindError::UnsupportedDevice);
    }
    if !device.capabilities.msi || device.capabilities.msi_offset == 0 {
        return Err(Ax200MsiBindError::MissingMsi);
    }

    let message =
        pci::MsiMessage::fixed(destination_apic_id, crate::interrupts::WIFI_DEVICE_VECTOR)
            .map_err(Ax200MsiBindError::Msi)?;

    Ok(Ax200MsiBinding {
        vendor_id: device.vendor_id,
        device_id: device.device_id,
        destination_apic_id,
        vector: u8::try_from(message.data).unwrap_or(crate::interrupts::WIFI_DEVICE_VECTOR),
        capability_offset: device.capabilities.msi_offset,
        is_64_bit: false,
    })
}

/// Program MSI only for the exact supported AX200 function.
///
/// This is the first physical binding step: PCI writes occur only after the
/// generic supported-device classification and MSI capability validation pass.
pub fn bind_ax200_msi(device: pci::Device) -> Result<Ax200MsiBinding, Ax200MsiBindError> {
    let _ = classify_supported(device).map_err(|_| Ax200MsiBindError::UnsupportedDevice)?;
    let destination =
        crate::smp::device_irq_destination().ok_or(Ax200MsiBindError::MissingApicDestination)?;
    let mut binding = ax200_msi_binding_contract(device, destination)?;
    let programmed = pci::program_msi(device, destination, crate::interrupts::WIFI_DEVICE_VECTOR)
        .map_err(Ax200MsiBindError::Msi)?;
    binding.capability_offset = programmed.offset;
    binding.is_64_bit = programmed.is_64_bit;
    Ok(binding)
}

/// Discover and bind a real AX200 if one is present in WovenHat's PCI inventory.
///
/// QEMU acceptance does not require such hardware to exist. `Ok(None)` means
/// no genuine AX200 was discovered and no MSI configuration was written.
pub fn discover_and_bind_ax200_msi() -> Result<Option<Ax200MsiBinding>, Ax200MsiBindError> {
    for index in 0..64 {
        let Some(device) = pci::device(index) else {
            continue;
        };
        if device.vendor_id == 0x8086
            && device.device_id == 0x2723
            && is_wireless_candidate(&device)
        {
            return bind_ax200_msi(device).map(Some);
        }
    }
    Ok(None)
}

pub fn stage13_10ab_binding_self_test() -> bool {
    let device = pci::Device {
        vendor_id: 0x8086,
        device_id: 0x2723,
        class: 0x02,
        subclass: 0x80,
        capabilities: pci::Capabilities {
            msi: true,
            msi_offset: 0x50,
            ..pci::Capabilities::default()
        },
        ..pci::Device::default()
    };

    let Ok(binding) = ax200_msi_binding_contract(device, 0x2a) else {
        return false;
    };

    if binding.vendor_id != 0x8086
        || binding.device_id != 0x2723
        || binding.destination_apic_id != 0x2a
        || binding.vector != crate::interrupts::WIFI_DEVICE_VECTOR
        || binding.capability_offset != 0x50
    {
        return false;
    }

    let mut wrong = device;
    wrong.device_id = 0x9999;
    if ax200_msi_binding_contract(wrong, 0x2a) != Err(Ax200MsiBindError::UnsupportedDevice) {
        return false;
    }

    let mut no_msi = device;
    no_msi.capabilities.msi = false;
    no_msi.capabilities.msi_offset = 0;
    ax200_msi_binding_contract(no_msi, 0x2a) == Err(Ax200MsiBindError::MissingMsi)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ax200DeferredServiceError {
    Csr(IntelRxInterruptError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ax200DeferredServiceEvent {
    pub work_was_pending: bool,
    pub rx_pending: bool,
    pub acknowledged: u32,
    pub raw_status: u32,
}

/// Deferred half of the AX200 interrupt path.
///
/// The vector 0xd0 hard IRQ publishes work, sends LAPIC EOI, and wakes the worker.
/// This routine consumes that publication outside hard-IRQ context and routes
/// the device event through the Stage 13.10Y Intel CSR interrupt controller.
/// The caller must be the sole consumer for the bound device and keep its CSR
/// mapping alive. This boundary does not register a worker or provide detach
/// synchronization. A service error consumes the claim and is returned to the
/// caller for recovery; it is not silently retried or converted into RX work.
pub fn service_deferred_ax200_interrupt(
    controller: &mut IntelRxInterruptController,
) -> Result<Ax200DeferredServiceEvent, Ax200DeferredServiceError> {
    if !crate::interrupts::take_wifi_device_work() {
        return Ok(Ax200DeferredServiceEvent {
            work_was_pending: false,
            rx_pending: false,
            acknowledged: 0,
            raw_status: 0,
        });
    }

    let event = controller
        .service()
        .map_err(Ax200DeferredServiceError::Csr)?;

    Ok(Ax200DeferredServiceEvent {
        work_was_pending: true,
        rx_pending: event.rx_pending,
        acknowledged: event.acknowledged,
        raw_status: event.raw_status,
    })
}

pub fn stage13_10ac_deferred_self_test() -> bool {
    let Some(function) = stage13_10i_supported_function() else {
        return false;
    };
    let Ok(mut page) = DmaPage::allocate_zeroed() else {
        return false;
    };
    let Ok(region) = MmioRegion::new(page.physical_address(), DMA_PAGE_SIZE) else {
        return false;
    };

    // SAFETY: synthetic CSR storage aliases one exclusively-owned DmaPage.
    let mmio = unsafe { VolatileMmio32::from_mapped(region, page.virtual_address()) };
    let Ok(csr) = IntelCsrBank::from_supported(function, mmio) else {
        return false;
    };
    let mut controller = IntelRxInterruptController::new(csr);

    if controller.enable_rx().is_err() {
        return false;
    }

    // With no work pending, even a fatal cause must remain untouched. Seeding
    // the mask also detects an accidental service call on the idle path.
    if page
        .write_u32(IntelCsr::Int.offset(), INTEL_CSR_INT_BIT_HW_ERR)
        .is_err()
        || page.write_u32(IntelCsr::IntMask.offset(), 0).is_err()
    {
        return false;
    }
    let Ok(idle) = service_deferred_ax200_interrupt(&mut controller) else {
        return false;
    };
    if idle.work_was_pending
        || idle.rx_pending
        || idle.acknowledged != 0
        || idle.raw_status != 0
        || page.read_u32(IntelCsr::Int.offset()) != Ok(INTEL_CSR_INT_BIT_HW_ERR)
        || page.read_u32(IntelCsr::IntMask.offset()) != Ok(0)
    {
        return false;
    }

    // Publish the same atomic work state as the real hard IRQ and seed FH_RX.
    if page
        .write_u32(IntelCsr::Int.offset(), INTEL_CSR_INT_BIT_FH_RX)
        .is_err()
    {
        return false;
    }
    crate::interrupts::publish_wifi_device_work_for_test();
    // Multiple publications coalesce into one CSR snapshot, not an IRQ count.
    crate::interrupts::publish_wifi_device_work_for_test();

    let Ok(serviced) = service_deferred_ax200_interrupt(&mut controller) else {
        return false;
    };
    if !serviced.work_was_pending
        || !serviced.rx_pending
        || serviced.acknowledged != INTEL_CSR_INT_BIT_FH_RX
        || serviced.raw_status != INTEL_CSR_INT_BIT_FH_RX
        || page.read_u32(IntelCsr::Int.offset()) != Ok(INTEL_CSR_INT_BIT_FH_RX)
        || page.read_u32(IntelCsr::IntMask.offset()) != Ok(INTEL_CSR_RX_INTERRUPT_MASK)
        || crate::interrupts::take_wifi_device_work()
    {
        return false;
    }

    // Work without an enabled RX cause is consumed but must not fabricate RX.
    if page.write_u32(IntelCsr::Int.offset(), 0).is_err() {
        return false;
    }
    crate::interrupts::publish_wifi_device_work_for_test();
    let Ok(empty) = service_deferred_ax200_interrupt(&mut controller) else {
        return false;
    };

    if !empty.work_was_pending || empty.rx_pending || empty.acknowledged != 0 || empty.raw_status != 0 {
        return false;
    }

    // Disabling must also revoke the saved mask: a previously queued work
    // publication must not restore RX delivery or report disabled RX work.
    if page
        .write_u32(IntelCsr::Int.offset(), INTEL_CSR_INT_BIT_FH_RX)
        .is_err()
    {
        return false;
    }
    crate::interrupts::publish_wifi_device_work_for_test();
    if controller.disable().is_err() || controller.configured_mask() != 0 {
        return false;
    }
    let Ok(disabled) = service_deferred_ax200_interrupt(&mut controller) else {
        return false;
    };
    if !disabled.work_was_pending
        || disabled.rx_pending
        || disabled.acknowledged != 0
        || disabled.raw_status != INTEL_CSR_INT_BIT_FH_RX
        || page.read_u32(IntelCsr::IntMask.offset()) != Ok(0)
        || page.read_u32(IntelCsr::Int.offset()) != Ok(INTEL_CSR_INT_BIT_FH_RX)
        || crate::interrupts::take_wifi_device_work()
        || controller.enable_rx().is_err()
    {
        return false;
    }

    // Both fatal results must cross the deferred boundary unchanged, leave
    // CSR delivery masked, and consume the claimed publication. A separately
    // published event must still be observed by a subsequent claim.
    for (cause, error) in [
        (INTEL_CSR_INT_BIT_HW_ERR, IntelRxInterruptError::FatalHardware),
        (INTEL_CSR_INT_BIT_SW_ERR, IntelRxInterruptError::FatalFirmware),
    ] {
        if page.write_u32(IntelCsr::Int.offset(), cause).is_err() {
            return false;
        }
        crate::interrupts::publish_wifi_device_work_for_test();
        if service_deferred_ax200_interrupt(&mut controller)
            != Err(Ax200DeferredServiceError::Csr(error))
            || page.read_u32(IntelCsr::IntMask.offset()) != Ok(0)
            || crate::interrupts::take_wifi_device_work()
        {
            return false;
        }
    }
    true
}
pub fn discover_first_supported() -> Option<SupportedWifiFunction> {
    for index in 0..64 {
        let Some(device) = pci::device(index) else {
            continue;
        };
        if supported_device(device.vendor_id, device.device_id).is_some()
            && is_wireless_candidate(&device)
        {
            if let Ok(bound) = classify_supported(device) {
                return Some(bound);
            }
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum IntelCsr {
    HwIfConfig = 0x0000,
    IntCoalescing = 0x004c,
    Int = 0x0008,
    IntMask = 0x000c,
    Reset = 0x0020,
    GpControl = 0x0024,
    GpDriver = 0x0050,
}

impl IntelCsr {
    pub const fn offset(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelCsrError {
    UnsupportedFamily,
    Mmio(MmioError),
}

/// Minimal Intel AX200-family CSR accessor.
///
/// Stage 13.10H intentionally provides only typed 32-bit CSR access. It does
/// not yet perform reset sequencing, firmware loading, interrupt setup, or
/// queue programming. Those steps will build on this narrow audited boundary.
pub struct IntelCsrBank {
    registers: VolatileMmio32,
}

impl IntelCsrBank {
    /// Construct over an already validated/mapped Intel register page.
    ///
    /// The caller must have passed the supported-chipset authority boundary.
    pub fn from_supported(
        function: SupportedWifiFunction,
        registers: VolatileMmio32,
    ) -> Result<Self, IntelCsrError> {
        if function.family != WifiDriverFamily::IntelIwlwifi {
            return Err(IntelCsrError::UnsupportedFamily);
        }
        Ok(Self { registers })
    }

    pub fn read(&self, csr: IntelCsr) -> Result<u32, IntelCsrError> {
        self.registers
            .read(csr.offset())
            .map_err(IntelCsrError::Mmio)
    }

    pub fn write(&mut self, csr: IntelCsr, value: u32) -> Result<(), IntelCsrError> {
        self.registers
            .write(csr.offset(), value)
            .map_err(IntelCsrError::Mmio)
    }
}

/// Intel legacy/INTx CSR interrupt causes used by the AX200-family transport.
///
/// These values are hardware ABI constants. Stage 13.10Y deliberately models
/// the CSR interrupt-service boundary only; vector/MSI-X routing remains a
/// later integration step.
pub const INTEL_CSR_INT_BIT_FH_RX: u32 = 1u32 << 31;
pub const INTEL_CSR_INT_BIT_HW_ERR: u32 = 1u32 << 29;
pub const INTEL_CSR_INT_BIT_SW_ERR: u32 = 1u32 << 25;
pub const INTEL_CSR_INT_BIT_RF_KILL: u32 = 1u32 << 7;
pub const INTEL_CSR_INT_BIT_SW_RX: u32 = 1u32 << 3;
pub const INTEL_CSR_INT_BIT_RX_PERIODIC: u32 = 1u32 << 0;

pub const INTEL_CSR_RX_INTERRUPT_MASK: u32 =
    INTEL_CSR_INT_BIT_FH_RX | INTEL_CSR_INT_BIT_SW_RX | INTEL_CSR_INT_BIT_RX_PERIODIC;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelRxInterruptError {
    Csr(IntelCsrError),
    FatalHardware,
    FatalFirmware,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntelRxInterruptEvent {
    pub raw_status: u32,
    pub enabled_status: u32,
    pub acknowledged: u32,
    pub rx_pending: bool,
}

/// WovenHat-owned Intel CSR RX interrupt service boundary.
///
/// Hardware interrupt delivery (INTx/MSI/MSI-X and APIC routing) is intentionally
/// outside this type. Once WovenHat's generic interrupt layer invokes it, this
/// controller masks CSR delivery, snapshots causes, acknowledges handled causes,
/// classifies RX work, and restores the configured mask.
pub struct IntelRxInterruptController {
    csr: IntelCsrBank,
    configured_mask: u32,
}

impl IntelRxInterruptController {
    pub fn new(csr: IntelCsrBank) -> Self {
        Self {
            csr,
            configured_mask: 0,
        }
    }

    pub const fn configured_mask(&self) -> u32 {
        self.configured_mask
    }

    pub fn enable_rx(&mut self) -> Result<(), IntelRxInterruptError> {
        self.configured_mask = INTEL_CSR_RX_INTERRUPT_MASK;
        self.csr
            .write(IntelCsr::IntMask, self.configured_mask)
            .map_err(IntelRxInterruptError::Csr)
    }

    pub fn disable(&mut self) -> Result<(), IntelRxInterruptError> {
        self.csr
            .write(IntelCsr::IntMask, 0)
            .map_err(IntelRxInterruptError::Csr)?;
        self.configured_mask = 0;
        Ok(())
    }

    pub fn service(&mut self) -> Result<IntelRxInterruptEvent, IntelRxInterruptError> {
        // Prevent new CSR delivery while the current cause snapshot is handled.
        self.csr
            .write(IntelCsr::IntMask, 0)
            .map_err(IntelRxInterruptError::Csr)?;

        let raw_status = self
            .csr
            .read(IntelCsr::Int)
            .map_err(IntelRxInterruptError::Csr)?;
        let enabled_status = raw_status & self.configured_mask;

        if raw_status & INTEL_CSR_INT_BIT_HW_ERR != 0 {
            return Err(IntelRxInterruptError::FatalHardware);
        }
        if raw_status & INTEL_CSR_INT_BIT_SW_ERR != 0 {
            return Err(IntelRxInterruptError::FatalFirmware);
        }

        let acknowledged = enabled_status & INTEL_CSR_RX_INTERRUPT_MASK;

        // Real Intel CSR_INT uses write-one-to-clear acknowledgement. Synthetic
        // MMIO cannot emulate W1C in memory, so the test verifies the value
        // written here as the ABI contract rather than pretending hardware W1C.
        if acknowledged != 0 {
            self.csr
                .write(IntelCsr::Int, acknowledged)
                .map_err(IntelRxInterruptError::Csr)?;
        }

        self.csr
            .write(IntelCsr::IntMask, self.configured_mask)
            .map_err(IntelRxInterruptError::Csr)?;

        Ok(IntelRxInterruptEvent {
            raw_status,
            enabled_status,
            acknowledged,
            rx_pending: acknowledged != 0,
        })
    }
}

pub fn stage13_10y_self_test() -> bool {
    let Some(function) = stage13_10i_supported_function() else {
        return false;
    };
    let Ok(mut page) = DmaPage::allocate_zeroed() else {
        return false;
    };
    let Ok(region) = MmioRegion::new(page.physical_address(), DMA_PAGE_SIZE) else {
        return false;
    };

    // SAFETY: synthetic CSR storage aliases one exclusively-owned DmaPage.
    let mmio = unsafe { VolatileMmio32::from_mapped(region, page.virtual_address()) };
    let Ok(csr) = IntelCsrBank::from_supported(function, mmio) else {
        return false;
    };
    let mut irq = IntelRxInterruptController::new(csr);

    if irq.enable_rx().is_err()
        || irq.configured_mask() != INTEL_CSR_RX_INTERRUPT_MASK
        || page.read_u32(IntelCsr::IntMask.offset()) != Ok(INTEL_CSR_RX_INTERRUPT_MASK)
    {
        return false;
    }

    // FH_RX is the RX DMA/command-response cause. An unrelated RF-kill cause is
    // deliberately present and must not be acknowledged by the RX boundary.
    if page
        .write_u32(
            IntelCsr::Int.offset(),
            INTEL_CSR_INT_BIT_FH_RX | INTEL_CSR_INT_BIT_RF_KILL,
        )
        .is_err()
    {
        return false;
    }
    let Ok(event) = irq.service() else {
        return false;
    };
    if event.raw_status != (INTEL_CSR_INT_BIT_FH_RX | INTEL_CSR_INT_BIT_RF_KILL)
        || event.enabled_status != INTEL_CSR_INT_BIT_FH_RX
        || event.acknowledged != INTEL_CSR_INT_BIT_FH_RX
        || !event.rx_pending
        || page.read_u32(IntelCsr::Int.offset()) != Ok(INTEL_CSR_INT_BIT_FH_RX)
        || page.read_u32(IntelCsr::IntMask.offset()) != Ok(INTEL_CSR_RX_INTERRUPT_MASK)
    {
        return false;
    }

    // SW_RX must also classify as receive work.
    if page
        .write_u32(IntelCsr::Int.offset(), INTEL_CSR_INT_BIT_SW_RX)
        .is_err()
    {
        return false;
    }
    let Ok(sw_event) = irq.service() else {
        return false;
    };
    if !sw_event.rx_pending || sw_event.acknowledged != INTEL_CSR_INT_BIT_SW_RX {
        return false;
    }

    // A masked unrelated cause is observable but produces no RX work.
    if page
        .write_u32(IntelCsr::Int.offset(), INTEL_CSR_INT_BIT_RF_KILL)
        .is_err()
    {
        return false;
    }
    let Ok(masked) = irq.service() else {
        return false;
    };
    if masked.rx_pending || masked.acknowledged != 0 {
        return false;
    }

    // Fatal causes fail closed.
    if page
        .write_u32(IntelCsr::Int.offset(), INTEL_CSR_INT_BIT_HW_ERR)
        .is_err()
        || irq.service() != Err(IntelRxInterruptError::FatalHardware)
    {
        return false;
    }

    true
}
pub const INTEL_CSR_RESET_SW_RESET: u32 = 0x0000_0080;
pub const INTEL_CSR_GP_CNTRL_MAC_CLOCK_READY: u32 = 0x0000_0001;
pub const INTEL_RESET_POLL_LIMIT: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelResetState {
    Uninitialized,
    ResetRequested,
    WaitingForReady,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelResetError {
    InvalidState,
    Timeout,
    Csr(IntelCsrError),
}

/// Explicit Intel reset/readiness lifecycle.
///
/// Stage 13.10I models the control flow and bounded readiness wait. The live
/// hardware path is not invoked by the acceptance test; synthetic MMIO is used
/// to validate state transitions, register effects, and timeout behavior.
pub struct IntelResetController {
    csr: IntelCsrBank,
    state: IntelResetState,
}

impl IntelResetController {
    pub const fn state(&self) -> IntelResetState {
        self.state
    }

    pub fn new(csr: IntelCsrBank) -> Self {
        Self {
            csr,
            state: IntelResetState::Uninitialized,
        }
    }

    pub fn request_reset(&mut self) -> Result<(), IntelResetError> {
        if self.state != IntelResetState::Uninitialized {
            return Err(IntelResetError::InvalidState);
        }

        let current = self
            .csr
            .read(IntelCsr::Reset)
            .map_err(IntelResetError::Csr)?;
        self.csr
            .write(IntelCsr::Reset, current | INTEL_CSR_RESET_SW_RESET)
            .map_err(IntelResetError::Csr)?;
        self.state = IntelResetState::ResetRequested;
        Ok(())
    }

    pub fn begin_ready_wait(&mut self) -> Result<(), IntelResetError> {
        if self.state != IntelResetState::ResetRequested {
            return Err(IntelResetError::InvalidState);
        }
        self.state = IntelResetState::WaitingForReady;
        Ok(())
    }

    pub fn poll_ready_bounded(&mut self, limit: usize) -> Result<(), IntelResetError> {
        if self.state != IntelResetState::WaitingForReady {
            return Err(IntelResetError::InvalidState);
        }
        if limit == 0 {
            self.state = IntelResetState::Failed;
            return Err(IntelResetError::Timeout);
        }

        for _ in 0..limit {
            let value = self
                .csr
                .read(IntelCsr::GpControl)
                .map_err(IntelResetError::Csr)?;
            if value & INTEL_CSR_GP_CNTRL_MAC_CLOCK_READY != 0 {
                self.state = IntelResetState::Ready;
                return Ok(());
            }
        }

        self.state = IntelResetState::Failed;
        Err(IntelResetError::Timeout)
    }
}

pub(crate) fn stage13_10i_supported_function() -> Option<SupportedWifiFunction> {
    let mut ax200 = pci::Device {
        segment: 0,
        bus: 2,
        device: 3,
        function: 0,
        vendor_id: 0x8086,
        device_id: 0x2723,
        revision: 1,
        class: 0x02,
        subclass: 0x80,
        ..pci::Device::default()
    };
    ax200.bars[0] = pci::Bar {
        valid: true,
        kind: pci::BarKind::Memory64,
        address: 0xfebc_0000,
        prefetchable: false,
    };
    classify_supported(ax200).ok()
}

pub const INTEL_CSR_GP_CNTRL_INIT_DONE: u32 = 0x0000_0004;
pub const INTEL_CSR_GP_CNTRL_MAC_ACCESS_REQ: u32 = 0x0000_0008;
pub const INTEL_CSR_GP_CNTRL_GOING_TO_SLEEP: u32 = 0x0000_0010;
pub const INTEL_MAC_ACCESS_POLL_LIMIT: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelMacAccessState {
    ResetReady,
    InitDone,
    AccessRequested,
    Granted,
    Released,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntelMacAccessError {
    InvalidState,
    Timeout,
    Csr(IntelCsrError),
}

/// Stage 13.10J device-initialization and MAC-access lifecycle.
///
/// This remains a control-plane boundary. Firmware loading, peripheral/PRPH
/// register access, interrupt programming, and TX/RX queue activation are
/// intentionally deferred.
pub struct IntelMacAccessController {
    csr: IntelCsrBank,
    state: IntelMacAccessState,
}

impl IntelMacAccessController {
    pub fn new(csr: IntelCsrBank) -> Self {
        Self {
            csr,
            state: IntelMacAccessState::ResetReady,
        }
    }

    pub const fn state(&self) -> IntelMacAccessState {
        self.state
    }

    pub fn mark_init_done(&mut self) -> Result<(), IntelMacAccessError> {
        if self.state != IntelMacAccessState::ResetReady {
            return Err(IntelMacAccessError::InvalidState);
        }
        let value = self
            .csr
            .read(IntelCsr::GpControl)
            .map_err(IntelMacAccessError::Csr)?;
        self.csr
            .write(IntelCsr::GpControl, value | INTEL_CSR_GP_CNTRL_INIT_DONE)
            .map_err(IntelMacAccessError::Csr)?;
        self.state = IntelMacAccessState::InitDone;
        Ok(())
    }

    pub fn request_access(&mut self) -> Result<(), IntelMacAccessError> {
        if self.state != IntelMacAccessState::InitDone {
            return Err(IntelMacAccessError::InvalidState);
        }
        let value = self
            .csr
            .read(IntelCsr::GpControl)
            .map_err(IntelMacAccessError::Csr)?;
        self.csr
            .write(
                IntelCsr::GpControl,
                value | INTEL_CSR_GP_CNTRL_MAC_ACCESS_REQ,
            )
            .map_err(IntelMacAccessError::Csr)?;
        self.state = IntelMacAccessState::AccessRequested;
        Ok(())
    }

    pub fn poll_access_bounded(&mut self, limit: usize) -> Result<(), IntelMacAccessError> {
        if self.state != IntelMacAccessState::AccessRequested {
            return Err(IntelMacAccessError::InvalidState);
        }
        if limit == 0 {
            self.state = IntelMacAccessState::Failed;
            return Err(IntelMacAccessError::Timeout);
        }

        for _ in 0..limit {
            let value = self
                .csr
                .read(IntelCsr::GpControl)
                .map_err(IntelMacAccessError::Csr)?;
            let ready = value & INTEL_CSR_GP_CNTRL_MAC_CLOCK_READY != 0;
            let sleeping = value & INTEL_CSR_GP_CNTRL_GOING_TO_SLEEP != 0;
            if ready && !sleeping {
                self.state = IntelMacAccessState::Granted;
                return Ok(());
            }
        }

        self.state = IntelMacAccessState::Failed;
        Err(IntelMacAccessError::Timeout)
    }

    pub fn release_access(&mut self) -> Result<(), IntelMacAccessError> {
        if self.state != IntelMacAccessState::Granted {
            return Err(IntelMacAccessError::InvalidState);
        }
        let value = self
            .csr
            .read(IntelCsr::GpControl)
            .map_err(IntelMacAccessError::Csr)?;
        self.csr
            .write(
                IntelCsr::GpControl,
                value & !INTEL_CSR_GP_CNTRL_MAC_ACCESS_REQ,
            )
            .map_err(IntelMacAccessError::Csr)?;
        self.state = IntelMacAccessState::Released;
        Ok(())
    }
}

pub fn stage13_10j_self_test() -> bool {
    let Some(function) = stage13_10i_supported_function() else {
        return false;
    };
    let Ok(mut page) = DmaPage::allocate_zeroed() else {
        return false;
    };
    let Ok(region) = MmioRegion::new(page.physical_address(), DMA_PAGE_SIZE) else {
        return false;
    };

    // Synthetic hardware advertises MAC clock readiness and not-going-to-sleep.
    if page
        .write_u32(
            IntelCsr::GpControl.offset(),
            INTEL_CSR_GP_CNTRL_MAC_CLOCK_READY,
        )
        .is_err()
    {
        return false;
    }

    // SAFETY: synthetic CSR storage aliases one exclusively-owned DmaPage.
    let mmio = unsafe { VolatileMmio32::from_mapped(region, page.virtual_address()) };
    let Ok(csr) = IntelCsrBank::from_supported(function, mmio) else {
        return false;
    };
    let mut mac = IntelMacAccessController::new(csr);

    if mac.request_access() != Err(IntelMacAccessError::InvalidState)
        || mac.mark_init_done().is_err()
        || mac.state() != IntelMacAccessState::InitDone
        || mac.request_access().is_err()
        || mac.state() != IntelMacAccessState::AccessRequested
        || mac
            .poll_access_bounded(INTEL_MAC_ACCESS_POLL_LIMIT)
            .is_err()
        || mac.state() != IntelMacAccessState::Granted
    {
        return false;
    }

    let Ok(granted_value) = page.read_u32(IntelCsr::GpControl.offset()) else {
        return false;
    };
    if granted_value & INTEL_CSR_GP_CNTRL_INIT_DONE == 0
        || granted_value & INTEL_CSR_GP_CNTRL_MAC_ACCESS_REQ == 0
    {
        return false;
    }

    if mac.release_access().is_err() || mac.state() != IntelMacAccessState::Released {
        return false;
    }
    let Ok(released_value) = page.read_u32(IntelCsr::GpControl.offset()) else {
        return false;
    };
    if released_value & INTEL_CSR_GP_CNTRL_MAC_ACCESS_REQ != 0 {
        return false;
    }

    // Timeout/fail-closed case: access request exists, but hardware is marked
    // going-to-sleep and never presents an acceptable grant condition.
    let Some(function) = stage13_10i_supported_function() else {
        return false;
    };
    let Ok(mut timeout_page) = DmaPage::allocate_zeroed() else {
        return false;
    };
    let Ok(timeout_region) = MmioRegion::new(timeout_page.physical_address(), DMA_PAGE_SIZE) else {
        return false;
    };
    if timeout_page
        .write_u32(
            IntelCsr::GpControl.offset(),
            INTEL_CSR_GP_CNTRL_GOING_TO_SLEEP,
        )
        .is_err()
    {
        return false;
    }
    // SAFETY: synthetic CSR storage aliases one exclusively-owned DmaPage.
    let timeout_mmio =
        unsafe { VolatileMmio32::from_mapped(timeout_region, timeout_page.virtual_address()) };
    let Ok(timeout_csr) = IntelCsrBank::from_supported(function, timeout_mmio) else {
        return false;
    };
    let mut timeout_mac = IntelMacAccessController::new(timeout_csr);

    timeout_mac.mark_init_done().is_ok()
        && timeout_mac.request_access().is_ok()
        && timeout_mac.poll_access_bounded(4) == Err(IntelMacAccessError::Timeout)
        && timeout_mac.state() == IntelMacAccessState::Failed
}
pub fn stage13_10i_self_test() -> bool {
    let Some(function) = stage13_10i_supported_function() else {
        return false;
    };

    let Ok(mut page) = DmaPage::allocate_zeroed() else {
        return false;
    };
    let Ok(region) = MmioRegion::new(page.physical_address(), DMA_PAGE_SIZE) else {
        return false;
    };

    // Seed synthetic readiness after reset has been requested. The reset
    // controller itself still observes the flag only through IntelCsrBank.
    if page
        .write_u32(
            IntelCsr::GpControl.offset(),
            INTEL_CSR_GP_CNTRL_MAC_CLOCK_READY,
        )
        .is_err()
    {
        return false;
    }

    // SAFETY: this synthetic register window aliases one live, exclusively
    // owned DmaPage for the duration of the controller.
    let mmio = unsafe { VolatileMmio32::from_mapped(region, page.virtual_address()) };
    let Ok(csr) = IntelCsrBank::from_supported(function, mmio) else {
        return false;
    };

    let mut reset = IntelResetController::new(csr);
    if reset.state() != IntelResetState::Uninitialized
        || reset.begin_ready_wait() != Err(IntelResetError::InvalidState)
        || reset.request_reset().is_err()
        || reset.state() != IntelResetState::ResetRequested
        || reset.request_reset() != Err(IntelResetError::InvalidState)
        || reset.begin_ready_wait().is_err()
        || reset.state() != IntelResetState::WaitingForReady
        || reset.poll_ready_bounded(INTEL_RESET_POLL_LIMIT).is_err()
        || reset.state() != IntelResetState::Ready
    {
        return false;
    }

    let Ok(reset_value) = page.read_u32(IntelCsr::Reset.offset()) else {
        return false;
    };
    if reset_value & INTEL_CSR_RESET_SW_RESET == 0 {
        return false;
    }

    // Independently prove a bounded wait fails closed instead of spinning
    // forever when hardware readiness never appears.
    let Some(function) = stage13_10i_supported_function() else {
        return false;
    };
    let Ok(timeout_page) = DmaPage::allocate_zeroed() else {
        return false;
    };
    let Ok(timeout_region) = MmioRegion::new(timeout_page.physical_address(), DMA_PAGE_SIZE) else {
        return false;
    };
    // SAFETY: same synthetic, exclusively-owned mapping contract as above.
    let timeout_mmio =
        unsafe { VolatileMmio32::from_mapped(timeout_region, timeout_page.virtual_address()) };
    let Ok(timeout_csr) = IntelCsrBank::from_supported(function, timeout_mmio) else {
        return false;
    };
    let mut timeout_reset = IntelResetController::new(timeout_csr);

    timeout_reset.request_reset().is_ok()
        && timeout_reset.begin_ready_wait().is_ok()
        && timeout_reset.poll_ready_bounded(4) == Err(IntelResetError::Timeout)
        && timeout_reset.state() == IntelResetState::Failed
}
pub fn stage13_10h_self_test() -> bool {
    let mut ax200 = pci::Device {
        segment: 0,
        bus: 2,
        device: 3,
        function: 0,
        vendor_id: 0x8086,
        device_id: 0x2723,
        revision: 1,
        class: 0x02,
        subclass: 0x80,
        ..pci::Device::default()
    };
    ax200.bars[0] = pci::Bar {
        valid: true,
        kind: pci::BarKind::Memory64,
        address: 0xfebc_0000,
        prefetchable: false,
    };

    let Ok(function) = classify_supported(ax200) else {
        return false;
    };

    let Ok(page) = DmaPage::allocate_zeroed() else {
        return false;
    };
    let Ok(region) = MmioRegion::new(page.physical_address(), DMA_PAGE_SIZE) else {
        return false;
    };

    // SAFETY: the synthetic CSR window aliases one live, exclusively-owned
    // DmaPage for the duration of this self-test.
    let mmio = unsafe { VolatileMmio32::from_mapped(region, page.virtual_address()) };
    let Ok(mut csr) = IntelCsrBank::from_supported(function, mmio) else {
        return false;
    };

    if IntelCsr::HwIfConfig.offset() != 0x0000
        || IntelCsr::Int.offset() != 0x0008
        || IntelCsr::IntMask.offset() != 0x000c
        || IntelCsr::Reset.offset() != 0x0020
        || IntelCsr::GpControl.offset() != 0x0024
        || IntelCsr::IntCoalescing.offset() != 0x004c
        || IntelCsr::GpDriver.offset() != 0x0050
    {
        return false;
    }

    if csr.write(IntelCsr::Reset, 0xa5a5_5a5a).is_err()
        || csr.read(IntelCsr::Reset) != Ok(0xa5a5_5a5a)
        || csr.write(IntelCsr::IntMask, 0x1122_3344).is_err()
        || csr.read(IntelCsr::IntMask) != Ok(0x1122_3344)
    {
        return false;
    }

    // Stage H must remain an accessor-layer test only. The synthetic page
    // proves typed register isolation without touching real PCI hardware.
    true
}
pub fn stage13_10g_self_test() -> bool {
    let mut supported = pci::Device {
        segment: 0,
        bus: 2,
        device: 3,
        function: 0,
        vendor_id: 0x8086,
        device_id: 0x2723,
        revision: 1,
        class: 0x02,
        subclass: 0x80,
        ..pci::Device::default()
    };
    supported.bars[0] = pci::Bar {
        valid: true,
        kind: pci::BarKind::Memory64,
        address: 0xfebc_0000,
        prefetchable: false,
    };

    // The supported path must classify the reviewed identity.
    let Ok(classified) = classify_supported(supported) else {
        return false;
    };
    if classified.family != WifiDriverFamily::IntelIwlwifi
        || classified.pci.vendor_id != 0x8086
        || classified.pci.device_id != 0x2723
    {
        return false;
    }

    // A generic wireless-class function can be described for diagnostics, but
    // it must never be accepted by the supported activation boundary.
    let mut unknown = supported;
    unknown.vendor_id = 0x1234;
    unknown.device_id = 0x5678;
    let Ok(candidate) = descriptor_from_device(unknown) else {
        return false;
    };
    if candidate.vendor_id != 0x1234
        || candidate.device_id != 0x5678
        || classify_supported(unknown) != Err(SupportedBindError::UnsupportedDevice)
    {
        return false;
    }

    // Wrong PCI class is rejected even if the vendor/device pair is known.
    let mut wrong_class = supported;
    wrong_class.subclass = 0x00;
    classify_supported(wrong_class) == Err(SupportedBindError::NotWireless)
}
pub fn stage13_10e_self_test() -> bool {
    let mut ax200 = pci::Device {
        segment: 0,
        bus: 2,
        device: 3,
        function: 0,
        vendor_id: 0x8086,
        device_id: 0x2723,
        revision: 1,
        class: 0x02,
        subclass: 0x80,
        ..pci::Device::default()
    };
    ax200.bars[0] = pci::Bar {
        valid: true,
        kind: pci::BarKind::Memory64,
        address: 0xfebc_0000,
        prefetchable: false,
    };
    ax200.capabilities.msi = true;
    ax200.capabilities.pcie = true;

    let Ok(bound) = classify_supported(ax200) else {
        return false;
    };
    if bound.family != WifiDriverFamily::IntelIwlwifi
        || bound.name != "Intel Wi-Fi 6 AX200"
        || bound.pci.vendor_id != 0x8086
        || bound.pci.device_id != 0x2723
        || bound.pci.mmio_base != 0xfebc_0000
    {
        return false;
    }

    let mut unknown = ax200;
    unknown.vendor_id = 0x1234;
    unknown.device_id = 0x5678;
    if classify_supported(unknown) != Err(SupportedBindError::UnsupportedDevice) {
        return false;
    }

    let mut wired = ax200;
    wired.subclass = 0x00;
    if classify_supported(wired) != Err(SupportedBindError::NotWireless) {
        return false;
    }

    let mut no_bar = ax200;
    no_bar.bars = [pci::Bar::default(); 6];
    classify_supported(no_bar) == Err(SupportedBindError::NoMmioBar)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmioError {
    Unaligned,
    OutOfRange,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MmioRegion {
    base: u64,
    span: usize,
}

impl MmioRegion {
    pub const fn new(base: u64, span: usize) -> Result<Self, MmioError> {
        if base & 3 != 0 {
            return Err(MmioError::Unaligned);
        }
        if span < 4 || span & 3 != 0 {
            return Err(MmioError::OutOfRange);
        }
        Ok(Self { base, span })
    }

    pub const fn base(&self) -> u64 {
        self.base
    }
    pub const fn span(&self) -> usize {
        self.span
    }

    pub fn register_address(&self, offset: usize) -> Result<u64, MmioError> {
        if offset & 3 != 0 || offset.checked_add(4).is_none_or(|end| end > self.span) {
            return Err(MmioError::OutOfRange);
        }
        self.base
            .checked_add(offset as u64)
            .ok_or(MmioError::OutOfRange)
    }
}

pub const DMA_RING_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaError {
    Full,
    Empty,
    InvalidLength,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct DmaDescriptor {
    pub physical_address: u64,
    pub length: u16,
    pub flags: u16,
}

pub struct DmaRing {
    entries: [DmaDescriptor; DMA_RING_CAPACITY],
    producer: usize,
    consumer: usize,
    count: usize,
}

impl DmaRing {
    pub const fn new() -> Self {
        Self {
            entries: [DmaDescriptor {
                physical_address: 0,
                length: 0,
                flags: 0,
            }; DMA_RING_CAPACITY],
            producer: 0,
            consumer: 0,
            count: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.count
    }
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn push(&mut self, descriptor: DmaDescriptor) -> Result<(), DmaError> {
        if descriptor.length == 0 {
            return Err(DmaError::InvalidLength);
        }
        if self.count == DMA_RING_CAPACITY {
            return Err(DmaError::Full);
        }
        self.entries[self.producer] = descriptor;
        self.producer = (self.producer + 1) % DMA_RING_CAPACITY;
        self.count += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Result<DmaDescriptor, DmaError> {
        if self.count == 0 {
            return Err(DmaError::Empty);
        }
        let descriptor = self.entries[self.consumer];
        self.entries[self.consumer] = DmaDescriptor::default();
        self.consumer = (self.consumer + 1) % DMA_RING_CAPACITY;
        self.count -= 1;
        Ok(descriptor)
    }

    pub fn reset(&mut self) {
        self.entries = [DmaDescriptor::default(); DMA_RING_CAPACITY];
        self.producer = 0;
        self.consumer = 0;
        self.count = 0;
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct InterruptState {
    pub pending: u32,
    pub handled: u64,
}

impl InterruptState {
    pub fn raise(&mut self, causes: u32) {
        self.pending |= causes;
    }

    pub fn take(&mut self) -> u32 {
        let causes = self.pending;
        if causes != 0 {
            self.pending = 0;
            self.handled = self.handled.saturating_add(1);
        }
        causes
    }

    pub fn reset(&mut self) {
        self.pending = 0;
    }
}

pub struct HardwareQueues {
    pub tx: DmaRing,
    pub rx: DmaRing,
    pub interrupts: InterruptState,
}

impl HardwareQueues {
    pub const fn new() -> Self {
        Self {
            tx: DmaRing::new(),
            rx: DmaRing::new(),
            interrupts: InterruptState {
                pending: 0,
                handled: 0,
            },
        }
    }

    pub fn quiesce(&mut self) {
        self.tx.reset();
        self.rx.reset();
        self.interrupts.reset();
    }
}

pub const DMA_PAGE_SIZE: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaMemoryError {
    OutOfFrames,
    DirectMapUnavailable,
    AddressOverflow,
}

/// One kernel-owned 4 KiB frame suitable for a device DMA buffer.
///
/// The frame remains exclusively owned by this value and is returned to the
/// physical allocator on Drop. The virtual address is the bootloader direct
/// mapping of the same physical frame, so CPU and device refer to one backing
/// allocation without copying.
pub struct DmaPage {
    frame: PhysFrame<Size4KiB>,
    physical_address: u64,
    virtual_address: u64,
}

impl DmaPage {
    pub fn allocate_zeroed() -> Result<Self, DmaMemoryError> {
        let frame = memory::allocate_frame().ok_or(DmaMemoryError::OutOfFrames)?;
        let physical_address = frame.start_address().as_u64();
        let Some(offset) = paging::physical_memory_offset() else {
            let _ = memory::deallocate_frame(frame);
            return Err(DmaMemoryError::DirectMapUnavailable);
        };
        let Some(virtual_address) = offset.checked_add(physical_address) else {
            let _ = memory::deallocate_frame(frame);
            return Err(DmaMemoryError::AddressOverflow);
        };

        // SAFETY: `frame` is exclusively owned by this DmaPage and the
        // bootloader direct map makes the complete 4 KiB frame writable at
        // `virtual_address`.
        unsafe {
            core::ptr::write_bytes(virtual_address as *mut u8, 0, DMA_PAGE_SIZE);
        }
        Ok(Self {
            frame,
            physical_address,
            virtual_address,
        })
    }

    pub const fn physical_address(&self) -> u64 {
        self.physical_address
    }
    pub const fn virtual_address(&self) -> u64 {
        self.virtual_address
    }
    pub const fn len(&self) -> usize {
        DMA_PAGE_SIZE
    }
    pub const fn is_empty(&self) -> bool {
        false
    }

    pub fn write_u32(&mut self, offset: usize, value: u32) -> Result<(), DmaMemoryError> {
        if offset & 3 != 0 || offset.checked_add(4).is_none_or(|end| end > DMA_PAGE_SIZE) {
            return Err(DmaMemoryError::AddressOverflow);
        }
        // SAFETY: Bounds/alignment were checked and this DmaPage uniquely owns
        // the backing frame for the duration of the write.
        unsafe {
            ((self.virtual_address + offset as u64) as *mut u32).write_volatile(value);
        }
        Ok(())
    }

    pub fn read_u32(&self, offset: usize) -> Result<u32, DmaMemoryError> {
        if offset & 3 != 0 || offset.checked_add(4).is_none_or(|end| end > DMA_PAGE_SIZE) {
            return Err(DmaMemoryError::AddressOverflow);
        }
        // SAFETY: Bounds/alignment were checked and the direct mapping remains
        // live while self owns the physical frame.
        Ok(unsafe { ((self.virtual_address + offset as u64) as *const u32).read_volatile() })
    }
}

impl Drop for DmaPage {
    fn drop(&mut self) {
        // The allocator rejects duplicate returns; ownership of `frame` is
        // confined to this value, so a normal Drop returns it exactly once.
        let _ = memory::deallocate_frame(self.frame);
    }
}

/// Publish CPU writes before handing DMA ownership to a device.
pub fn dma_publish() {
    fence(Ordering::Release);
}

/// Observe device DMA writes before the CPU consumes a completed descriptor.
pub fn dma_consume() {
    fence(Ordering::Acquire);
}

/// Audited volatile 32-bit register access over an already mapped MMIO window.
///
/// Construction is unsafe because the caller must prove that `virtual_base`
/// denotes a live device mapping for the complete span. Offset validation and
/// all volatile pointer operations are then contained here.
pub struct VolatileMmio32 {
    region: MmioRegion,
    virtual_base: u64,
}

impl VolatileMmio32 {
    /// # Safety
    ///
    /// `virtual_base..virtual_base + region.span()` must be a valid, writable
    /// kernel mapping of the device register window for the lifetime of this
    /// value. No ordinary RAM alias may be concurrently treated as Rust data.
    pub const unsafe fn from_mapped(region: MmioRegion, virtual_base: u64) -> Self {
        Self {
            region,
            virtual_base,
        }
    }

    pub fn read(&self, offset: usize) -> Result<u32, MmioError> {
        let _ = self.region.register_address(offset)?;
        let virtual_address = self
            .virtual_base
            .checked_add(offset as u64)
            .ok_or(MmioError::OutOfRange)?;
        // SAFETY: Constructor contract establishes a live MMIO mapping and
        // register_address validated this aligned 32-bit access.
        Ok(unsafe { (virtual_address as *const u32).read_volatile() })
    }

    pub fn write(&mut self, offset: usize, value: u32) -> Result<(), MmioError> {
        let _ = self.region.register_address(offset)?;
        let virtual_address = self
            .virtual_base
            .checked_add(offset as u64)
            .ok_or(MmioError::OutOfRange)?;
        // SAFETY: Constructor contract establishes a live writable MMIO
        // mapping and register_address validated this aligned 32-bit access.
        unsafe { (virtual_address as *mut u32).write_volatile(value) };
        Ok(())
    }
}

/// Map one physical device register page uncached/NX and return the isolated
/// volatile accessor. Stage 13.10C deliberately limits a binding to one page;
/// larger chipset BARs will be mapped page-by-page by the concrete driver.
pub fn map_mmio_page(physical: u64) -> Result<VolatileMmio32, MmioError> {
    if physical & (DMA_PAGE_SIZE as u64 - 1) != 0 {
        return Err(MmioError::Unaligned);
    }
    let virtual_base = paging::map_mmio(physical).map_err(|_| MmioError::OutOfRange)?;
    let region = MmioRegion::new(physical, DMA_PAGE_SIZE)?;
    // SAFETY: paging::map_mmio created a writable, uncached, NX kernel mapping
    // of the physical register page represented by `region`.
    Ok(unsafe { VolatileMmio32::from_mapped(region, virtual_base) })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorOwner {
    Cpu,
    Device,
    Completed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueError {
    Full,
    Empty,
    InvalidLength,
    NotCpuOwned,
    NotDeviceOwned,
    NotCompleted,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct OwnedDescriptor {
    descriptor: DmaDescriptor,
    owner: DescriptorOwner,
}

impl OwnedDescriptor {
    const fn empty() -> Self {
        Self {
            descriptor: DmaDescriptor {
                physical_address: 0,
                length: 0,
                flags: 0,
            },
            owner: DescriptorOwner::Cpu,
        }
    }
}

/// Fixed-capacity DMA descriptor lifecycle.
///
/// CPU -> Device is a publish operation: descriptor fields are fully written
/// before the ownership transition. Device -> Completed is represented by the
/// interrupt/backend side. Completed -> CPU is the only legal reclaim path,
/// preventing a buffer from being reused while hardware still owns it.
pub struct DeviceQueue {
    entries: [OwnedDescriptor; DMA_RING_CAPACITY],
    head: usize,
    tail: usize,
    count: usize,
    device_owned: usize,
    completed: usize,
}

impl DeviceQueue {
    pub const fn new() -> Self {
        Self {
            entries: [OwnedDescriptor::empty(); DMA_RING_CAPACITY],
            head: 0,
            tail: 0,
            count: 0,
            device_owned: 0,
            completed: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.count
    }
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub const fn device_owned(&self) -> usize {
        self.device_owned
    }
    pub const fn completed(&self) -> usize {
        self.completed
    }

    pub fn submit(&mut self, descriptor: DmaDescriptor) -> Result<usize, QueueError> {
        if descriptor.length == 0 {
            return Err(QueueError::InvalidLength);
        }
        if self.count == DMA_RING_CAPACITY {
            return Err(QueueError::Full);
        }
        if self.entries[self.tail].owner != DescriptorOwner::Cpu {
            return Err(QueueError::NotCpuOwned);
        }
        let slot = self.tail;
        self.entries[slot].descriptor = descriptor;
        dma_publish();
        self.entries[slot].owner = DescriptorOwner::Device;
        self.tail = (self.tail + 1) % DMA_RING_CAPACITY;
        self.count += 1;
        self.device_owned += 1;
        Ok(slot)
    }

    /// Model the device/interrupt side completing one descriptor. A concrete
    /// chipset ISR will call the equivalent transition only after reading the
    /// hardware completion state.
    pub fn complete(&mut self, slot: usize) -> Result<(), QueueError> {
        if slot >= DMA_RING_CAPACITY || self.entries[slot].owner != DescriptorOwner::Device {
            return Err(QueueError::NotDeviceOwned);
        }
        dma_consume();
        self.entries[slot].owner = DescriptorOwner::Completed;
        self.device_owned -= 1;
        self.completed += 1;
        Ok(())
    }

    pub fn reclaim(&mut self) -> Result<DmaDescriptor, QueueError> {
        if self.count == 0 {
            return Err(QueueError::Empty);
        }
        if self.entries[self.head].owner != DescriptorOwner::Completed {
            return Err(QueueError::NotCompleted);
        }
        dma_consume();
        let descriptor = self.entries[self.head].descriptor;
        self.entries[self.head] = OwnedDescriptor::empty();
        self.head = (self.head + 1) % DMA_RING_CAPACITY;
        self.count -= 1;
        self.completed -= 1;
        Ok(descriptor)
    }

    /// Reset is legal only after hardware has been stopped and no descriptor
    /// remains device-owned. This prevents teardown from freeing DMA memory
    /// while a bus master may still access it.
    pub fn quiesce(&mut self) -> Result<(), QueueError> {
        if self.device_owned != 0 {
            return Err(QueueError::NotCpuOwned);
        }
        self.entries = [OwnedDescriptor::empty(); DMA_RING_CAPACITY];
        self.head = 0;
        self.tail = 0;
        self.count = 0;
        self.completed = 0;
        Ok(())
    }
}

/// Real DMA backing retained across the full queue ownership lifecycle.
///
/// The DmaPage itself stays inside this value, so its Drop implementation
/// cannot return the physical frame to the allocator while a device-owned
/// descriptor still refers to that frame.
pub struct OwnedDmaBuffer {
    first_frame: PhysFrame<Size4KiB>,
    physical_address: u64,
    virtual_address: u64,
    page_count: usize,
    length: u16,
    flags: u16,
}
impl OwnedDmaBuffer {
    pub fn allocate(length: u16, flags: u16) -> Result<Self, DmaMemoryError> {
        if length == 0 {
            return Err(DmaMemoryError::AddressOverflow);
        }
        let page_count = usize::from(length).div_ceil(DMA_PAGE_SIZE);
        let first_frame =
            memory::allocate_contiguous_frames(page_count).ok_or(DmaMemoryError::OutOfFrames)?;
        let physical_address = first_frame.start_address().as_u64();
        let release = |count: usize| {
            for index in 0..count {
                if let Ok(frame) = PhysFrame::from_start_address(PhysAddr::new(
                    physical_address + (index * DMA_PAGE_SIZE) as u64,
                )) {
                    let _ = memory::deallocate_frame(frame);
                }
            }
        };
        let Some(offset) = paging::physical_memory_offset() else {
            release(page_count);
            return Err(DmaMemoryError::DirectMapUnavailable);
        };
        let Some(virtual_address) = offset.checked_add(physical_address) else {
            release(page_count);
            return Err(DmaMemoryError::AddressOverflow);
        };
        let allocation_len = page_count
            .checked_mul(DMA_PAGE_SIZE)
            .ok_or(DmaMemoryError::AddressOverflow)?;
        unsafe { core::ptr::write_bytes(virtual_address as *mut u8, 0, allocation_len) };
        Ok(Self {
            first_frame,
            physical_address,
            virtual_address,
            page_count,
            length,
            flags,
        })
    }
    pub const fn physical_address(&self) -> u64 {
        self.physical_address
    }
    pub const fn virtual_address(&self) -> u64 {
        self.virtual_address
    }
    pub const fn length(&self) -> u16 {
        self.length
    }
    pub const fn page_count(&self) -> usize {
        self.page_count
    }
    pub const fn flags(&self) -> u16 {
        self.flags
    }
    pub fn descriptor(&self) -> DmaDescriptor {
        DmaDescriptor {
            physical_address: self.physical_address,
            length: self.length,
            flags: self.flags,
        }
    }
    pub fn write_u32(&mut self, offset: usize, value: u32) -> Result<(), DmaMemoryError> {
        if offset & 3 != 0
            || offset
                .checked_add(4)
                .is_none_or(|end| end > usize::from(self.length))
        {
            return Err(DmaMemoryError::AddressOverflow);
        }
        unsafe { ((self.virtual_address + offset as u64) as *mut u32).write_volatile(value) };
        Ok(())
    }
    pub fn read_u32(&self, offset: usize) -> Result<u32, DmaMemoryError> {
        if offset & 3 != 0
            || offset
                .checked_add(4)
                .is_none_or(|end| end > usize::from(self.length))
        {
            return Err(DmaMemoryError::AddressOverflow);
        }
        Ok(unsafe { ((self.virtual_address + offset as u64) as *const u32).read_volatile() })
    }
    pub fn write_bytes(&mut self, offset: usize, bytes: &[u8]) -> Result<(), DmaMemoryError> {
        let end = offset
            .checked_add(bytes.len())
            .ok_or(DmaMemoryError::AddressOverflow)?;
        if end > usize::from(self.length) {
            return Err(DmaMemoryError::AddressOverflow);
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (self.virtual_address as *mut u8).add(offset),
                bytes.len(),
            );
        }
        Ok(())
    }
    pub fn read_byte(&self, offset: usize) -> Result<u8, DmaMemoryError> {
        if offset >= usize::from(self.length) {
            return Err(DmaMemoryError::AddressOverflow);
        }
        Ok(unsafe { ((self.virtual_address as *const u8).add(offset)).read_volatile() })
    }

    pub fn read_bytes(&self, offset: usize, out: &mut [u8]) -> Result<(), DmaMemoryError> {
        let end = offset
            .checked_add(out.len())
            .ok_or(DmaMemoryError::AddressOverflow)?;
        if end > usize::from(self.length) {
            return Err(DmaMemoryError::AddressOverflow);
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                (self.virtual_address as *const u8).add(offset),
                out.as_mut_ptr(),
                out.len(),
            );
        }
        Ok(())
    }
}
impl Drop for OwnedDmaBuffer {
    fn drop(&mut self) {
        let base = self.first_frame.start_address().as_u64();
        for index in 0..self.page_count {
            if let Ok(frame) =
                PhysFrame::from_start_address(PhysAddr::new(base + (index * DMA_PAGE_SIZE) as u64))
            {
                let _ = memory::deallocate_frame(frame);
            }
        }
    }
}
pub fn stage13_10u_dma_self_test() -> bool {
    let before = memory::stats().allocated_frames;
    {
        let Ok(mut buffer) = OwnedDmaBuffer::allocate(32 * 1024, 0) else {
            return false;
        };
        if buffer.page_count() != 8
            || buffer.length() != 32 * 1024
            || buffer.physical_address() & (DMA_PAGE_SIZE as u64 - 1) != 0
        {
            return false;
        }
        let pattern = [0x5au8; 96];
        if buffer.write_bytes(DMA_PAGE_SIZE - 32, &pattern).is_err()
            || buffer.read_byte(DMA_PAGE_SIZE - 32) != Ok(0x5a)
            || buffer.read_byte(DMA_PAGE_SIZE + 63) != Ok(0x5a)
            || buffer
                .write_u32(DMA_PAGE_SIZE * 2 - 4, 0x1122_3344)
                .is_err()
            || buffer.read_u32(DMA_PAGE_SIZE * 2 - 4) != Ok(0x1122_3344)
            || buffer.read_byte(32 * 1024).is_ok()
        {
            return false;
        }
    }
    memory::stats().allocated_frames == before
}

struct BufferSlot {
    owner: DescriptorOwner,
    buffer: Option<OwnedDmaBuffer>,
}

impl BufferSlot {
    const fn empty() -> Self {
        Self {
            owner: DescriptorOwner::Cpu,
            buffer: None,
        }
    }
}

/// Fixed-capacity queue that owns the actual DMA pages, not just their
/// descriptor values. A frame cannot be dropped/recycled until reclaim()
/// returns the OwnedDmaBuffer to CPU ownership.
pub struct OwnedBufferQueue {
    entries: [BufferSlot; DMA_RING_CAPACITY],
    head: usize,
    tail: usize,
    count: usize,
    device_owned: usize,
    completed: usize,
}

impl OwnedBufferQueue {
    pub const fn new() -> Self {
        Self {
            entries: [const { BufferSlot::empty() }; DMA_RING_CAPACITY],
            head: 0,
            tail: 0,
            count: 0,
            device_owned: 0,
            completed: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.count
    }
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub const fn device_owned(&self) -> usize {
        self.device_owned
    }
    pub const fn completed(&self) -> usize {
        self.completed
    }

    pub fn submit(&mut self, buffer: OwnedDmaBuffer) -> Result<(usize, DmaDescriptor), QueueError> {
        if buffer.length() == 0 {
            return Err(QueueError::InvalidLength);
        }
        if self.count == DMA_RING_CAPACITY {
            return Err(QueueError::Full);
        }
        if self.entries[self.tail].owner != DescriptorOwner::Cpu
            || self.entries[self.tail].buffer.is_some()
        {
            return Err(QueueError::NotCpuOwned);
        }

        let slot = self.tail;
        let descriptor = buffer.descriptor();
        self.entries[slot].buffer = Some(buffer);
        dma_publish();
        self.entries[slot].owner = DescriptorOwner::Device;
        self.tail = (self.tail + 1) % DMA_RING_CAPACITY;
        self.count += 1;
        self.device_owned += 1;
        Ok((slot, descriptor))
    }

    pub fn device_write(&mut self, slot: usize, bytes: &[u8]) -> Result<(), QueueError> {
        if slot >= DMA_RING_CAPACITY || self.entries[slot].owner != DescriptorOwner::Device {
            return Err(QueueError::NotDeviceOwned);
        }
        let Some(buffer) = self.entries[slot].buffer.as_mut() else {
            return Err(QueueError::NotDeviceOwned);
        };
        buffer
            .write_bytes(0, bytes)
            .map_err(|_| QueueError::InvalidLength)
    }
    pub fn complete(&mut self, slot: usize) -> Result<(), QueueError> {
        if slot >= DMA_RING_CAPACITY
            || self.entries[slot].owner != DescriptorOwner::Device
            || self.entries[slot].buffer.is_none()
        {
            return Err(QueueError::NotDeviceOwned);
        }
        dma_consume();
        self.entries[slot].owner = DescriptorOwner::Completed;
        self.device_owned -= 1;
        self.completed += 1;
        Ok(())
    }

    pub fn reclaim_with_slot(&mut self) -> Result<(usize, OwnedDmaBuffer), QueueError> {
        if self.count == 0 {
            return Err(QueueError::Empty);
        }
        if self.entries[self.head].owner != DescriptorOwner::Completed {
            return Err(QueueError::NotCompleted);
        }
        dma_consume();
        let slot = self.head;
        let Some(buffer) = self.entries[slot].buffer.take() else {
            return Err(QueueError::NotCompleted);
        };
        self.entries[slot].owner = DescriptorOwner::Cpu;
        self.head = (self.head + 1) % DMA_RING_CAPACITY;
        self.count -= 1;
        self.completed -= 1;
        Ok((slot, buffer))
    }
    pub fn reclaim(&mut self) -> Result<OwnedDmaBuffer, QueueError> {
        self.reclaim_with_slot().map(|(_, buffer)| buffer)
    }
    /// Teardown is legal only once hardware no longer owns any slot. Completed
    /// or CPU-owned buffers may then drop normally and return frames safely.
    pub fn quiesce(&mut self) -> Result<(), QueueError> {
        if self.device_owned != 0 {
            return Err(QueueError::NotCpuOwned);
        }
        for entry in &mut self.entries {
            entry.buffer = None;
            entry.owner = DescriptorOwner::Cpu;
        }
        self.head = 0;
        self.tail = 0;
        self.count = 0;
        self.completed = 0;
        Ok(())
    }
}

pub fn stage13_10f_self_test() -> bool {
    let before = memory::stats().allocated_frames;

    {
        let mut queue = OwnedBufferQueue::new();
        let Ok(mut first) = OwnedDmaBuffer::allocate(1536, 0x31) else {
            return false;
        };
        let first_phys = first.physical_address();

        if first_phys & (DMA_PAGE_SIZE as u64 - 1) != 0
            || first.length() != 1536
            || first.write_u32(0, 0x5748_4642).is_err()
            || first.read_u32(0) != Ok(0x5748_4642)
        {
            return false;
        }

        let Ok((slot, descriptor)) = queue.submit(first) else {
            return false;
        };
        if descriptor.physical_address != first_phys
            || descriptor.length != 1536
            || descriptor.flags != 0x31
            || queue.device_owned() != 1
            || queue.completed() != 0
            || queue.quiesce() != Err(QueueError::NotCpuOwned)
        {
            return false;
        }

        // The queue owns the real DmaPage here. The allocator must still show
        // the frame as live while the device owns it.
        if memory::stats().allocated_frames != before + 1 {
            return false;
        }

        if queue.complete(slot).is_err() || queue.device_owned() != 0 || queue.completed() != 1 {
            return false;
        }

        let Ok(reclaimed) = queue.reclaim() else {
            return false;
        };
        if reclaimed.physical_address() != first_phys
            || reclaimed.read_u32(0) != Ok(0x5748_4642)
            || !queue.is_empty()
        {
            return false;
        }

        // Reclaimed buffer is CPU-owned but still alive until this Drop.
        if memory::stats().allocated_frames != before + 1 {
            return false;
        }
        drop(reclaimed);
        if memory::stats().allocated_frames != before {
            return false;
        }

        // Also prove quiesce safely drops completed-but-not-reclaimed buffers.
        let Ok(second) = OwnedDmaBuffer::allocate(512, 0x44) else {
            return false;
        };
        let Ok((second_slot, _)) = queue.submit(second) else {
            return false;
        };
        if queue.complete(second_slot).is_err()
            || memory::stats().allocated_frames != before + 1
            || queue.quiesce().is_err()
            || memory::stats().allocated_frames != before
        {
            return false;
        }
    }

    memory::stats().allocated_frames == before
}
pub fn stage13_10d_self_test() -> bool {
    let mut queue = DeviceQueue::new();
    let first = DmaDescriptor {
        physical_address: 0x0040_0000,
        length: 1536,
        flags: 0x11,
    };
    let second = DmaDescriptor {
        physical_address: 0x0041_0000,
        length: 2048,
        flags: 0x22,
    };

    let Ok(first_slot) = queue.submit(first) else {
        return false;
    };
    let Ok(second_slot) = queue.submit(second) else {
        return false;
    };
    if first_slot != 0
        || second_slot != 1
        || queue.len() != 2
        || queue.device_owned() != 2
        || queue.completed() != 0
        || queue.reclaim() != Err(QueueError::NotCompleted)
        || queue.quiesce() != Err(QueueError::NotCpuOwned)
    {
        return false;
    }

    // Complete out of order. Reclaim must still respect queue head ordering.
    if queue.complete(second_slot).is_err()
        || queue.completed() != 1
        || queue.reclaim() != Err(QueueError::NotCompleted)
        || queue.complete(first_slot).is_err()
        || queue.complete(first_slot) != Err(QueueError::NotDeviceOwned)
        || queue.device_owned() != 0
        || queue.completed() != 2
        || queue.reclaim() != Ok(first)
        || queue.reclaim() != Ok(second)
        || !queue.is_empty()
    {
        return false;
    }

    // Exercise ring wraparound and prove a device-owned slot cannot be reused.
    for index in 0..DMA_RING_CAPACITY {
        let descriptor = DmaDescriptor {
            physical_address: 0x0080_0000 + index as u64 * DMA_PAGE_SIZE as u64,
            length: 512,
            flags: index as u16,
        };
        let Ok(slot) = queue.submit(descriptor) else {
            return false;
        };
        if queue.complete(slot).is_err() || queue.reclaim() != Ok(descriptor) {
            return false;
        }
    }

    queue.is_empty()
        && queue.device_owned() == 0
        && queue.completed() == 0
        && queue.quiesce().is_ok()
}
pub fn stage13_10c_self_test() -> bool {
    let before = memory::stats().allocated_frames;
    {
        let Ok(mut page) = DmaPage::allocate_zeroed() else {
            return false;
        };
        if page.physical_address() & (DMA_PAGE_SIZE as u64 - 1) != 0
            || page.virtual_address() == 0
            || page.len() != DMA_PAGE_SIZE
            || page.is_empty()
            || page.read_u32(0) != Ok(0)
            || page.write_u32(0, 0x5748_444d).is_err()
        {
            return false;
        }
        dma_publish();
        dma_consume();
        if page.read_u32(0) != Ok(0x5748_444d)
            || page.write_u32(2, 1).is_ok()
            || page.read_u32(DMA_PAGE_SIZE).is_ok()
        {
            return false;
        }

        // Exercise the same audited volatile access boundary against the
        // owned test frame. This validates pointer containment without
        // touching a nonexistent physical Wi-Fi device in QEMU.
        let Ok(region) = MmioRegion::new(page.physical_address(), DMA_PAGE_SIZE) else {
            return false;
        };
        // SAFETY: For this self-test the DmaPage is live, writable and
        // exclusively owned for the complete synthetic register window.
        let mut registers = unsafe { VolatileMmio32::from_mapped(region, page.virtual_address()) };
        if registers.write(4, 0xa5a5_5a5a).is_err()
            || registers.read(4) != Ok(0xa5a5_5a5a)
            || registers.read(2).is_ok()
            || registers.write(DMA_PAGE_SIZE, 0).is_ok()
        {
            return false;
        }
    }
    memory::stats().allocated_frames == before
}
pub fn stage13_10b_self_test() -> bool {
    let Ok(mmio) = MmioRegion::new(0xfebc_0000, 0x1000) else {
        return false;
    };
    if mmio.register_address(0) != Ok(0xfebc_0000)
        || mmio.register_address(0x0ffc) != Ok(0xfebc_0ffc)
        || mmio.register_address(2).is_ok()
        || mmio.register_address(0x1000).is_ok()
    {
        return false;
    }

    let mut queues = HardwareQueues::new();
    let tx = DmaDescriptor {
        physical_address: 0x0020_0000,
        length: 1500,
        flags: 1,
    };
    let rx = DmaDescriptor {
        physical_address: 0x0021_0000,
        length: 1600,
        flags: 2,
    };

    if queues.tx.push(tx).is_err()
        || queues.rx.push(rx).is_err()
        || queues.tx.len() != 1
        || queues.rx.len() != 1
    {
        return false;
    }

    queues.interrupts.raise(0x1);
    queues.interrupts.raise(0x4);
    if queues.interrupts.take() != 0x5
        || queues.interrupts.pending != 0
        || queues.interrupts.handled != 1
    {
        return false;
    }

    if queues.tx.pop() != Ok(tx) || queues.rx.pop() != Ok(rx) {
        return false;
    }

    for index in 0..DMA_RING_CAPACITY {
        let descriptor = DmaDescriptor {
            physical_address: 0x0030_0000 + (index as u64 * 0x1000),
            length: 512,
            flags: 0,
        };
        if queues.tx.push(descriptor).is_err() {
            return false;
        }
    }
    if queues.tx.push(tx) != Err(DmaError::Full) {
        return false;
    }

    queues.interrupts.raise(0xffff);
    queues.quiesce();
    queues.tx.is_empty()
        && queues.rx.is_empty()
        && queues.interrupts.pending == 0
        && queues.interrupts.handled == 1
}
pub fn self_test() -> bool {
    let mut good = pci::Device {
        segment: 0,
        bus: 2,
        device: 3,
        function: 0,
        vendor_id: 0x8086,
        device_id: 0x2723,
        revision: 1,
        class: 0x02,
        subclass: 0x80,
        ..pci::Device::default()
    };
    good.bars[0] = pci::Bar {
        valid: true,
        kind: pci::BarKind::Memory64,
        address: 0xfebc_0000,
        prefetchable: false,
    };
    good.capabilities.msi = true;
    good.capabilities.pcie = true;
    let Ok(desc) = descriptor_from_device(good) else {
        return false;
    };
    if desc.vendor_id != 0x8086
        || desc.device_id != 0x2723
        || desc.mmio_bar != 0
        || desc.mmio_base != 0xfebc_0000
        || !desc.msi
        || !desc.pcie
    {
        return false;
    }

    let mut wired = good;
    wired.subclass = 0x00;
    if descriptor_from_device(wired) != Err(HwBindError::NotWireless) {
        return false;
    }

    let mut no_bar = good;
    no_bar.bars = [pci::Bar::default(); 6];
    descriptor_from_device(no_bar) == Err(HwBindError::NoMmioBar)
}

# Kernel Device Registry

WovenHat exposes boot hardware through a bounded kernel registry in `kernel/src/device.rs`.
The registry is initialized after the concrete drivers are initialized and before IRQs are
unmasked.

## Registered boot devices

| Name | Kind | IRQ |
| --- | --- | --- |
| `framebuffer-console` | Console | none |
| `com1` | Serial | none |
| `pit` | Timer | 0 |
| `ps2-keyboard` | Keyboard | 1 |

The PS/2 interrupt path queues raw set-1 scancodes. A shared decoder supplies both the
kernel UI and the nonblocking userspace standard-input endpoint without allocating in
the IRQ handler.

## PCI inventory

The HAL scans PCI configuration mechanism 1 across all 256 buses, 32 device slots,
and advertised multifunction functions. It records up to 64 functions and reports
total storage, network, display, and bridge classes over serial. Inventory truncation
is explicit. Enumeration is read-only and completes before interrupts are enabled.
A detected legacy primary-master disk is registered separately as ata0.

## ACPI inventory

The HAL consumes the physical RSDP address supplied by the bootloader. It validates
the ACPI 1.0 checksum and, for revision 2 or newer, the extended checksum before
following the RSDT or XSDT. Every physical read must fit within one boot memory-map
region. SDT lengths are capped at 64 KiB, checksums are verified, and enumeration is
capped at 256 tables. A validated MADT is parsed into bounded processor, I/O APIC,
interrupt-override, and local-APIC-address topology. Boot reports that topology plus
FADT, HPET, and MCFG presence over serial; missing or invalid firmware data is reported
without preventing fallback boot. See [interrupt-controller topology](interrupt-topology.md).

## Invariants

- The registry has a fixed capacity and does not allocate.
- Device names are unique.
- A hardware IRQ can belong to only one registered device.
- Lookup returns a copied descriptor, so callers cannot mutate registry state.
- Boot fails closed if registration or the registry self-test fails.

## Extension path

PCI functions are inventoried but are only added to the device registry after a driver
binds them. The validated MADT topology must still be applied to local APIC and I/O
APIC initialization and IRQ routing. Driver binding should extend Device with bus identity and lifecycle state
while preserving the name and IRQ uniqueness checks.

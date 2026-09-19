# Stage 13.2 PCI/PCIe — implementation candidate

Status: **implemented; acceptance pending on the Windows/QEMU gate**.

## Scope implemented

- ACPI MCFG allocation parsing with checksum/range validation inherited from the ACPI table walker.
- Bounded ECAM region inventory with segment group and bus-range metadata.
- ECAM configuration-space address calculation and MMIO access, with legacy PCI configuration mechanism #1 fallback when MCFG is unavailable.
- SMP-safe serialization of legacy CF8/CFC configuration transactions and configuration read/modify/write operations.
- Rich PCI function identity: segment/BDF, vendor/device, revision, programming interface, class/subclass, header type, command/status.
- Assigned BAR decoding for I/O, 32-bit MMIO and 64-bit MMIO BARs.
- Bounded standard capability traversal for PM, MSI, MSI-X and PCIe capability discovery, including malformed-chain detection.
- Existing `read_config_dword`, `write_config_dword`, `enable_io_bus_master`, `bar0_io_base`, and inventory APIs retained for legacy VirtIO networking compatibility.
- Stage 13.2 QEMU feature, marker, runtime harness support and 1/2/4 CPU acceptance script.

## Deliberately not claimed in this candidate

- BAR size probing/resource allocation or rebalance.
- PCIe extended capability traversal beyond the first 256-byte conventional capability list.
- MSI/MSI-X vector allocation/programming.
- PCI hot-plug and power-management policy.
- IOMMU/DMA isolation.
- NVMe/AHCI/USB drivers; those remain later driver milestones.

## Acceptance gate

Run `powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage13-2-acceptance.ps1`.
Do not mark Stage 13.2 accepted or update the accepted-stage status documents until build, warning-denying Clippy and QEMU 1/2/4 CPU runs all pass.

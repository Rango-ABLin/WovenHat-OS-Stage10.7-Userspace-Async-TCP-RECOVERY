# Stage 13.4 AHCI/SATA candidate audit

## Scope

Stage 13.4 adds a native AHCI/SATA block backend on top of the accepted Stage 13.2 PCIe and Stage 13.3 NVMe foundations. It discovers PCI class 01h/subclass 06h/programming-interface 01h controllers, enables PCI memory decoding and bus mastering, selects an implemented active SATA disk port, programs command-list and received-FIS DMA bases, and exposes 512-byte SATA media through the existing `BlockDevice` contract.

## Architecture

The driver uses allocator-owned 4 KiB DMA pages for the command list, received FIS area, one command table/PRDT and a bounded transfer buffer. It stops the AHCI command engine before rebasing, clears port errors/status, restarts the engine, issues IDENTIFY DEVICE, requires 48-bit LBA support, and uses READ DMA EXT / WRITE DMA EXT / FLUSH CACHE EXT for block I/O. Access is serialized by the existing IRQ-safe controller mutex.

Legacy ATA PIO and NVMe are preserved as independent backends. Stage 13.4 does not replace either path.

## Acceptance

`run-stage13-4-acceptance.ps1` requires build, warning-denying freestanding Clippy, and isolated QEMU boots on 1/2/4 CPUs. The runtime harness attaches a dedicated 16 MiB SATA disk to an ICH9 AHCI controller. Each boot must discover AHCI, initialize a SATA disk, perform write/flush/read round-trip through `BlockDevice`, restore sector zero, and exit through the standard debug-exit success path.

## Production gaps

The stage deliberately uses one command slot and bounded polling for synchronous completion. Interrupt-driven completion, NCQ/multi-slot scaling, ATAPI, hotplug, staggered spin-up, enclosure management, controller reset recovery, multiple disks/controllers, advanced power management, TRIM/DSM, and broad real-hardware qualification remain later production work.

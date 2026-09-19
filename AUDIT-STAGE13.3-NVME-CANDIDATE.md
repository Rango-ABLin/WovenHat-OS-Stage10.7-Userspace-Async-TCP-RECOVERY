# Stage 13.3 NVMe candidate audit

## Scope

Stage 13.3 adds the first native WovenHat NVMe path on top of the accepted Stage 13.2 PCI/PCIe foundation. The implementation discovers PCI class 01h/subclass 08h/programming-interface 02h controllers, enables PCI memory decoding and bus mastering, maps BAR MMIO uncached, initializes NVMe admin and I/O queue pairs, identifies controller/namespace 1, and exposes a 512-byte namespace through the existing `BlockDevice` contract.

## Architecture

The driver owns physically contiguous 4 KiB DMA pages for admin SQ/CQ, I/O SQ/CQ, identify data, and a bounded transfer buffer. Queue entries use command IDs and NVMe completion phase bits. Release/acquire fences bracket submission/completion visibility. The controller is serialized behind the existing IRQ-safe mutex so Stage 13.3 does not introduce an unsynchronized cross-CPU queue path.

The stage deliberately preserves ATA and VirtIO storage paths. NVMe is an additional block backend rather than a replacement.

## Acceptance

`run-stage13-3-acceptance.ps1` requires build, warning-denying freestanding Clippy, and isolated QEMU boots on 1/2/4 CPUs. The runtime harness attaches a dedicated 16 MiB QEMU NVMe namespace. Each boot must discover and initialize the controller, identify the namespace, perform a write/flush/read round trip through `BlockDevice`, restore the original sector, and exit through the standard debug-exit success path.

## Safety and production gaps

Unsafe code is restricted to volatile MMIO and exclusive direct-map access to allocator-owned DMA pages. Commands and queues are bounded. This stage uses polling for bounded controller bring-up and synchronous queue completion; MSI/MSI-X, interrupt-driven completion, multi-queue scaling, multiple namespaces/controllers, larger transfers/PRP lists/SGLs, reset recovery, power management, hotplug, and broad physical-hardware qualification remain explicit later production work.

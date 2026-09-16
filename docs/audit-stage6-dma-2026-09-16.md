# Stage 6 VirtIO DMA audit — 2026-09-16

VirtIO network initialization now obtains a physically contiguous DMA arena
from the frame allocator instead of relying on the bootloader's static BSS
layout. The arena contains both legacy split-virtqueue regions and packet
buffers. The direct physical-memory mapping provides the kernel virtual
address, while the physical base is programmed into the device descriptors.

The allocator reserves the whole run from one usable memory range and applies
the current NUMA-domain preference. If initialization fails after reservation,
an ownership guard returns every reserved frame to the allocator. A
`network-test` image reports transport initialization errors explicitly and
halts, so a missing DMA capability cannot be mistaken for a successful network
qualification.

Validation completed:

```powershell
cargo clippy -p wovenhat-kernel --target x86_64-unknown-none --features network-test -- -D warnings
python scripts/test-network-qemu.py --cpus 1
python scripts/test-release.py
```

The focused 1-CPU live DHCP/DNS/ICMP/UDP/TCP gate passed. The complete release
matrix passed network and storage on 1, 2, and 4 CPUs in debug mode, plus the
4-CPU release network gate. Repeated Stage 6 hotplug tests also passed on 2
and 4 CPUs after the allocator change.

This closes the software-side contiguous-DMA allocation gap for the current
VirtIO network path. Broader DMA-capable drivers, IOMMU isolation, and
hardware throughput qualification remain separate production requirements.

# WovenHat OS 0.1.0 — Stage 3 Standalone Runtime

Stage 3 moves WovenHat from a kernel-debug environment toward a standalone OS runtime.

## Native userspace terminal

- Userspace `write(1, ...)` and `write(2, ...)` are rendered to the UEFI framebuffer and mirrored to COM1.
- The framebuffer terminal supports line wrapping, scrolling, backspace, tab, and the clear/home ANSI sequences used by `/bin/sh`.
- `sh` transfers foreground terminal ownership to the spawned userspace process.
- While userspace owns the foreground, the kernel diagnostic shell does not consume PS/2 keyboard input.
- Process exit/fault releases foreground ownership and the kernel shell is restored automatically.

## VirtIO-net dataplane

- Transitional VirtIO-net PCI discovery (`vendor 1af4`, device `1000`).
- PCI I/O-space and bus-master enablement.
- Legacy VirtIO status/feature negotiation.
- Real split virtqueues for RX queue 0 and TX queue 1.
- Page-aligned DMA descriptor memory with physical-contiguity verification.
- Eight posted receive buffers and one bounded transmit buffer.
- Used-ring reclaim, receive re-posting, queue notification, and transport statistics.
- Polling dataplane first; legacy ISR is acknowledged while descriptor indices remain authoritative.
- smoltcp Ethernet/ARP/IPv4 interface configured at `10.0.2.15/24`, gateway `10.0.2.2` for QEMU user networking.

## QEMU

Use the transitional device while this driver targets the legacy PCI transport:

```powershell
-device virtio-net-pci,netdev=net0,disable-modern=on `
-netdev user,id=net0
```

The modern VirtIO PCI capability transport (device 1041), MSI-X, DHCP, DNS, and POSIX/BSD socket syscalls remain later milestones. Networking is optional at boot: failure to initialize the NIC must not prevent the shell from starting.

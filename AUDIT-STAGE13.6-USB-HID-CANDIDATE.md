# WovenHat OS Stage 13.6 — USB HID Candidate

## Scope
Stage 13.6 builds a bounded USB HID class path on the accepted Stage 13.5 xHCI controller core. It adds xHCI device addressing, EP0 control transfers, USB device/configuration/interface/endpoint descriptor parsing, HID boot-protocol selection, interrupt-IN endpoint configuration, and boot-report reception.

## Acceptance target
The QEMU acceptance harness attaches `qemu-xhci` plus `usb-kbd`. The kernel must enumerate the HID keyboard and complete a real interrupt-IN transfer, preserving the existing SMP and Stage 10.3 regression markers on 1, 2, and 4 CPUs.

## Safety / architecture
- DMA buffers are kernel-owned contiguous pages and remain alive for controller lifetime.
- xHCI MMIO remains behind the existing paging MMIO mapping boundary.
- Controller state is serialized by the existing ranked IRQ mutex.
- Command and transfer waits are bounded; unrelated event-ring entries are demultiplexed instead of treated as immediate command failure.
- Completion remains polling-based because the current kernel does not yet expose a generic MSI/MSI-X allocation/delivery API.
- Stage 13.7 remains responsible for a unified PS/2 + USB input framework.

## Deliberate production gaps
This focused stage does not claim hub support, arbitrary HID report-descriptor parsing, hot-unplug recovery, multiple simultaneous HID devices, interrupt-driven xHCI completion, or a unified input event service. Mouse/touchpad protocol classification is represented, but this acceptance specifically proves the QEMU USB boot keyboard path.

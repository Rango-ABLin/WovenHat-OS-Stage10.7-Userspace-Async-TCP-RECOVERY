# WovenHat OS 0.0.9 — Stage 2 Runtime

Normal boot remains shell-first. Stage 2 adds runtime features without moving expensive validation back into the critical boot path.

## Kernel shell additions

- `devices` / `dev` — show the registered console, serial, timer, keyboard and block devices.
- `net` — probe PCI for a VirtIO network function and report its BDF location. This is discovery only; DMA virtqueue transport remains a later milestone.
- `userland` — install the built-in `/bin` programs into the in-memory VFS on demand.
- `kill <pid> [signal]` — send a signal to a process; signal 15 is the default.
- `sh` — now provisions the built-in userland automatically before spawning the userspace shell.

## Built-in userland

`/bin/user`, `/bin/init`, `/bin/sh`, `/bin/echo`, `/bin/true`, `/bin/false`, `/bin/cat`, `/bin/ls`, `/bin/sleep`, `/bin/pwd`, `/bin/mkdir`, and `/bin/rm`.

## Networking status

VirtIO PCI discovery and bounded software RX/TX queues are present. Physical descriptor rings, DMA mapping, queue notification, interrupt completion, and end-to-end smoltcp packet I/O are not yet implemented.

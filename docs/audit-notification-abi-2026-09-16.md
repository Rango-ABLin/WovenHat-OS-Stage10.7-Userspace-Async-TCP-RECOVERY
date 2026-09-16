# Notification ABI follow-up — 2026-09-16

Added syscall 95, `NotificationPoll`, with IPC capability enforcement and a
fixed 24-byte record (`kind`, `source`, `payload`). The `libwoven::notifications`
record mirrors this layout for userspace consumers. Copyout occurs before the
queue entry is consumed, preserving retry safety for invalid pointers.

Build, freestanding Clippy, Rust regressions, and QEMU notification boots on
1, 2, and 4 CPUs passed.

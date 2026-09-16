# Notification post and libwoven syscall wrappers — 2026-09-16

Added syscall 96 for capability-gated application-defined notifications and
real `libwoven` Ring-3 wrappers for notification post/poll. The wrappers retain
the fixed ABI numbers and document pointer safety requirements.

Build, kernel and `libwoven` Clippy, Rust regressions, and QEMU notification
boots passed on 1, 2, and 4 CPUs.

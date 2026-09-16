# Notification recipient isolation — 2026-09-16

Structured notifications now carry an owner recipient. Queue consumption scans
only records addressed to the current process while preserving FIFO order among
that process's records. The notification syscall therefore cannot consume
another process's child-exit or user event.

Build, freestanding Clippy, Rust regressions, and Stage 11.3 QEMU boots passed
on 1, 2, and 4 CPUs after this change.

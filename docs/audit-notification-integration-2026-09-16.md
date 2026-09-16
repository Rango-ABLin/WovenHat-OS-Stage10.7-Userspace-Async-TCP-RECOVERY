# Notification integration follow-up — 2026-09-16

Process termination now publishes a structured `ChildExit` notification after
the process becomes an exited zombie and before descriptor teardown completes.
The bounded queue remains lock-protected and loss-aware; later userspace ABI
work can consume the same records without polling process tables.

Build, Clippy, Rust regressions, and the Stage 11.3 QEMU notification gate
passed on 1 CPU. The existing 2/4 CPU Stage 11.3 gates remain the multicore
regression matrix for this shared lifecycle path.

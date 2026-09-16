# Foundation storage journal — 2026-09-16

Added a bounded write-ahead intent journal with generation-safe tokens,
commit records, and recovery of uncommitted intents. Mounted VFS mutations
continue to mark the volume dirty until synchronization succeeds.

Build, freestanding Clippy, Rust regressions, and the journal QEMU gate passed
on 1, 2, and 4 CPUs. Evidence is retained in `audit-artifacts/stage1-5-*`.

This closes the journal protocol foundation; the next storage gap is applying
these intents atomically to FAT32 metadata and validating recovery after an
injected write interruption.

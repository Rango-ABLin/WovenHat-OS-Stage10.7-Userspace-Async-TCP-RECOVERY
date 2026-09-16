# Journal persistence follow-up — 2026-09-16

Mounted-file persistence now begins a journal intent before FAT32 writes and
commits it only after the device flush succeeds. The next persistence attempt
replays and retires stale intents, while failed writes remain represented until
recovery. Existing dirty-volume semantics are preserved.

Build, Clippy, Rust regressions, and the disposable FAT32 QEMU suite passed.
The remaining gap is durable journal storage on disk; the current bounded log
protects the in-kernel transaction boundary.

# Foundation storage follow-up — 2026-09-16

The full release matrix passed before and after this change. VFS mutations now
automatically mark a mounted `/mnt` volume dirty for writes, removes, renames,
and directory creation; successful sync clears the dirty state. This closes
one concrete Stage 1–5 gap without changing temporary files or read-only paths.

Validation: host Clippy, Rust regressions, and the disposable FAT32 QEMU boot
passed. The 1 CPU storage evidence is in `target/storage-regression-1-debug`.

# Stage 1–5 bounded gap closure — 2026-09-16

## Canonical path spelling

`kernel/src/vfs.rs` now validates every absolute VFS path against Unicode NFC
using a fixed `MAX_PATH_SIZE` scratch buffer. Decomposed spellings are rejected
at the VFS boundary, so equivalent names cannot create duplicate in-memory
nodes. FAT32 keeps the same NFC comparison for on-disk lookup and long-name
encoding.

## Empty-file metadata safety

FAT32 directory entries with `first_cluster == 0` are valid for empty files but
cannot address the WMD2 inode table. The persistence path now retains the WMD1
path record for these entries and only removes it after a WMD2 record is
successfully written. This prevents ownership metadata loss during empty-file
creation or rewrite.

## Bounded multi-intent journal coverage

The FAT32 self-test now appends two independent metadata intents, removes the
first, and verifies that the second remains readable. This covers ring coexistence
and selective cleanup without claiming a multi-file atomic transaction. Full
data rollback and hardware power-loss qualification remain separate Stage 6+
work.

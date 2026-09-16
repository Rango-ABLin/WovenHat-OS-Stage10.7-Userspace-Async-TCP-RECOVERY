# Data recovery checksum audit — 2026-09-16

Pending FAT32 file intents carry the bounded-file FNV-1a checksum captured
before persistence. Mount/import resolves the matching file, reads it through
the validated FAT32 path, and compares the full file checksum before restoring
ownership metadata or retiring the intent. A mismatch leaves the intent
pending for a later repair pass and does not apply metadata.

The behavior is covered by the FAT32 structural regression, host build and
Clippy gates, and storage QEMU runs on 1, 2, and 4 CPUs. This protects one
file-data transaction; multi-file rollback and physical power-loss testing are
separate production work.

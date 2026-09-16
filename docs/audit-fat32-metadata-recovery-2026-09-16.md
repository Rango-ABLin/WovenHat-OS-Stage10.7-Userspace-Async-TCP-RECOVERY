# FAT32 metadata recovery audit — 2026-09-16

Disk-backed persistence uses a two-state metadata record. A pending ownership
record is flushed before FAT data writes. After the data transaction flushes,
the record is promoted to committed and flushed again.

During the next mounted import, pending records are matched to existing files,
their recorded bounded-file FNV-1a data checksum is verified, and only matching
records are restored to VFS nodes and promoted to committed. This lets an
interrupted update converge after an unclean shutdown without applying metadata
over changed data.

The durable intent is bounded to a two-sector ring with 30 fixed-size records
and is covered by the FAT32 structural self-test plus the 1/2/4 CPU storage
QEMU gate. Multiple outstanding file intents survive a reboot and are replayed
individually during import. The in-memory journal remains as a transaction
guard. Full data rollback beyond the bounded file transaction and hardware
power-loss qualification remain open production work.

# FAT32 metadata recovery audit — 2026-09-16

Disk-backed persistence now uses a two-state metadata record. A pending
ownership record is flushed before FAT data writes. After the data transaction
flushes, the record is promoted to committed and flushed again. During the
next mounted import, pending records are restored to matching VFS nodes and
promoted to committed, allowing an interrupted update to converge after an
unclean shutdown.

The intent is bounded to a two-sector durable ring with 30 fixed-size records
and is covered by the FAT32 structural self-test plus the 1/2/4 CPU storage
QEMU gate. Multiple outstanding file intents survive a reboot and are replayed
individually during import. The in-memory journal remains as a transaction
guard. Full data rollback with checksum verification and hardware power-loss
qualification remain open production work.

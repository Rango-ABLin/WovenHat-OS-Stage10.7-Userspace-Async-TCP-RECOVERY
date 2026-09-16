# Async block I/O completion

`kernel/src/block_io.rs` provides the scheduler-level completion layer for
primary ATA sector operations. It exposes a `BlockDevice` wrapper for ata0, then
queues eligible reads, writes, and flushes to a fixed-size request table. The
`block-io` task drains that table and wakes blocked callers when a request has a
result.

Queueing is deliberately conservative. A request uses the worker only after the
scheduler has started, CPU interrupts are enabled, the caller is a scheduled task,
and the caller is not the block-I/O worker itself. Early boot, interrupts-disabled
paths, and worker reentry use direct ATA access. This avoids deadlocking boot or
the worker while still moving normal shell, persistence, pager, and swap traffic
off the requesting task.

The queue is bounded by `MAX_BLOCK_IO_REQUESTS`. If all slots are active, submit
fails instead of allocating or blocking inside a global lock. Read completion
copies the returned sector into the caller buffer; write and flush completion
propagate the device result. The worker currently executes one sector operation at
a time over the existing legacy ATA PIO driver.

The diagnostic shell exposes:

```text
blockio
iostat
```

Both commands print queued, completed, direct-fallback, pending, and active
request counters, followed by swap slot usage. A normal interactive disk test can
combine this with FAT32 metadata mutation:

```text
iostat
mkdir /mnt/docs
write /mnt/docs/a.txt hello
persist /mnt/docs/a.txt
rename /mnt/docs /mnt/archive
cat /mnt/archive/a.txt
rm /mnt/archive/a.txt
rm /mnt/archive
iostat
```

Successful persistence should print `persist: written to FAT32`, `cat` should
print `hello`, the `rm` commands should remove the file and then the empty
directory, and the second `iostat` should show completed block operations on a
detected ATA disk. `stat /mnt/cache.txt` should report `writable: yes` before
overwriting an imported FAT32 file on writable media.

Validation:

```powershell
cargo build --features qemu-test
python scripts/test-memory-qemu.py
python scripts/test-storage-qemu.py
```

Expected serial checkpoints include:

```text
[BLOCK IO] async completion tests: PASSED
[BLOCK IO] worker completion: PASSED
[STORAGE MUTATION] live FAT32 rename/delete: PASSED
[BOOT] ALL VALIDATIONS PASSED
```

This layer is not yet hardware asynchronous I/O. The ATA driver still uses
bounded PIO polling, now with command-settle delays and retry loops for sector
transfers, and no AHCI/NVMe/virtio interrupt or DMA completion ring is
implemented yet.

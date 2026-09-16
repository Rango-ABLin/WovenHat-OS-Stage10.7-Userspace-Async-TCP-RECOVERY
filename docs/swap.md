# Swap backing

WovenHat swap slots preserve dirty private lazy mmap pages after eviction. Each
mapping stores an opaque swap handle; handles carry a slot generation so stale
references are rejected after release and reuse.

The swap module supports two backings:

- Disk-backed slots write one 4 KiB page as eight 512-byte sectors through the
  generic `BlockDevice` API and flush before publishing the handle. After the
  scheduler starts, eligible primary-ATA sector operations go through the
  bounded `block-io` completion worker.
- RAM-backed slots keep the page in kernel memory and are used when no disk
  area is configured or a disk write cannot complete.

The VM path does not write swap over arbitrary filesystem data. FAT32 mounting
configures ATA swap only when the mounted volume leaves raw trailing sectors
outside its advertised FAT32 sector count. New `wovenhat-disk.img` files created
by `scripts/create-fat32.py` reserve 32 swap pages this way. Existing disk images
are never overwritten by the script; they keep using RAM fallback unless you
remove and recreate the image.

Swap is still an ephemeral paging store, not a hibernation or crash-recovery
format. Slot contents are valid only while the kernel is running. Early boot,
interrupts-disabled code, and the block-I/O worker itself fall back to direct
ATA access. The current completion path is asynchronous at the scheduler level
over the existing PIO driver; hardware interrupt/DMA completion remains a later
driver milestone.

Validation:

```powershell
cargo build --features qemu-test
python scripts/test-memory-qemu.py
```

Expected serial checkpoints include:

```text
[BLOCK IO] async completion tests: PASSED
[SWAP] disk-backed policy tests: PASSED
[PRIVATE SWAP MMAP] dirty eviction/refault: PASSED
[BLOCK IO] worker completion: PASSED
[BOOT] ALL VALIDATIONS PASSED
```

On an interactive boot, `blockio` or `iostat` prints queued/completed/direct
block-I/O counters plus swap slot usage.

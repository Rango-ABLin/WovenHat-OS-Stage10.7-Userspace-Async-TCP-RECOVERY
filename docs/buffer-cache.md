# Persistent buffer cache

The ATA primary-master device owns a bounded, allocation-free 64-sector
(32 KiB payload) write-back buffer cache. The same cache serves boot mounting,
partition discovery, on-demand FAT32 reads, file persistence, directory
persistence, and FAT32 delete/rename metadata updates. Cache keys are absolute
device LBAs; partition views translate before reaching the cache. Each owned
device has its own cache.

Reads hit cached sectors or load from the device. Full-sector writes avoid a
read-before-write and coalesce until explicit flush or eviction. LRU replacement
writes a dirty victim before reusing it. Failed writeback leaves the dirty
entry and its bytes intact for retry; failed reads do not install partial data.
Read-only devices reject writes immediately. Zero-capacity instances safely
bypass caching. Access ages saturate instead of wrapping.

`BlockDevice::flush` drains software buffers through wrappers, including
partition views and borrowed devices. Mutating operations (`persist`, `mkdir`,
`rm`, `rename`, and `sync`) flush before reporting success, retaining clean
entries for later operations. Drop does not silently flush or claim success. ATA
hardware-cache durability, power-loss ordering, journaling, and background
writeback are not provided by this milestone.

`sync` snapshots mounted filenames before persisting, avoiding the previous
recursive VFS-lock acquisition. It retries the shared device cache flush and
reports failure instead of a successful count when persistence fails. The sync
syscall returns its existing generic error sentinel on failure.

The diagnostic-shell `fs` command shows hit/miss, writeback and eviction counts,
plus resident, dirty, and capacity sector counts. It reports no ATA device if
none is detected. VFS RAM-file reads still use their resident node data; this
is a sector buffer cache, not a VM-backed file-page cache. The `blockio`/`iostat`
command shows whether scheduled sector operations used the bounded block-I/O
worker or the direct early-boot fallback.

## Validation (2026-09-07)

- Normal `cargo build` passed.
- Host tests: `rustc --edition 2021 --test tests/buffer_cache.rs -o
  target/buffer-cache-tests.exe`, then `target/buffer-cache-tests.exe`: 4 passed.
  These cover read reuse, write coalescing, dirty LRU eviction, backing-device
  contents, flush retry, failed reads, invalid buffers, bounds, read-only media,
  zero capacity, and partition flush forwarding.
- QEMU test feature: `[BUFFER CACHE] regression tests: PASSED`,
  `[VFS] read/write and path semantics: PASSED`,
  `[BLOCK IO] async completion tests: PASSED`, and
  `[BOOT] ALL VALIDATIONS PASSED`.
- Live normal-kernel QEMU test used a disposable raw FAT32 image with 70,000
  sectors on `-machine pc` (legacy IDE). The current ATA PIO driver did not
  detect the Q35 SATA disk. Production disk contents were not used in the test.
  FAT32 mounted successfully. Two successive `persist /mnt/new.txt` calls
  increased hits from 13 to 34 while misses stayed at 6; `sync` returned after
  persisting two files and brought hits to 76 with misses still at 6. Dirty
  sectors were zero after each persistence operation. After QEMU shutdown,
  direct inspection of the backing FAT32 image confirmed `NEW.TXT` = `cached`.
- Target-specific Clippy still reports the same 16 pre-existing errors outside
  the cache changes. `git diff --check` passed.

## Manual check

On a normal boot with a supported ATA FAT32 disk, run:

```text
fs
mkdir /mnt/docs
write /mnt/docs/a.txt hello
persist /mnt/docs/a.txt
rename /mnt/docs /mnt/archive
cat /mnt/archive/a.txt
stat /mnt/docs/a.txt
rm /mnt/archive/a.txt
rm /mnt/archive
sync
fs
```

Successful persistence prints `persist: written to FAT32`; `cat` should print
`hello`; `stat /mnt/docs/a.txt` should print `stat: not found`; and both `rm`
commands should complete without leaving dirty sectors after `sync`. Repeat
operations should show cache hits increasing on a detected ATA disk. FAT32
mutation currently accepts short 8.3 path components only.

## Default launcher disk setup

`run-stage9.ps1` now uses legacy IDE (`-machine pc`) with the persistent raw
FAT32 image `wovenhat-disk.img`. On first use, `scripts/create-fat32.py` creates
an empty 35,840,000-byte FAT32 volume; existing images are never overwritten.
The image is ignored by Git. The old `wovenhat-data` host folder is preserved,
but its files are not automatically imported into the new image.

This replaces QEMU's `fat:rw:` folder backend, whose generated filesystem did
not satisfy the kernel FAT32 geometry requirements. Changing only the machine
type enabled ATA detection but still left persistence failing.

Validated a disposable copy of the generated image in QEMU: FAT32 mounted,
`persist /mnt/cache.txt` succeeded, repeat persist increased hits from 12 to 32
with misses fixed at 5 and dirty=0, and sync completed successfully.


Follow-up: demand-loaded FAT32 file-page caching, VM-backed file mappings,
physical-page reclamation, swap-backed dirty private refault, and
scheduler-level block-I/O completion are now implemented. Remaining storage work
is true hardware interrupt/DMA completion, richer block drivers, and crash-safe
metadata updates.

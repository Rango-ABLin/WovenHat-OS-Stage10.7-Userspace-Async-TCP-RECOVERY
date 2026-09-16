# Storage Architecture

WovenHat's storage stack is split into a hardware-independent sector layer and
filesystem parsers.

## Block-device contract

`kernel/src/block.rs` defines 512-byte logical sectors through `BlockDevice`:

- `sector_count` reports the addressable media size.
- `read_sector` and `write_sector` require exactly one 512-byte buffer.
- Out-of-range, malformed-buffer, and read-only failures are explicit.
- `RamDisk` provides a deterministic implementation for kernel validation.

The interface is intentionally allocation-free and can be implemented by ATA, AHCI,
NVMe, virtio-blk, or USB mass-storage drivers.

The ATA module implements polling-mode discovery plus sector read/write transport
for the legacy primary-master channel. IDENTIFY and every transfer use bounded
status polling, command-settle delays, and retry loops, reject device faults,
and expose at most the LBA28 address range.
A detected disk is registered as ata0; systems without legacy ATA continue
booting without a block device. Boot validates an actual LBA 0 transfer when
present.

After the scheduler starts, `kernel/src/block_io.rs` wraps primary ATA access in
a bounded completion queue. Scheduled tasks queue eligible sector reads, writes,
and flushes to the `block-io` worker; early boot, interrupts-disabled code, and
worker reentry use direct access. The shell `blockio`/`iostat` command reports
queued, completed, direct, pending, and active operations. This is scheduler-level
asynchronous completion over the existing PIO driver, not yet interrupt-driven
or DMA-backed device completion.

## FAT32 validation

`kernel/src/fat32.rs` provides strict mount metadata validation, short-name lookup
across the root-directory cluster chain, FAT-chain decoding, and bounded file reads. It
validates:

- the 0x55AA boot signature;
- 512-byte sectors;
- power-of-two cluster geometry;
- reserved-sector and FAT counts;
- FAT32-only BPB fields;
- declared media size against the block device;
- FAT/data-region overflow;
- the FAT32 minimum cluster count;
- root-cluster bounds;
- deleted, long-name, and volume-label directory entries;
- free, bad, end-of-chain, and out-of-range FAT entries;
- cyclic, overlong, and prematurely terminated file chains.

The parser reports corrupt and unsupported media without indexing outside a sector.

## VFS mount

The VFS uses a bounded 16-node registry with 64-byte paths and 8 KiB file payloads.
Built-in files retain explicit write permissions. When ata0 contains a FAT32 volume,
boot first checks for a superfloppy filesystem at LBA 0, then the four primary MBR
entries for FAT32 types 0x0B and 0x0C, and finally a CRC-validated GPT behind a
protective MBR. EFI System and Microsoft Basic Data GUIDs are candidates; FAT32 BPB
validation determines the actual format. Partition-relative reads are bounds checked
against both the selected partition and underlying device. See
[partition-table discovery](partition-tables.md). Boot imports up to eight
regular root files as lowercase 8.3 paths below /mnt and marks them writable
when the underlying block device and FAT attributes allow it. It records the resulting
node count for later lifecycle checks. No-disk and non-FAT media are non-fatal outcomes.

## Current boundary

File reads stream across FAT chains up to the file's declared size and the mounted media's cluster count; they may span multiple sectors and clusters without a fixed 64/128-cluster read cap.
Subdirectory **lookup and path resolution** are implemented (`find_in_directory`,
`list_directory`, `resolve_path`, `encode_short_name`). Directory metadata scans
still use a bounded traversal guard to reject corrupt loops early. Boot import walks up to
two directory levels into the VFS under `/mnt`.

Short-name `/mnt` paths now support durable FAT32 create/overwrite through
`persist`, directory creation through `mkdir`, and file/directory deletion and
rename through `rm`, `rename`, `sys_unlink`, and `sys_rename`. Directory rename
moves the on-disk entry and the VFS subtree together, so renaming
`/mnt/home/anthony` to `/mnt/home/user` also makes descendants appear under
`/mnt/home/user/...` instead of leaving orphaned backing paths. Cross-parent
directory moves update `..`; non-empty directory deletion, destination
collisions, read-only FAT entries, invalid FAT 8.3 names, cross-mount rename,
and directory moves into their own descendants are rejected before metadata is
changed where possible. Successful mutations invalidate file pages and flush the
sector cache before returning.

`/mnt` now has an explicit lifecycle state independent of whatever VFS entries
remain in RAM. `sync` walks VFS files below `/mnt`, persists them to FAT32,
flushes ATA, and clears the mount dirty flag only on success. `umount /mnt`
runs that sync, invalidates clean file pages, flushes again, and marks the mount
unavailable; shell and userspace file paths then reject `/mnt` reads, writes,
mkdir, rm, rename, stat, cd, and executable loads until `mount /mnt`, `remount`,
or `rescan` successfully imports the FAT32 volume again. `df /mnt` reports actual
free clusters by scanning the FAT and compares them with FSInfo hints.
`fscheck /mnt` validates the root and bounded directory tree chains, reports file
and directory counts, and flags FSInfo free-count mismatches.

Current FAT32 mutation limits are deliberate: only short 8.3 names are created,
long-filename entries are skipped rather than generated, and there is no journal
or power-loss-atomic metadata transaction. Full directories now grow by linking a
zeroed cluster, and valid FSInfo sectors are maintained as free-space hints.
Flushes make completed operations visible to the device, but they are not a
crash-consistency guarantee.

Next storage increments:

1. Backup-GPT validation and extended/logical MBR partitions.
2. Secondary-channel and slave-device ATA discovery.
3. Hardware interrupt/DMA-backed completion for AHCI, NVMe, or virtio-blk.
4. FAT32 long filename creation and crash-safe metadata
   ordering.
5. Deeper than 2-level import / on-demand path resolution into VFS.

Boot-time self-tests validate block bounds, read-only protection, queued
block-I/O completion, FAT32 geometry, root-chain lookup, missing entries, invalid
signatures, MBR/GPT bounds, GPT CRC corruption, partition-relative I/O,
multi-cluster reads beyond the former fixed cluster cap, end-of-chain handling,
file/directory cycle rejection, durable FAT32 delete/rename primitives,
same-parent and cross-parent directory rename, collision rejection, non-empty
directory delete rejection, freed cluster chains, full-directory cluster
extension, FSInfo free-space hint updates, overwrite rollback after failed data
writes, and VFS disk-backing rename semantics. `scripts/test-storage-qemu.py`
adds a live disposable-ATA regression that creates `/mnt/tmutd/a.txt`, persists
it, renames the directory to `/mnt/tmuta`, verifies the old path is gone and the
new file reads back, then fills real FAT32 directories enough to force growth and
renames a file into a full destination directory, runs `df` and `fscheck`, syncs the mounted VFS view, verifies unmounted `/mnt` paths are rejected, and remounts the volume.

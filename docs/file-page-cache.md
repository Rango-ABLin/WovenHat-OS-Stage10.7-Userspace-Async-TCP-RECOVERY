# Demand-loaded file pages

FAT32 file imports now retain VFS metadata and the original disk path, rather
than eagerly reading the payload. `stat` and directory listing do not fetch
file data. `read` and `read_all` fetch clean 4 KiB pages through a shared,
bounded 16-page cache (64 KiB payload). Repeated reads share cached file pages
across descriptors and avoid FAT traversal and sector-cache calls on hits.

The cache uses LRU replacement. All pages are clean and can be discarded;
failed loads are never marked valid. Partial final pages expose only bytes
inside EOF. FAT32 range reads handle unaligned offsets and cross-sector/cluster
reads without fetching earlier file contents. File-chain traversal is now
bounded by the declared file size and mounted media cluster count instead of a
fixed 64/128-cluster read ceiling.

Storage invalidates all file pages before FAT32 create/overwrite, mkdir,
delete, or rename operations, including operations that later fail. This
deliberately conservative policy prevents stale reads after overwrites, metadata
moves, or cluster reuse. File writes still use VFS RAM storage plus explicit
persistence through the sector write-back cache. Imported files retain their
existing read-only status.

VFS reads copy backing metadata and release the VFS registry lock before disk
I/O. Storage acquires ATA before file-page cache locks; cache loaders never
call VFS. VFS rename updates disk-backing paths for the renamed node and all
renamed descendants, and the storage layer now performs the matching on-disk
FAT32 rename for `/mnt` short-name paths.

`fs` displays a separate `file pages:` line with hits, misses, evictions,
resident pages, and capacity. The cache uses fixed kernel storage for its page
slots. This is not yet VM frame allocation/reclamation or file-backed mmap;
the current mmap syscall remains anonymous. Existing fixed RAM-node buffers
remain reserved, even for disk-backed nodes, but no duplicate disk payload is
loaded into them. Removing that reservation is a separate VFS storage change.

## Manual test

Use a FAT32 file that survived a QEMU restart (for example `/mnt/cache.txt`).
After restarting through `run-stage9.ps1`, run:

```text
fs
cat /mnt/cache.txt
fs
cat /mnt/cache.txt
fs
```

At boot, file pages should show resident=0. For a small file, the first read
adds one miss and one resident page. The second read adds a hit with no further
misses or sector-cache activity. A newly created RAM file does not exercise
disk page loading until it is persisted and imported from disk.

## Validation — 2026-09-07

- `cargo build` passed.
- `rustc --edition 2021 --test tests/page_cache.rs -o target/page-cache-tests.exe`
  followed by `target/page-cache-tests.exe`: 5 passed. Covers page boundaries,
  EOF, cache hits, LRU replacement, failed loads, invalidation, existing FAT32
  regression cases, integrated FAT32 range/page-cache reads through 64 KiB,
  FAT32 streaming reads beyond the former fixed cluster cap, and FAT32
  delete/rename mutation regressions.
- QEMU reported buffer cache, file pages, and VFS regression tests PASSED.
  As before, the broader suite subsequently panicked because the scheduler was
  not initialized. Test-feature build used the previously documented unused-code
  and unused-import warning allowances.
- Normal-kernel QEMU with a disposable FAT32 disk: zero resident pages on boot;
  first `cat` produced one miss, second `cat` produced one hit without further
  sector activity. Replacing and persisting the file, then importing it again,
  returned the new contents and produced a new page miss. Reading after VFS
  rename also returned the correct contents.
- Target-specific Clippy still reports the same 16 existing errors elsewhere;
  no diagnostics in page_cache.rs, storage.rs, or vfs.rs. `git diff --check` passed.


Follow-up: read-only private file snapshots are now available via syscall 58;
see `file-mmap.md`. Syscalls 60/61 now load private pages on first access.
FAT32 faults use the file-page cache; shared writable mappings and msync are now available through 62/63.

Physical mapping aliases now use file_frames.rs with reference-counted frames,
shared write coherence, and unpinned LRU reclamation. The original FAT32 byte
cache remains below the backing loader. See file-mmap.md for current limits.

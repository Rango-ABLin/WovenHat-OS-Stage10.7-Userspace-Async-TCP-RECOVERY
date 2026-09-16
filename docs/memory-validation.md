# Memory milestone validation - 2026-09-08

| Area | Implemented and checked |
| --- | --- |
| Boot baseline | Scheduler initialized before pipe tests; mmap arena no longer overlaps the stack; corrected Ring-3 pointer fixture; full boot suite passes |
| Mapping lifetime | Referenced RAM and disk inodes survive unlink/replacement; shared truncation zeroes resident tails and absent truncated reads fail |
| Physical cache | Mapping aliases share physical cache frames; private writes split through COW; VFS shared read/write coherence; FAT32 file reads stream past the former fixed cluster cap |
| Reclamation | Unpinned LRU eviction, allocation-pressure reclaim/retry, live mapped-page eviction/refault, dirty private swap-slot refault, disk-backed swap policy through queued block I/O, pinned-frame protection, exact cleanup; reference-table exhaustion and overflow rollback |
| Shared mappings | Syscall 62, shared aliases across fork, syscall 63 msync, RAM writeback and durable ATA/FAT32 flush |
| FAT32 mutations | Durable short-name `/mnt` mkdir/persist/delete/rename, full-directory cluster extension, FSInfo free-space hint updates, overwrite rollback, non-empty delete rejection, destination collision rejection, cluster-chain freeing, and VFS backing-path subtree updates |
| Fault I/O and pager | No process/physical-cache lock across backing reads; CPU IRQs remain enabled with a BSP preemption guard; lazy file faults are queued to a bounded pager worker; eligible ATA sector reads/writes/flushes are queued to a bounded block-I/O worker; live timer IRQ, state-restoration, and Ring-3 mmap fault tests |

Validation performed:

- `python scripts/test-memory-qemu.py`: PASS, QEMU exit 33,
  `[BLOCK IO] async completion tests: PASSED`,
  `[BLOCK IO] worker completion: PASSED`, and
  `[BOOT] ALL VALIDATIONS PASSED`, with two CPUs advertised but BSP scheduling.
- The QEMU boot suite runs a real Ring-3 `mmaptest` after user-process
  reaping/reclamation checks; it reported `[PAGER] Ring-3 faults queued=8
  completed=8: PASSED`.
- Lazy mmap self-tests evict clean and dirty private resident pages, verify the
  PTEs are absent again, check frame reclamation, and refault from either
  backing file data or a swap slot. The swap policy self-test writes pages
  through a block-device-backed store and validates stale-handle rejection,
  refcounting, slot reuse, and RAM fallback.
- The block-I/O self-tests validate queue write/read completion, bounded queue
  exhaustion, failure propagation, and a runtime worker wake/completion path.
- Host bounds test: 1 passed; buffer-cache tests: 4 passed; file-page-cache/FAT32
  tests: 5 passed, including FAT32 streaming and mutation regressions.
- Normal QEMU: repeated `mmaptest` reports `FILE MMAP: PASS`.
- Disposable ATA disk: `msynctest` reports `MSYNC DISK: PASS`, checks unlink of
  a disk-backed mapped file followed by persisted replacement, and verifies
  both saved fixtures after a fresh boot.
- `python scripts/test-storage-qemu.py`: PASS with a disposable IDE FAT32 disk;
  serial output reported `[STORAGE MUTATION] live FAT32 rename/delete/growth: PASSED`.
- `git diff --check`: passed. Clippy still reports existing repository issues;
  this is not a claim of a clean Clippy baseline.

Limits: live mapped-page eviction can discard clean private lazy pages, write back
shared pages, and save dirty private lazy pages into bounded swap slots before
refault. Swap prefers a configured reserved ATA sector range and falls back to
kernel RAM when no safe disk area exists. The pager worker parks faulting user
contexts and performs page population on a scheduler task. Eligible ATA sector
operations after scheduler startup queue to the block-I/O worker, but the
underlying legacy ATA driver is still PIO-polled and early boot still uses direct
access. No application-processor scheduling is implemented. Before enabling SMP,
the BSP preemption guard must become per-CPU, and mapping publication/unmap
synchronization and remote TLB shootdowns must be added.
See `file-mmap.md` for ABI and detailed semantics.

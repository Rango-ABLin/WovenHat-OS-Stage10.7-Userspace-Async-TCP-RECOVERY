# Stage 6 heap capacity and mapping audit — 2026-09-17

The previous kernel heap eagerly mapped 256 KiB, tracked at most 512 live
allocations, and kept at most 128 free intervals. Reusing an aligned free
block discarded its leading padding. An allocation attempted after the live
table filled could consume a free block before failing. Deallocation silently
dropped an interval when the free table filled. These paths could lose usable
heap memory long before physical RAM was exhausted.

The heap now maps one sixteenth of remaining physical RAM at boot, clamped to
256 KiB–8 MiB, and tracks 2,048 live allocations. The current 256 MiB QEMU
boot reports `[HEAP] mapped=8388608 bytes`. Allocation checks metadata
capacity before removing a free block. Each live entry retains its complete
reserved span, including alignment padding; deallocation coalesces adjacent
intervals and rewinds the bump pointer when the highest span becomes free.
The free metadata table holds 2,049 intervals, the maximum for a coalesced
contiguous arena with at most 2,048 live allocations. Reported free bytes
include both holes and the untouched tail.

`paging::map_range` now releases frames and unmaps pages it added when a
later page in the same call fails. A boot probe pre-maps the second page of a
two-page range, verifies that the combined mapping fails with `AlreadyMapped`,
then checks the first page was rolled back while the second stayed mapped.
The ordinary map/write/unmap boot probe now returns its test frame as well.
The reclaimed-frame table holds 4,096 entries, enough to return every data
frame from the maximum 8 MiB heap map plus bootstrap page-table and probe
frames during this early-boot transaction. General runtime frame reclamation
remains bounded by that table.
The rollback probe covers leaf mappings and their data frames; it does not
prove that a failed page-table allocation returns every intermediate table
frame. That path remains part of the broader memory-allocator audit.

`tests/heap.rs` runs three production-state tests for alignment recovery,
fragmented coalescing, and live-table exhaustion without free-space loss.
The QEMU memory gate exercises the RAM-scaled heap, a 300 KiB allocation,
and the mapping rollback probe. The final source passed
`python scripts/test-release.py`: warning-denying kernel/host Clippy, host
tests, 1/2/4-CPU memory/storage/network, legacy PIC, 4-CPU release suites,
release build, and shell/PS/2 smoke. Dedicated twice-cycled 2/4-CPU AP
hotplug also passed. Logs are retained in `target/release-validation` and the
dated `audit-artifacts/stage6-hotplug-{2,4}cpu-*` directories.

This is still a bounded heap. Runtime growth above the eagerly mapped size
cannot call the rank-10 pager from arbitrary allocation contexts that may
already hold higher-ranked locks. The fixed live-object table, allocator
performance under broad multicore workloads, and low-memory hardware
qualification remain open.

# Stages 1–12 production completion audit

This controlling gap list prevents bounded acceptance markers from being
mistaken for full production completion.

| Area | Foundation present | Production gaps still open |
| --- | --- | --- |
| Stages 1–5 | Boot, memory, paging, scheduler, VFS, userspace, storage; 1/2/4-CPU acceptance; bounded Unicode long-filename create/lookup/delete/rename, NFC name normalization, named listing, mounted import listing, directory growth, bounded VFS uid/gid/mode enforcement, WMD1/WMD2 ownership metadata persistence/import and durable file-intent recovery | Non-QEMU hardware qualification/drivers, DMA storage completion, metadata-table capacity beyond the fixed ABI, full data rollback/checksum replay, unclean-shutdown hardware qualification |
| Stage 6 | Bounded SMP, TLB shootdowns, 1–4 CPU tests, ACPI SRAT CPU/memory-domain discovery, domain-local placement/rebalancing/allocation, x2APIC MSR path, migratable audited I/O workers, QEMU-qualified non-contiguous AP offline/re-online control, ranked interrupt-safe locks for scheduler/process/paging/COW/frame-allocation/file-frame cache/VFS/heap/swap/FAT32 clean-page cache, async/completion/worker, pipe, IPC, WovenGuard lineage, device/keyboard, journal, mount-record and key-vault domains, generation-tagged VFS/pipe handles, unlocked/revalidated disk-backed VFS reads and materialization, allocator-reserved contiguous VirtIO DMA arena | General multicore userspace, unrestricted concurrent I/O/DMA throughput, multi-node NUMA qualification, non-contiguous hotplug hardware qualification, APIC-ID-above-255 hardware coverage, cross-layer FAT32 mutation transactions and stress qualification, bounded heap capacity, broader cross-layer lock stress/priority inheritance |
| Stages 7–9 | Isolation, IPC, capability delegation/revocation, WovenGuard, ELF W^X, formal threat model | Dynamic libc/linking, complete signals, scheduler-backed threads |
| Stage 10 | TCP, completion ports, timer/event foundations, Ring-3 timer/event gate, unified completion path | Broader production driver and hardware qualification |
| Stage 11 | Process/thread state, notifications, `libwoven`, loader checks, production ASLR | Real scheduler-backed user threads, dynamic relocations, shared libraries, loader TLS, RELRO |
| Stage 12 | Typed VFS, metadata, checksums, snapshot/mount boundaries, xattrs and restore records, ChaCha20-Poly1305 envelope and bounded revocable key vault | Native WovenFS, journaling/COW disk replay, full data rollback, measured/persistent key provisioning and encrypted-volume mount integration |

For every open item we will add implementation, focused host tests, 1/2/4 CPU
QEMU tests, and an audit entry. A stage is production-complete only when every
roadmap requirement has passing evidence or an explicitly accepted design.

Stage 6 lock follow-up: terminal rendering now uses a ranked, IRQ-live local
preemption guard; shell cwd state uses a short ranked IRQ mutex. The remaining
lock and priority-inheritance items in the table refer to broader cross-layer
paths and scheduler behavior, not the terminal or shell metadata guards.
The socket runtime and VirtIO transport now use explicit ranks 20 and 30;
every current kernel `IrqMutex` construction has a declared rank. Broader
lock-path stress and priority inheritance remain open.

The heap follow-up scales eager mapping with RAM up to 8 MiB, raises live
metadata capacity to 2,048, preserves alignment padding, coalesces frees, and
rolls back partial kernel mappings. Runtime growth beyond the pre-mapped region
and removal of the fixed live-object table remain open Stage 6 work.

The Stage 6 non-contiguous hotplug software gate now parks CPU 1 of four while
CPU 3 continues to execute work and acknowledge TLB shootdowns. This changes
the foundation evidence in the Stage 6 row; physical non-contiguous hotplug
qualification remains open, as do high APIC-ID and multi-node NUMA hardware
gates. Failed scheduler evacuation is now atomic with respect to task moves.
Deterministic AP rejection and unclaimed-timeout recovery now pass the 2/4-CPU
QEMU hotplug gate; claimed-transition hardware-fault and concurrent workload
stress remain open.

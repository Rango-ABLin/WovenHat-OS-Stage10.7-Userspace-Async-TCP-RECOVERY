# WovenHat OS — Full Audited Source Package

Date: 2026-09-10
Baseline: WovenHat-wovenhat-os(8).zip
Overlay: Stage 7.1 CPU Affinity Foundation, based on Stage 7.0.11 self-audited stabilization

## What this package is
This is the complete WovenHat OS source tree with the latest Stage 7.1 scheduler/SMP files integrated into the full repository. It is intended to replace using the earlier partial patch ZIPs separately.

## Integrated audited changes
- Stage 7.0 scheduler handoff stabilization via `TaskState::Switching`.
- IRQ-safe process-table locking discipline.
- Atomic pager block/wake scheduling handshake.
- Runnable-load accounting includes Switching tasks.
- Stage 7.1 per-task non-zero CPU affinity masks.
- Affinity-aware scheduling, explicit migration, automatic rebalancing, and ready-task affinity changes.
- Userspace remains pinned to CPU0 by design.
- Scheduler affinity invariant validation.
- Stage 7.1 affinity regression and 1/2/4 CPU acceptance script.

## Static audit performed while packaging
- Searched kernel/src, src, tests and scripts for TODO/FIXME/todo!/unimplemented!/panic! markers: none found by the packaging scan.
- Verified direct `PROCESS_TABLE.lock()` appears only inside the IRQ-safe wrapper implementation in `kernel/src/task.rs`.
- Reviewed spawn sites: SMP test jobs use `spawn_on` / `spawn_migratable_on` according to the documented safety contract.
- Removed transient `.git`, `target`, OVMF variable state, disk image, QEMU debug log, and accumulated run-log directories from the distribution package.

## Important verification status
The packaging environment did not contain `rustc` or `cargo`, so build, clippy, host-test, and QEMU execution could not be rerun here. This package therefore means **source-integrated and statically audited**, not independently runtime-certified.

Run the following on the WovenHat Windows development machine before declaring Stage 7.1 frozen:

```powershell
cargo build
cargo clippy -p wovenhat-kernel -- -D warnings
cargo test
.\run-stage7-1-acceptance.ps1
```

The Stage 7.1 acceptance script is expected to prove:
- build + clippy + host tests pass;
- memory tests: 20/20 on 1 CPU, 100/100 on 2 CPUs, 50/50 on 4 CPUs;
- network tests pass on 1/2/4 CPUs;
- zero panic, page fault, lockup, or timeout.

## Known deferred technical debt
Per the Stage 7 self-audit, signal/kill termination of another task still needs to evolve into a scheduler-owned request/acknowledgement protocol before unrestricted userspace migration or true parallel userspace is enabled. Legacy VFS/storage/network/GUI/allocator-facing service domains remain BSP-owned until separately audited for SMP/preemption safety.

## Next milestone
After the acceptance matrix passes cleanly, Stage 7.1 can be frozen and development can proceed to the next SMP maturity step without weakening the current userspace CPU0 pinning or service-domain ownership rules.

## Stage 7.1.1 correction after runtime acceptance

A 2-CPU runtime panic exposed an AP-online/affinity publication race in Stage 7.1.
The source has been corrected in `kernel/src/smp.rs` and `kernel/src/task.rs` without
weakening the affinity invariant. Full runtime acceptance must be rerun before Stage 7.1
is considered frozen.

## Stage 7.1.4 diagnostic update
Stage 7.1 freeze remains BLOCKED. A rare pager-yield stall reproduced on a
1-CPU run, so Stage 7.1.3 AP idle contention is not the root cause. Stage 7.1.4
adds targeted yield handoff tracing and explicit block/sleep replacement
invariant assertions. Do not advance to Stage 7.2 until the stall is localized,
fixed, and the full 1/2/4 CPU plus network acceptance matrix passes.

## Stage 7.5 candidate
Built from validated Stage 7.4 only. Adds per-CPU file-I/O preemption depth and
pins the existing file-backed mmap/pager acceptance workload to an AP on SMP.
General userspace balancing and concurrent service execution remain deferred.

# WovenHat OS Stage 7 stabilization self-audit

## Scope

This audit reviews the multicore scheduler, task ownership transitions, migration/rebalancing,
process-table locking, pager block/wake ordering, and the SMP acceptance tests used to decide
whether Stage 7.0 may be frozen.

## Correctness rules now enforced by the design

1. A physically executing task must never be published as ordinary `Ready` before its stack
   context is saved. The `Switching` state closes this hand-off window.
2. A `Switching` task is not migratable or schedulable by another CPU.
3. Runnable-load accounting treats `Switching` as physically running, because it still consumes
   its source CPU until `wovenhat_context_switch` completes.
4. The file-fault requester is made `Blocked` and the pager is made `Ready` under one scheduler
   critical section, closing the lost-wakeup window.
5. Process-table access goes through `process_table_lock()`, which disables local interrupts
   while the non-reentrant spin mutex is held and restores IF only after unlock.
6. Nested scheduler/process-table paths use the order `SCHEDULER -> PROCESS_TABLE`. Paths that
   need the opposite data flow must release the process guard before taking the scheduler.
7. SMP regression tests prove durable outcomes (migration counters, execution masks, completion)
   rather than requiring exact instantaneous runnable-load snapshots while other CPUs are live.

## Audit findings addressed

### A. Scheduler hand-off publication race — fixed

The old scheduler could mark an outgoing, still-executing task `Ready` before the assembly stack
switch. Another CPU could then migrate or select that task. `TaskState::Switching` now represents
this physical hand-off and is finalized only after the incoming context is actually executing.

### B. Pager lost wakeup — fixed

The old pager sequence could wake the requester before the requester had become `Blocked`.
Blocking the requester and publishing the pager readiness under the scheduler lock makes the
handshake atomic with respect to scheduler state.

### C. IRQ-unsafe process table accesses — fixed

All direct process-table accesses now route through the IRQ-safe wrapper; the sole raw
`PROCESS_TABLE.lock()` is inside that wrapper itself.

### D. Rebalance acceptance used a racy snapshot — fixed

The regression no longer asserts an exact max/min load shape while destination CPUs are actively
turning Ready probes into Running probes. It checks durable evidence that work migrated and ran.

### E. Switching tasks were omitted from load accounting — fixed in this audit

After introducing `TaskState::Switching`, `run_loads()` still counted only `Ready` and `Running`.
A switching task is physically executing, so the balancer could transiently underestimate a CPU by
one runnable task. `Switching` is now counted in the running component of `CpuRunLoad`.

## Non-blocking technical debt found

The current signal/kill implementation can mark another task `Dead` directly. Before unrestricted
userspace migration or true parallel userspace is enabled, signal termination should become a
scheduler-owned termination request/acknowledgement protocol so a running task is never destroyed
by metadata mutation alone. This is important, but it is not required to prove the current Stage 7.0
restricted-kernel-job SMP baseline because userspace remains CPU0-owned.

Several legacy service domains (VFS/storage/network/GUI/allocator-facing code) remain intentionally
BSP-owned. Do not relax the documented `spawn_on` / `spawn_migratable_on` safety contract until
those domains receive their own SMP/preemption audits.

## Freeze gate

Stage 7.0 is frozen only after the clean tree passes:

- `cargo build`
- `cargo clippy -p wovenhat-kernel -- -D warnings`
- `cargo test`
- 100/100 two-CPU memory/boot runs
- memory/boot acceptance on 1, 2, and 4 CPUs
- network acceptance on 1, 2, and 4 CPUs
- no panic, page fault, lockup, or timeout in those runs

After that gate, remove temporary `[PROC-DIAG]` and `[PAGER-DIAG]` logging, rerun the same 1/2/4
clean acceptance matrix, commit the frozen baseline, and proceed to Stage 7.1 CPU affinity.

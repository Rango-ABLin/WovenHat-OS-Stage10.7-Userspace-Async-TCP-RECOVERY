WovenHat OS Stage 7.0.11 - audited stabilization candidate
===========================================================

This package is based on the current Stage 7.0.10 integrated source and keeps:
- scheduler hand-off TaskState::Switching protection;
- atomic pager block/wake publication;
- IRQ-safe process-table wrapper;
- concurrency-correct runnable-load/rebalance regression tests.

Additional self-audit correction:
- CpuRunLoad now counts TaskState::Switching as physically running. During the
  hand-off window the task still occupies its source CPU, so excluding it could
  transiently undercount source load and mislead placement/rebalancing.

Audit result:
- Only one raw PROCESS_TABLE.lock() remains, inside process_table_lock().
- Restricted migration still requires TaskState::Ready.
- Switching tasks cannot be selected by prepare_switch or migrate_ready_task.
- Pager requester Blocking + pager Ready publication remains one scheduler
  critical section.
- Exact instantaneous SMP load-balance assertions remain removed.

Also included:
- docs/STAGE7-SELF-AUDIT.md
- updated docs/smp-lock-audit.md addendum
- run-stage7-freeze-gate.ps1, which performs build/clippy/tests, 100 two-CPU
  memory runs, then the 1/2/4 CPU memory and network matrices.

Important audit debt (not a Stage 7.0 blocker while userspace remains CPU0):
kill/signal termination still mutates target task state directly. Before true
parallel/migratable userspace, replace that with scheduler-owned termination
request/acknowledgement semantics.

Validation must be performed on the Windows development host because this
container does not have the WovenHat Rust/QEMU toolchain.

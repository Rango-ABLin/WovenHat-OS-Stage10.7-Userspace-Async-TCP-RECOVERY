WovenHat OS Stage 7.1.4 — Scheduler Handoff Diagnostic
Date: 2026-09-10

Purpose
-------
This is a diagnostic build for the rare pager acceptance stall seen on both
1-CPU and 2-CPU QEMU runs. It is NOT the Stage 7.1 freeze release.

Changes
-------
1. Adds a pager-window-only yield trace in kernel/src/task.rs.
2. The trace records phases A-H around yield_now():
   A enter
   B interrupts disabled
   C scheduler lock wait
   D scheduler lock acquired + outgoing/current task identity
   E prepare_switch completed + selected task identity
   F context switch entered
   G context switch returned (or no switch)
   H yield exit
3. kernel/src/main.rs enables the trace only while the pager mmaptest
   acceptance loop is running, then disables it after the child exits.
4. block_current() and sleep_current() now assert if prepare_switch() returns
   no replacement after the current task was marked Blocked/Sleeping.

Interpretation
--------------
If a failed serial log ends at phase C, investigate scheduler lock acquisition.
If it ends at phase D, investigate prepare_switch().
If it ends at phase F, the selected incoming task ran but the yielding task did
not return; use selected_id/name/slot and next_rsp from phases E/F.
If it reaches G but not H, investigate interrupt restoration/post-switch path.
If an invariant panic says a blocked/sleeping task has no runnable replacement,
the scheduler metadata/physical-execution divergence has been caught directly.

Validation order
----------------
Run:
  cargo build
  cargo clippy -p wovenhat-kernel -- -D warnings
  cargo test

Then run focused 1-CPU tests first. Stop on the first timeout or panic and
preserve target/memory-regression-1-debug/serial.log before rerunning QEMU.

Important
---------
This build preserves the Stage 7.1 Switching handoff, affinity invariants,
Stage 7.1.1 AP online publication fix, Stage 7.1.2 fork first-dispatch
trampoline, and Stage 7.1.3 idle HLT change. The added assertions are
intentional diagnostic guards, not yet the final production scheduling policy.

# Stage 8.1 preservation fix — Stage 7.4 Ready-window race

## Observed failure
The Stage 8.1 runtime probe passed, block I/O passed, Stage 7.3 passed, and the Stage 7.4 explicit migration probe passed. The next Stage 7.4 hard-affinity setup intermittently failed before execution.

## Root cause
`spawn_migratable_user_probe()` publishes a migratable `/bin/true` task in `Ready` state owned by CPU0. The Stage 7.4 proof then performed a second API call (`migrate_ready_task()` or `set_ready_task_affinity()`) after returning to interrupt-enabled BSP execution. A timer interrupt in that narrow interval can dispatch the tiny CPU0-owned probe. The second API then correctly returns `NotReady` (or `UnknownTask` after very fast retirement), producing a false preservation failure.

This is a test-observation/setup race. The scheduler's Ready-only semantics are correct and are not weakened.

## Fix
Both Stage 7.4 setup sequences now keep local BSP interrupts disabled across:

1. publishing the new CPU0-owned Ready probe;
2. inspecting its initial affinity where applicable; and
3. applying the explicit Ready migration or hard-affinity transfer.

The existing migration/affinity APIs still take the global scheduler lock and still reject non-Ready tasks. Once ownership has moved to an AP, the local interrupt boundary ends and normal SMP execution continues.

## Scope
Production scheduler, task states, IPC, SMP, IPI, process lifecycle, and affinity semantics are unchanged. Only the Stage 7.4 runtime proof in `kernel/src/main.rs` is hardened against local timer preemption during its intentionally two-step Ready-state setup.

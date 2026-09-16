# WovenHat OS Stage 7.1 — CPU Affinity Foundation Audit

## Scope
Stage 7.1 adds a hard CPU-affinity invariant on top of the stabilized Stage 7.0 scheduler. It deliberately does not enable userspace migration.

## Implemented contract
- Every live task has a non-zero `affinity_mask`.
- Every task's owner CPU must be online and inside its affinity mask.
- BSP kernel task and BSP idle task are CPU0-only.
- AP idle tasks are pinned to their own CPU.
- Userspace tasks and fork children remain CPU0-only.
- `spawn_on(cpu, ...)` creates a pinned single-CPU mask.
- `spawn_migratable_on(cpu, ...)` creates an all-online-CPU mask.
- `migrate_ready_task()` rejects offline CPUs, pinned tasks, userspace tasks, non-Ready tasks, and destinations outside the affinity mask.
- `set_ready_task_affinity()` is restricted to Ready, migratable kernel tasks. It rejects zero masks and offline bits. If the current owner is excluded, ownership moves atomically to the lowest allowed CPU while the scheduler lock is held.
- Scheduler selection requires both `task.cpu == current_cpu` and the current CPU bit in `affinity_mask`.
- Automatic rebalancing evaluates only destinations allowed by each task's affinity mask.
- `Switching` tasks remain non-migratable and are counted as physically runnable during handoff.

## New public API
- `task_affinity(TaskId) -> Option<usize>`
- `set_ready_task_affinity(TaskId, usize) -> Result<(), AffinityError>`
- `MigrationError::AffinityDenied`
- `AffinityError::{UnknownTask, InvalidMask, OfflineCpu, Pinned, NotReady, Userspace}`

## Runtime invariant checker
The scheduler now validates affinity invariants before every scheduling decision and after ownership-changing operations. This intentionally converts silent affinity corruption into an immediate, localized kernel assertion.

## Stage 7.1 regression proof
`affinity_foundation_test()` verifies:
1. zero masks are rejected;
2. offline CPU bits are rejected;
3. a CPU0-only mask cannot be escaped by automatic rebalancing;
4. explicit migration outside the mask returns `AffinityDenied`;
5. restricting a Ready migratable task to CPU1 moves ownership to CPU1;
6. the task actually executes on the expected CPU.

Existing migration stress, runnable-load, automatic-rebalance, IPI, TLB shootdown and preemption tests continue to run after the new affinity test.

## Explicit non-goals
These are intentionally deferred so Stage 7.1 stays auditable:
- userspace migration;
- process-level affinity syscalls;
- Hotplug transitions beyond the bounded Stage 6 contiguous-prefix
  offline/re-online control;
- NUMA/topology-aware placement;
- scheduler classes/cgroups/cpusets;
- migration of Running, Switching, Blocked or Sleeping tasks.

## Gate to proceed to Stage 7.2
Required: build, clippy with warnings denied, host tests, repeated 1/2/4 CPU memory runs, and 1/2/4 CPU network acceptance with zero panics/timeouts.

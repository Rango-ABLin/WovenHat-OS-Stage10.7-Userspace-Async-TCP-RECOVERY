# Stage 7.6 Scheduler State Semantics Correction

## Regression observed

The first race fix introduced `has_started` and rejected Stage 7.4 explicit
Ready migration/affinity operations once that flag was set. The 2-CPU
preservation test then failed at the Stage 7.4 hard-affinity transfer.

## Root semantic error

`has_started` mixed two independent concepts:

1. **history** — whether the scheduler has ever dispatched the task; and
2. **permission** — whether a particular migration API is allowed to move it.

That made a Stage 7.6 automatic-balancing policy silently redefine Stage 7.4's
validated explicit API contract.

## Correction

`TaskControlBlock::ever_dispatched` is now pure execution-history metadata.
It is set under the scheduler lock at `Ready -> Running`.

Only `Scheduler::rebalance_one()` consumes this bit for Ring-3 tasks. Thus the
new Stage 7.6 automatic policy may move a Ring-3 task only before its first
scheduler dispatch.

`migrate_ready_task()` and `set_ready_task_affinity()` again use the Stage 7.4
contract only: live task, explicitly migratable, currently `Ready`, target
online, and affinity constraints satisfied.

## Preserved invariants

- No Running or Switching task is migrated.
- Scheduler ownership changes remain under the global scheduler lock.
- Affinity remains a hard constraint.
- Legacy CPU0-owned userspace remains unchanged.
- Pinned Stage 7.3 userspace remains pinned.
- Stage 7.6 automatic Ring-3 rebalancing remains bounded to never-dispatched
  tasks.

## Acceptance requirement

The full Stage 7.6 acceptance suite must pass before freeze/tag.

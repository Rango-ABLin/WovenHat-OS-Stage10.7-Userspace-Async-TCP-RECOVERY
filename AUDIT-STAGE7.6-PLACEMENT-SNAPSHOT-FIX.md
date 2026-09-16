# Stage 7.6 — race-free multicore placement proof

## Evidence
An intermittent SMP run reached:
- `[S7.6] automatic Ready Ring-3 rebalancing: PASSED`
- `[S7.6] least-loaded Ring-3 placement: FAILED owner=1`

On a 2-CPU run, `owner=1` proves least-loaded AP placement succeeded. The old
acceptance branch could therefore only be failing because `task_affinity(task_id)`
returned something other than the expected mask.

## Root cause
`spawn_multicore_user_process()` installs the Ready TCB under the scheduler lock,
validates affinity invariants, releases the lock, sends a remote reschedule IPI,
and returns. The `/bin/true` probe can execute and retire on the AP before the
caller subsequently calls `task_affinity(task_id)`. Once the task is reaped, the
live-task lookup returns `None`. The test confused lifetime with placement.

## Correction
The spawn API now returns `MulticoreSpawnPlacement`, an immutable snapshot copied
from the installed TCB under the scheduler lock after affinity validation:
- `owner_cpu`
- `affinity_mask`

The Stage 7.6 acceptance test validates that snapshot rather than performing a
racy post-spawn live-task lookup. Scheduler policy, migration policy, affinity
semantics, and task lifetime are unchanged.

This does not weaken the test: it moves observation into the synchronization
domain where the property is established.

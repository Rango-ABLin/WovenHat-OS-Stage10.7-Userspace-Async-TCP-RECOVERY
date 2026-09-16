# WovenHat OS Stage 7.5 Audit — Multicore Pager & File-I/O Ownership

## Validated baseline
Stage 7.4 is the only implementation baseline for this candidate. The earlier
combined "Stage 7 Complete" experiments are not used as a source tree.

## Problem being solved
`file_fault_io()` intentionally enables interrupts while a task performs an
I/O operation. Before Stage 7.5, `FILE_IO_DEPTH` was a single global counter
and `preempt_from_interrupt()` honored it only on CPU0. That matched the old
BSP-only file-fault execution model but is not correct once a faulting Ring-3
task runs on an AP: the AP itself could be preempted while holding an I/O lock,
while CPU0 could be unnecessarily prevented from scheduling.

## Correctness change
`FILE_IO_DEPTH` is now an array indexed by logical CPU. The function disables
local interrupts before capturing the current CPU index and incrementing the
local depth. Interrupts may then be enabled for the I/O operation. On return,
interrupts are disabled again and the same CPU's depth is decremented.

A task cannot be involuntarily migrated while this region is active because
local timer preemption checks the same per-CPU depth and returns without a
scheduler switch. This makes the CPU index stable for the enter/leave pair.
Other CPUs remain independently schedulable.

## Pager execution proof
The existing `/bin/mmaptest` workload is loaded exactly as before. On SMP, the
process is spawned through the already-validated Stage 7.3 pinned-user API on
the highest online AP. Its lazy file-backed page fault therefore originates on
an AP. The existing Stage 7.1.5 block/wake transaction marks the requester
Blocked and makes the BSP pager Ready under one scheduler critical section,
then issues the existing reschedule request to the pager CPU. The requester is
resumed on its pinned AP after completion and the original user instruction is
retried by `iretq`.

## Locking / ownership boundary
No process-table/cache lock is intentionally held across `file_fault_io()`.
The Stage 7.1 lock order (`SCHEDULER -> PROCESS_TABLE`) remains unchanged.
The pager remains one BSP-owned worker in Stage 7.5; this milestone changes the
location of the faulting userspace process, not pager-worker topology.

## Explicit non-goals
Stage 7.5 does not make normal userspace automatically migratable and does not
claim arbitrary concurrent filesystem/network service execution. Those are
separate follow-on milestones after this AP file-fault path is accepted.

## Acceptance gate
The gate preserves the validated Stage 7.2/7.3/7.4 markers, then requires the
new Stage 7.5 AP pager marker over repeated 2-CPU and 4-CPU QEMU runs, plus
network regressions at 1/2/4 CPUs.

Stage 7.5 is not considered frozen until the script prints:

`=== STAGE 7.5 ACCEPTANCE: PASS ===`

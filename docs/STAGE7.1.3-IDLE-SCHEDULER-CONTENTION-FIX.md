# Stage 7.1.3 — Idle Scheduler Contention Fix

## Symptom

After the Stage 7.1.1 AP-online affinity fix and Stage 7.1.2 fork hand-off fix,
1-CPU stress was stable, while a 2-CPU acceptance run produced one rare timeout
(1 failure in 100).  The failing serial trace stopped at:

    [PAGER-DIAG] yield: BEGIN iteration=5

There was no panic, page fault, or exception afterward.

## Root cause

`idle_task()` called `yield_now()` continuously. On an otherwise idle AP this
created a hot loop that repeatedly acquired the single global `SCHEDULER`
spinlock even though there was no runnable work for that CPU.

CPU0's `yield_now()` disables interrupts before taking the same lock. With an
unfair spinning mutex and two concurrently scheduled virtual CPUs, the idle AP
could rarely reacquire the lock often enough to starve CPU0 until the QEMU test
harness timed out.

This explains the observed shape:

- 1 CPU: stable, because there is no competing AP idle loop.
- 2 CPUs: overwhelmingly successful, with a rare no-panic stall.
- Last serial marker: immediately before a blocking scheduler-lock acquisition.

## Fix

The idle task now:

1. checks `preemption_point()` for already-pending work;
2. executes `hlt()` instead of calling `yield_now()` continuously.

This is the normal idle-CPU model: an idle CPU sleeps until an interrupt rather
than polling the scheduler lock.

WovenHat already has the required wakeup mechanisms:

- LAPIC timer interrupts call `task::tick()` and `task::preempt_from_interrupt()`;
- the reschedule IPI handler calls `task::request_reschedule()` followed by
  `task::preempt_from_interrupt()`.

Therefore a halted idle CPU is awakened and scheduled promptly when work becomes
ready, without creating global-lock contention while idle.

## Invariants preserved

This change does not weaken:

- `TaskState::Switching` hand-off protection;
- the nested scheduler hand-off assertion;
- CPU affinity validation;
- userspace CPU0 pinning;
- ready-task migration rules;
- reschedule IPI behavior.

## Verification gate

Recommended focused validation before the full Stage 7.1 acceptance run:

    1 CPU x 30
    2 CPU x 100
    4 CPU x 30

Then run the complete `run-stage7-1-acceptance.ps1` gate.

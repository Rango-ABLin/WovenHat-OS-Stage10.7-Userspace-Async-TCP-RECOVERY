# Stage 7.1.5 — Scheduler/Pager Correctness Repair

## Why this release exists

Stage 7.1.3 and 7.1.4 exposed a rare silent stall during the Ring-3 mmap pager acceptance test. The failure reproduced on one CPU, ruling out AP-startup and cross-CPU affinity as the primary cause. Stage 7.1.5 therefore repairs scheduler and pager state-transition hazards rather than adding another timing-sensitive diagnostic workaround.

## Corrections

### 1. Transactional scheduler selection

`Scheduler::prepare_switch()` now resolves and validates the incoming task address space **before** publishing any new `Running`/`Switching` ownership. The previous order could theoretically mutate scheduler state and then return `None` through `?` if an incoming Ready task lacked an address space. That would create scheduler metadata without the corresponding physical stack switch.

A Ready task missing an address space is now an explicit invariant violation.

### 2. Reschedule requests are consumed before yielding

`preemption_point()` now uses an atomic `swap(false)` before `yield_now()`. Previously it loaded the flag, yielded, and cleared the flag only after the task eventually resumed. A newer reschedule request published while that task was switched out could therefore be erased on return.

The new rule is generation-safe at this level: the request being serviced is consumed first; any later request remains set.

### 3. Pager no longer polls with `sleep_current(1)`

The pager now has an explicit queue-empty blocking handshake. It holds the pager queue lock across the empty check and publication of its `Blocked` task state. Producers must acquire the same queue lock before enqueueing, so an enqueue cannot slip between “queue empty” and “pager blocked”. After the pager publishes `Blocked`, it releases the queue lock; a producer can then enqueue and make the pager Ready under the scheduler lock.

This removes timer polling and the associated lost-wakeup/check-sleep window.

### 4. Race-safe idle parking

The idle task no longer performs a plain `preemption_point(); hlt();` sequence. It disables interrupts, rechecks the reschedule flag, and uses x86 `enable_and_hlt()` for the actual park. This closes the classic interrupt-between-check-and-HLT window while retaining the Stage 7.1.3 goal of avoiding a busy idle loop on the global scheduler spinlock.

### 5. Existing invariants retained

The Stage 7.1.1 AP-online publication ordering, Stage 7.1.2 fork first-dispatch handoff, `Switching` state, CPU-affinity restrictions, scheduler/process-table lock ordering, and block/sleep replacement assertions remain in place.

## Acceptance

Run:

```powershell
.\run-stage7-1-5-acceptance.ps1
```

The harness builds, runs Clippy with warnings denied, runs host tests, then executes 50 one-CPU memory/boot runs, 100 two-CPU runs, 50 four-CPU runs, and network acceptance on 1/2/4 CPUs. It stops on the first failure and preserves the serial log plus a filtered diagnostic summary under `stage7-1-5-artifacts`.

Stage 7.1 must not be frozen until this complete gate passes.

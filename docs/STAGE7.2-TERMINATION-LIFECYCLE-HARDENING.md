# Stage 7.2 — Scheduler-Owned Termination & Process Lifecycle Hardening

## Problem closed by this stage

Before Stage 7.2, `kill_process()` could directly set another task's TCB to
`Dead`, decrement `task_count`, publish `ProcessState::Exited`, and release file
references. That decision was made by the killer, not by the CPU that physically
owned the target task. This was acceptable only while userspace was effectively
serialized; it is not a safe basis for future multi-CPU userspace.

## New state flow

```text
kill(pid, SIGTERM/SIGKILL/SIGINT)
        |
        v
record pending signal in Process
        |
        v
SCHEDULER.request_termination(task, signal)
        |
        +-- Ready/Blocked/Sleeping --> Dead --> publish Exited
        |                              (safe: not physically executing)
        |
        +-- Running/Switching --> termination_signal latched
                                  |
                                  v
                           reschedule / handoff
                                  |
                                  v
                           finalize_switch()
                                  |
                                  v
                         Dead --> publish Exited

waitpid()
   |
   v
release deferred mappings/address space
   |
   v
remove zombie process slot
```

## Safety invariant

A task that is physically executing may not be changed to `Dead` by another CPU.
Its terminating transition is acknowledged only after it has surrendered the
CPU, or by itself at a safe syscall return boundary.

## Resource lifetime

For signal termination, file references are released when scheduler retirement
is acknowledged. Address-space destruction is intentionally deferred until
`wait_process()`. This avoids expensive page-table teardown under the scheduler
lock and guarantees no live CPU context can still depend on those mappings.

## Locking

The existing global ordering is preserved:

`SCHEDULER -> PROCESS_TABLE`

No path introduced in Stage 7.2 takes PROCESS_TABLE and then SCHEDULER.

## Acceptance

The boot suite now creates a userspace process, leaves it Ready/off-CPU, delivers
SIGTERM, requires process exit to become visible only through scheduler-owned
retirement, then requires `wait_process()` to return 143 and reclaim its deferred
address space. The serial marker is:

`[TERM] scheduler-owned Ready-task termination + deferred reap: PASSED`

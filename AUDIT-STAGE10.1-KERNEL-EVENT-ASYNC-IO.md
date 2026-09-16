# WovenHat OS Stage 10.1 — Kernel Event + Async I/O Foundation

## Objective
Stage 10.1 converts the already-worker-backed block-I/O queue into a genuinely
event-driven scheduler path. The previous implementation queued work on a
storage worker but the submitting task repeatedly called `yield_now()` until
its request became complete. The worker was asynchronous; the waiter was still
polling.

## New control flow

```text
caller task
   |
   | queue request + TaskId waiter
   | (completion event armed before worker wake)
   v
block I/O queue ----------------------+
   |                                  |
   | wake worker                      | caller: wait_for_event()
   v                                  |        -> Blocked
storage worker                        |
   |                                  |
   | perform ATA operation            |
   | publish RequestState::Complete   |
   | signal_event(waiter TaskId) -----+
   v
scheduler marks waiter Ready / latches pending permit
   |
   v
caller rechecks its request and consumes result
```

## Race property
The Stage 8.3 event is scheduler-latched. If completion occurs before the task
actually blocks, `signal_event` stores the pending permit in the TCB. The later
`wait_for_event` consumes that permit instead of sleeping. If the task is
already blocked, the same signal makes it Ready. Thus completion cannot vanish
in the check/sleep window.

Each queued block request is armed with its waiter before the worker is woken,
which also prevents completion from occurring without a corresponding event
for the submitter. Unrelated task events are treated as spurious wakeups: the
request state is rechecked and the task waits again if necessary.

## Acceptance proof
`stage10_1_event_driven_completion_valid()` executes a real queued ATA request
through the block worker and requires all of these counters to advance:

- queued requests
- completed requests
- scheduler event waits
- worker completion signals

It also requires the request table to return to zero active entries.

The Stage 10.1 acceptance wrapper first runs the complete validated Stage 9.5
acceptance chain unchanged, then requires the Stage 10.1 production marker in
1-, 2-, and 4-CPU memory-regression serial logs.

## Security boundary
Stage 10.1 does not bypass Stage 9.5. Resource authorization remains at the
existing WovenGuard device/network/storage gates. This stage changes scheduling
and completion mechanics, not who is authorized to perform I/O.

# WovenHat OS Stage 10.2 — Generic Async Request / Completion API

## Goal
Generalize the Stage 10.1 event-driven block-I/O proof into one bounded kernel asynchronous-operation substrate that future storage, network, device, service, graphics, input, USB, and AI-service paths can reuse.

## Architecture
Stage 10.2 adds `kernel/src/async_op.rs` with:

- a fixed-capacity, allocation-free global operation table;
- generation-tagged `Handle` values to reject stale references;
- owner `TaskId` binding;
- explicit `AsyncClass` identity (`Block`, `Network`, `Device`, `Service`);
- `Pending` and `Complete` lifecycle state;
- compact completion payload (`status`, `value`);
- scheduler-event-backed blocking waits;
- producer completion from a different task/worker;
- rollback release when a producer cannot enqueue its real work;
- counters for allocations, completions, waits, scheduler signals, releases, stale-handle rejection, and live operations.

## Lost-wakeup rule
Wait registration and completion are serialized by the async table lock. If the owner registers first, completion signals that task through the Stage 8.3 scheduler event. If completion occurs first, the `Complete` state itself is observed by the later wait, so no completion is lost.

Unrelated scheduler events may wake a task; the waiter therefore rechecks its generation-tagged operation state until its own completion is visible.

## Real block-I/O integration
`kernel/src/block_io.rs` no longer owns its own task waiter identity. A queued block request now stores an `async_op::Handle`.

1. submitter allocates `AsyncClass::Block`;
2. request is inserted into the bounded storage queue;
3. enqueue rollback releases the generic handle if the storage queue is full;
4. storage worker writes the request result/data into the storage queue;
5. storage worker calls `async_op::complete`;
6. owner calls `async_op::wait` and then consumes the block result.

This preserves Stage 10.1's non-polling storage behavior while moving completion ownership into the generic substrate.

## Stage 10.2 production proof
`stage10_2_generic_async_completion_valid()` requires:

- all four async classes to allocate and round-trip completion metadata;
- completion-before-wait to succeed;
- a stale consumed handle to be rejected;
- generic allocation/completion/release/stale counters to advance;
- a real queued ATA request to pass through the generic API;
- both generic async and block-I/O active tables to drain back to zero.

Kernel marker:

```text
[S10.2] generic async request/completion API: PASSED
```

## Preservation contract
`run-stage10-2-acceptance.ps1` runs the complete validated Stage 10.1 acceptance chain unchanged first, then requires the Stage 10.2 production marker on the existing 1/2/4 CPU production logs.

No existing timeout, stress count, warning policy, network test, WovenGuard test, IPC test, or SMP criterion is weakened.


## SMP runtime hardening
A later 2-CPU stress timeout exposed two worker-wakeup hazards. The worker is
now identified by TaskId rather than a global IN_WORKER flag, and producer
notification uses signal_event()/wait_for_event() instead of wake_task() plus
periodic sleep. See AUDIT-STAGE10.2-SMP-RUNTIME-FIX.md.

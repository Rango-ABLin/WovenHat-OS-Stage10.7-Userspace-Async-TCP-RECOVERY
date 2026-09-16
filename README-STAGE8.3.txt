WovenHat OS Stage 8.3 — Race-Free IPC Events / Waits
=====================================================

Baseline
--------
This candidate is built directly from the Stage 8.2 CLIPPY-FIX tree that passed
full Stage 8.2 acceptance on 1/2/4 CPUs.

What Stage 8.3 adds
-------------------
1. A scheduler-latched per-task event permit (`event_pending`).
2. `task::signal_event(TaskId)`:
   - wakes a task already Blocked; or
   - latches a permit if the task has not blocked yet.
3. `task::wait_for_event()`:
   - consumes an early permit without sleeping; or
   - atomically publishes Running -> Blocked under the scheduler lock.
4. Bounded read/write waiter lists on each IPC endpoint object.
5. `ipc::receive_handle_blocking()` for empty-queue waits.
6. `ipc::send_handle_blocking()` for full-queue waits.
7. Wake-one semantics when a message arrives or queue capacity becomes free.
8. Exact-handle waiter cleanup on close and owner waiter cleanup on unregister.
9. A production runtime probe that forces both blocked-receiver and
   blocked-sender paths. On SMP, the sender runs on the highest online AP.

Why the latched event exists
----------------------------
The unsafe sequence is:
  queue empty -> release IPC lock -> sender sends/wakes -> receiver blocks

If wake only works on an already-Blocked task, the wake can disappear.
Stage 8.3 instead records an early event in the receiver's TCB. When the
receiver reaches wait_for_event(), it sees the permit and does not sleep.

This also avoids acquiring the scheduler lock while holding the IPC lock.
That matters because existing process-creation paths can already hold scheduler
state while registering IPC state; adding IPC -> scheduler nesting would create
a lock-order inversion.

Intentionally NOT included yet
------------------------------
- wait-many / select across several handles
- userspace IPC wait syscalls
- timeout/cancellation semantics
- signal interruption of IPC waits
- shared memory
- general userspace handle transfer (Stage 8.5)
- service discovery

Validation
----------
Run from the project root:

  $env:CARGO_NET_OFFLINE = "true"
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage8-3-acceptance.ps1

Required final marker:

  === STAGE 8.3 ACCEPTANCE: PASS ===

Until that marker is observed on the user's Windows/QEMU environment, this
package is an IMPLEMENTED / STATIC-AUDITED candidate, not a validated stage.

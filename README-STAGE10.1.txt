WovenHat OS — Stage 10.1 Kernel Event + Async I/O Foundation

Purpose
-------
Move the existing asynchronous block-I/O path from scheduler polling to real
scheduler-integrated completion events while preserving every validated Stage
9.5 security, SMP, IPC, filesystem, and networking guarantee.

What changed
------------
* Each queued block request records the submitting TaskId as its completion waiter.
* The request is completion-event armed before the worker is woken.
* The submitter waits with task::wait_for_event() instead of repeated yield_now().
* The block worker publishes Complete under the queue lock, then signal_event()s
  only the task that owns that request.
* Stage 8.3's latched-event semantics cover completion-before-wait safely.
* Spurious/unrelated scheduler events are harmless: the waiter rechecks its own
  request and sleeps again until that request reaches Complete.
* Counters prove real event waits and completion signals occurred.
* Stage 10.1 adds a production marker on 1/2/4 CPU boots.

Run
---
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\RUN-STAGE10.1.ps1

Expected ending
---------------
Stage 10.1 kernel event + async I/O foundation marker: PASS (1 CPU)
Stage 10.1 kernel event + async I/O foundation marker: PASS (2 CPU)
Stage 10.1 kernel event + async I/O foundation marker: PASS (4 CPU)

=== STAGE 10.1 ACCEPTANCE: PASS ===

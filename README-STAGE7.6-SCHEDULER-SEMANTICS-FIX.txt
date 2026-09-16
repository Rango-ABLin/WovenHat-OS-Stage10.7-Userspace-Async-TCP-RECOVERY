WovenHat OS Stage 7.6 Scheduler State Semantics Correction
===========================================================

Problem
-------
The first Stage 7.6 race fix overloaded a TCB `has_started` flag: it was both
execution-history metadata and a policy gate in Stage 7.4 explicit Ready
migration/affinity APIs. That changed behavior already validated by Stage 7.4
and caused the hard-affinity preservation test to fail.

Corrected semantics
-------------------
The field is now named `ever_dispatched` and has one meaning only:
"the scheduler has selected this task in a Ready -> Running transition at
least once." It is written while the global scheduler lock is held.

Policy use
----------
- Stage 7.6 automatic `rebalance_one()` checks `ever_dispatched` for Ring-3
  tasks. Automatic userspace balancing therefore remains pre-first-dispatch.
- Stage 7.4 `migrate_ready_task()` does NOT use `ever_dispatched`; it preserves
  the validated Ready + migratable + affinity contract.
- Stage 7.4 `set_ready_task_affinity()` does NOT use `ever_dispatched`; it also
  preserves the validated Ready + migratable + affinity contract.
- Running/Switching tasks still cannot be migrated because both explicit APIs
  require TaskState::Ready, and automatic balancing only considers Ready.

Why this is the right boundary
------------------------------
Execution history is scheduler state, not authority. Stage 7.6 needs history
to keep automatic policy conservative, but Stage 7.4's explicit operations
already have a separate state/affinity contract and must not be silently
redefined by the automatic-balancing milestone.

Validation status
-----------------
This package is corrected source, not yet accepted. Run:

  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage7-6-acceptance.ps1

Do not tag Stage 7.6 until the script prints:

  === STAGE 7.6 ACCEPTANCE: PASS ===

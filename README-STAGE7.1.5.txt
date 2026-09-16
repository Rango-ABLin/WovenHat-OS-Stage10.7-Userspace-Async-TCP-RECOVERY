WovenHat OS Stage 7.1.5 — Scheduler/Pager Correctness Repair

Purpose:
  Repair the rare pager/scheduler stall seen in Stage 7.1.3/7.1.4.

Main changes:
  - Scheduler selection validates incoming address space before committing state.
  - preemption_point consumes the current reschedule request before yielding.
  - Pager uses an explicit queue-empty Blocked/wake handshake instead of 1-tick polling.
  - Idle parking rechecks pending work with interrupts disabled and uses STI+HLT semantics.
  - Stage 7.1.1/7.1.2 affinity and first-dispatch fixes remain intact.
  - Stage 7.1.4 detailed yield tracer remains available but is disabled during normal acceptance.

Run the complete gate with:
  .\run-stage7-1-5-acceptance.ps1

Do not advance to Stage 7.2 until the script reports:
  STAGE 7.1.5 ACCEPTANCE: PASS

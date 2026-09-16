> SUPERSEDED by `AUDIT-STAGE7.6-SCHEDULER-SEMANTICS-FIX.md`. The `has_started` policy described below regressed the validated Stage 7.4 explicit affinity path.

WovenHat OS Stage 7.6 — corrected integrated multicore userspace candidate

This revision fixes the intermittent 2-CPU timeout seen after 24 successful runs.

Correction:
  Ring-3 ownership migration is now restricted to pre-first-dispatch Ready tasks.
  A TCB has_started flag is published under the scheduler lock at Ready -> Running.
  The periodic rebalancer, explicit Ready migration, and affinity transfer all
  refuse to move a userspace task after it has executed once.

This preserves the validated Stage 7.4 migration boundary while retaining the
Stage 7.6 least-loaded placement and automatic balancing of fresh Ring-3 work.

Run:
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage7-6-acceptance.ps1

Do not tag Stage 7.6 until the complete acceptance suite passes.

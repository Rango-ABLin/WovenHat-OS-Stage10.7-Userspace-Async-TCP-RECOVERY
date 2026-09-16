# Stage 7.1.4 Scheduler Handoff Diagnostic

## Trigger
A pager acceptance run can stop after `[PAGER-DIAG] yield: BEGIN iteration=N`
without a panic, page fault report, or exception. The same signature has now
appeared in a 1-CPU run, ruling out an SMP-only lock starvation explanation.

## Instrumented state machine
`yield_now()` now emits a numbered sequence and phase markers while the pager
acceptance window is active. The diagnostic captures CPU, scheduler slot,
task ID/name/state, `switching_out`, whether a switch was prepared, the selected
task, and the incoming saved RSP.

The critical transition is:

`Running -> Switching -> physical stack switch -> finalize_switch() -> Ready`

For never-before-run tasks, finalization occurs in `task_bootstrap()`. Fork
children use `wovenhat_fork_first_dispatch` before syscall resume. Previously-run
tasks finalize after `wovenhat_context_switch` returns in `switch_stacks()`.

## Additional invariant guards
`block_current()` and `sleep_current()` previously allowed this invalid outcome:

1. mark current task Blocked/Sleeping;
2. `prepare_switch()` returns `None`;
3. the function continues physically executing that task.

Stage 7.1.4 converts this into an explicit diagnostic panic. The final scheduler
should eventually guarantee an idle fallback rather than rely on this assertion.

## Scope
No affinity policy is relaxed. No userspace migration is enabled. No handoff
assertion is removed. This build exists only to localize the remaining stall.

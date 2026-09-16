# WovenHat OS Stage 7.1.2 — Fork First-Dispatch Scheduler Handoff Fix

## Failure observed
The Stage 7.1 acceptance gate intermittently panicked on a 1-CPU run during the pager regression loop:

`assertion failed: nested scheduler hand-off on one CPU`

The stale value was `switching_out[0] == 0`, meaning the BSP kernel task had been published as the outgoing `Switching` task but its handoff had not been finalized before a later scheduling attempt.

## Root cause
Normal kernel tasks and newly-created userspace processes first enter through `task_bootstrap()`, which calls `Scheduler::finalize_switch()` before enabling interrupts or entering the task body/user context.

A newly forked child was different. `TaskControlBlock::initialize_fork()` built a context that returned directly to `wovenhat_syscall_resume`. Therefore its first dispatch bypassed `task_bootstrap()` and could leave the per-CPU `switching_out[]` marker stale.

Later scheduling activity (the pager test happened to expose it) correctly detected that stale marker and panicked rather than permitting a second handoff on the same CPU.

## Fix
Stage 7.1.2 adds `wovenhat_fork_first_dispatch`, a tiny assembly trampoline used only for the first dispatch of a fork child. It:

1. calls `wovenhat_finalize_scheduler_handoff`,
2. returns with the saved fork frame still intact,
3. jumps to the existing `wovenhat_syscall_resume` path,
4. resumes ring 3 exactly as before.

The invariant is not weakened or removed.

The obsolete Rust helper `syscall::resume_address()` is removed because the fork initializer no longer uses it.

## Safety properties preserved
- A task in `Switching` is not published Ready until the physical stack switch has completed.
- The first dispatch of every task kind now finalizes the outgoing handoff.
- Fork register state remains stored in `UserForkFrame`; the bootstrap call occurs below that frame and does not consume it.
- Normal syscall returns are unchanged.
- Interrupts remain disabled during first-dispatch handoff finalization.

## Validation gate
Run:

```powershell
cargo build
cargo clippy -p wovenhat-kernel -- -D warnings
cargo test
python .\scripts\test-memory-qemu.py --cpus 1
```

Then run a focused repeated 1-CPU test before the full gate. Finally run:

```powershell
.\run-stage7-1-acceptance.ps1
```

Stage 7.1 is not frozen until the full acceptance matrix passes.

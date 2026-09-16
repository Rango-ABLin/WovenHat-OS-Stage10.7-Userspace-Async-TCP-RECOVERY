> SUPERSEDED by `AUDIT-STAGE7.6-SCHEDULER-SEMANTICS-FIX.md`. The `has_started` policy described below regressed the validated Stage 7.4 explicit affinity path.

# Stage 7.6 SMP race fix — first-dispatch migration boundary

## Failure observed
The original Stage 7.6 closure candidate passed build, Clippy, host tests, all 1-CPU preservation runs, and 24 consecutive 2-CPU stress runs before the 25th 2-CPU memory run timed out.

That pattern identifies a timing-sensitive scheduler ownership problem rather than a deterministic compiler, pager, VFS, or network failure.

## Root cause
Stage 7.4 validated Ready-state Ring-3 migration **before first dispatch**. The Stage 7.6 periodic rebalancer broadened this rule unintentionally: any migratable userspace task that later became `Ready` again after it had already executed could be moved to another CPU.

`Ready` proves that the task is not physically executing at that instant, but it does not by itself prove that WovenHat's post-dispatch userspace kernel-return/context history is portable between CPU ownership paths. That stronger protocol has not yet been implemented or validated.

## Fix
`TaskControlBlock` now carries `has_started`.

- Fresh tasks start with `has_started = false`.
- The bootstrap kernel task starts with `has_started = true` because it is already executing.
- `prepare_switch()` sets `has_started = true` under the scheduler lock at the same transition that publishes `Ready -> Running`.
- Kernel tasks retain the existing Ready migration rules.
- Ring-3 tasks may be moved only while `Ready && !has_started`.
- `rebalance_one()`, `migrate_ready_task()`, and `set_ready_task_affinity()` all enforce the same boundary.

The result preserves Stage 7.6's least-loaded initial placement and pre-first-dispatch automatic Ring-3 balancing while preventing the unproven post-dispatch migration class.

## What remains intentionally deferred
A later milestone may implement true post-dispatch userspace migration. That work must explicitly audit saved syscall/interrupt return frames, per-CPU kernel state, FPU/SIMD state, TLS, pending signals, and any per-CPU service ownership before relaxing this guard.

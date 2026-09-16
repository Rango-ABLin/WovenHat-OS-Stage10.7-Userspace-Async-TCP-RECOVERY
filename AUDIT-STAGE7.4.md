# Stage 7.4 source audit — Ready-state Ring-3 Migration & Affinity

Baseline: validated Stage 7.3 Pinned Ring-3 Foundation.

## Scope
- Normal `spawn_user_process()` remains CPU0-owned and non-migratable.
- Stage 7.3 `spawn_pinned_user_process_on()` remains hard-pinned.
- New `spawn_migratable_user_probe()` is the only new Ring-3 path that sets `migratable = true`.
- The probe starts Ready on CPU0 with the current online affinity mask.
- Existing `migrate_ready_task()` and `set_ready_task_affinity()` now accept an explicitly migratable Ring-3 task.
- Both operations still require `TaskState::Ready` and hold the global scheduler lock while changing ownership.
- Running, Switching, Blocked and Sleeping task migration remains rejected by the Ready-state check.
- General userspace placement, fork migration, filesystem/network/pager migration and concurrent service I/O are not enabled.

## Runtime proofs added
1. Explicit CPU0 -> highest-online-AP Ready-state ownership migration of `/bin/true`.
2. Single-bit hard-affinity ownership transfer of a Ready `/bin/true` probe to the highest online AP.
3. Exit(0) and parent reap after each transfer.
4. Stage 7.2 termination and Stage 7.3 AP Ring-3 markers remain mandatory.
5. Network regression remains mandatory on 1/2/4 CPUs.

## Local validation limitation
This packaging environment does not contain the Rust/Cargo/QEMU toolchain, so compile/Clippy/runtime acceptance must be established by `run-stage7-4-acceptance.ps1` on the development host.

# Stage 7.6 Candidate Audit

Baseline: validated Stage 7.5.

Kernel source changes are intentionally concentrated in:

- `kernel/src/task.rs`: least-loaded audited userspace placement, Ready-only Ring-3 rebalance eligibility, userspace migration accounting, fork placement inheritance.
- `kernel/src/main.rs`: integrated Stage 7.6 boot proofs and AP mmap/fork execution through the multicore API.

Acceptance harness:

- `run-stage7-6-acceptance.ps1`

Safety invariants retained:

- `SCHEDULER -> PROCESS_TABLE` lock order.
- migration only when `TaskState::Ready`.
- owner CPU must be online and inside non-zero affinity.
- Switching remains physically running and cannot be migrated.
- legacy userspace stays pinned because `initialize_user()` itself is unchanged.
- file-fault I/O remains protected by the Stage 7.5 per-CPU depth guard.
- acknowledged global TLB shootdown remains unchanged.

Candidate is not called complete until the acceptance script passes all build, lint, host test, repeated 1/2/4 CPU memory regressions and 1/2/4 CPU network regressions.

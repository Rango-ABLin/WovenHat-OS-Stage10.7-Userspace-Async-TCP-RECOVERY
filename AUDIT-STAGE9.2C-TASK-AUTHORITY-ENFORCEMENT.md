# Stage 9.2C Audit - Real Task Authority Enforcement

## Security objective
Turn WovenGuard lineage from metadata into an enforcement dependency for real task capabilities.

## Enforcement model
A task capability is effective only when:
- its raw capability bit is present, and
- if a lineage binding exists, that generation-tagged lineage is still live and contains that capability.

This means recursive lineage revocation takes effect at the next capability check without requiring a mutable sweep through every task.

## Locking
The existing global order remains scheduler -> WovenGuard lineage registry.
Lineage registry operations never acquire the scheduler lock, so the new effective-authority checks do not introduce a reverse lock edge.

## Delegation
`task::grant()` still passes Stage 9.1 domain/TaskControl/no-amplification policy first. A successful delegation then derives a one-capability child lineage and stores it beside the target capability bit.

## Revocation
`task::revoke()` first passes TaskControl policy, then revokes a bound lineage subtree and clears local state. Empty temporary roots are released to preserve the fixed 64-entry lineage budget.

## Fork
Fork inherits capability lineage bindings with capability state. This prevents delegated authority from becoming intrinsic merely because a process forked.

## Lifecycle
`Scheduler::reap_dead()` recalls all lineage nodes owned by a retiring task before recycling its TCB. Descendants are invalidated by the same recursive semantics proved in Stage 9.2B.

## Scope intentionally deferred
- IPC handle lineage binding
- service discovery lineage binding
- shared-memory handle lineage binding
- SMP stress specifically targeted at simultaneous capability use/revocation

These should be closed in the next enforcement/stress stage rather than mixed into the first task-authority boundary.

## Build-cleanup correction — strict `-D warnings`

The Stage 9.2B inactive-address-space TLB liveness refactor introduced
`protect_user_range_in_inactive()` for ELF construction. After that migration,
the former public `protect_user_range_in()` wrapper had no remaining caller in
the kernel binary and therefore tripped the project's strict dead-code policy.

The obsolete wrapper has been removed. The shared
`protect_user_range_in_impl(...)` implementation and the inactive loader path
remain unchanged. This is an API/liveness cleanup only; it does not weaken TLB
synchronization for any live mapping path.

## Strict-Clippy follow-up fix

Focused validation exposed two `-D warnings` Clippy findings in `kernel/src/task.rs` after the Stage 9.2C enforcement changes:

1. `initialize_fork` exceeded the seven-argument Clippy threshold after capability lineage and security-domain inheritance were added. The security fields are now carried by `ForkSecurityContext`, keeping fork security provenance as one coherent value rather than suppressing `clippy::too_many_arguments`.
2. The intrinsic-lineage cleanup path used a nested `if` that triggered `clippy::collapsible_if`. The condition was flattened with identical semantics; no authorization or cleanup check was removed.

No lint allowances were added. Runtime policy and revocation semantics are unchanged.

## SMP IRQ-safe lineage registry correction
Full acceptance exposed an intermittent 4-CPU timeout after the Stage 9.2A marker. The cause was same-CPU interrupt re-entrancy on the plain `LINEAGES` spin mutex after Stage 9.2C made lineage cleanup reachable from scheduler dead-task reaping. All lineage-registry access is now wrapped in `x86_64::instructions::interrupts::without_interrupts`, and retiring-task lineage cleanup executes as one bounded IRQ-safe registry transaction. See `AUDIT-STAGE9.2C-SMP-IRQ-LINEAGE-LOCK-FIX.md`.

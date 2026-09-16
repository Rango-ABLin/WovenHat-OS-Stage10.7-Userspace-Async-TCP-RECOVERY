# Stage 9.2C SMP IRQ-Safe Lineage Lock Fix

## Failure observed
During full Stage 9.2C acceptance, build, strict Clippy, host tests, 1-CPU x10, 2-CPU x30, and the first ten 4-CPU memory runs passed. 4-CPU run 11 timed out. The preserved serial log stopped immediately after:

`[S9.2A] WovenGuard capability lineage foundation: PASSED`

with no Stage 9.2B marker.

## Root cause
Stage 9.2C made the WovenGuard lineage registry reachable from scheduler lifecycle code through `Scheduler::reap_dead()` -> `wovenguard::revoke_all_owned_lineages()`.

The lineage registry used a plain `spin::Mutex`. Stage 9.2A/9.2B operations can hold that lock in normal kernel context while timer interrupts and scheduling are live. If the same CPU is interrupted while it owns `LINEAGES`, the interrupt-driven scheduler can enter dead-task reaping and try to acquire `LINEAGES` again. A spin mutex is not re-entrant, so that CPU waits forever for a lock it already owns.

This is a same-CPU interrupt re-entrancy deadlock. Its timing sensitivity explains why 1 CPU, all 2-CPU runs, and the first ten 4-CPU runs passed before the 4-CPU run 11 timeout.

## Fix
`kernel/src/wovenguard.rs` now centralizes every lineage-registry access through:

```rust
fn with_lineages<R>(operation: impl FnOnce(&mut LineageRegistry) -> R) -> R {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut registry = LINEAGES.lock();
        operation(&mut registry)
    })
}
```

This guarantees that local interrupts cannot re-enter lineage code while the CPU owns the registry lock. Other CPUs still synchronize through the spin mutex, so SMP mutual exclusion is preserved.

`revoke_all_owned_lineages()` was also changed to hold one IRQ-safe registry critical section for its complete bounded cleanup rather than repeatedly releasing and reacquiring the registry lock.

## What was deliberately not changed
- No timeout was increased.
- No acceptance count was reduced.
- No warning or Clippy lint was suppressed.
- Stage 9.2B recursive-revocation semantics were not weakened.
- Stage 9.2C task-capability enforcement semantics were not weakened.
- The scheduler, paging, IPC, and networking acceptance criteria remain unchanged.

## Validation requirement
Run the unchanged full Stage 9.2C acceptance script. Stage 9.2C is not considered fully validated until the complete 1/2/4 CPU and network matrix passes.

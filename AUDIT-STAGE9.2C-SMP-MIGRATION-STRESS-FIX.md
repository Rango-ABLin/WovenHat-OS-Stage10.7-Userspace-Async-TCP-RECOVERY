# Stage 9.2C SMP migration-stress deterministic coverage fix

## Failure evidence
The preserved 2-CPU run-8 serial log did not hang. It reached a kernel panic in `kernel/src/smp.rs` at the final migration-stress CPU-mask assertion:

- observed mask: `0b10` (`2`)
- expected mask: `0b11` (`3`)
- panic: `migration stress did not exercise every CPU`

All WovenGuard markers S9.1, S9.2A, S9.2B and S9.2C had already passed in that boot. The pager and integrated Stage 7.6 userspace tests had also passed.

## Root cause
The old migration stress test spawned every migratable probe Ready on CPU 0, then immediately created migration/rebalancing pressure. A valid scheduler execution can migrate every Ready probe from CPU 0 to CPU 1 before any probe executes its first instruction. The test then sees only CPU 1 in `STRESS_MASK` and panics even though migration itself worked. This is a timing-dependent test oracle, not evidence that WovenGuard failed.

## Correction
The stress test now separates two properties that were previously conflated:

1. deterministic execution coverage: one short pinned atomic-only probe is spawned on every online CPU and the test waits for every CPU to execute;
2. migration pressure: the original `online_count * 4` migratable jobs still start on CPU 0, run 64 migration/rebalance rounds, require at least `jobs` successful ownership transfers, and must all complete.

The stress counts, migration threshold, timeout budgets, strict assertions, and production scheduler behavior are not weakened. The migration phase receives its own original 500-tick budget after coverage completes.

## Scope
Changed file: `kernel/src/smp.rs`.
The earlier WovenGuard IRQ-safe lineage registry hardening remains intact; the new preserved failure demonstrates that the reported run-8 failure itself was the SMP test oracle panic described above.

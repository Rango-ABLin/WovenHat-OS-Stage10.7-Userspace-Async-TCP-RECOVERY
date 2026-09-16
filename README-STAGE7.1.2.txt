WovenHat OS Stage 7.1.2
=======================

This release layers two scheduler correctness fixes on the Stage 7.1 CPU-affinity foundation:

1. Stage 7.1.1: AP online/affinity publication race fix.
2. Stage 7.1.2: fork-child first-dispatch scheduler handoff fix.

Stage 7.1.2 fixes an intermittent 1-CPU panic:
  "nested scheduler hand-off on one CPU"

Changed source files:
  kernel/src/task.rs
  kernel/src/syscall.rs

See:
  docs/STAGE7.1.2-FORK-FIRST-DISPATCH-HANDOFF-FIX.md

The strict scheduler invariant remains enabled. Full Stage 7.1 acceptance is still required before freeze.

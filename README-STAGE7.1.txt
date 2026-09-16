WovenHat OS Stage 7.1 — CPU Affinity Foundation

BASE:
  Stage 7.0.11 self-audited stabilization candidate.

REPLACE:
  kernel/src/task.rs
  kernel/src/smp.rs

OPTIONAL DOCS/SCRIPT:
  docs/STAGE7.1-CPU-AFFINITY-AUDIT.md
  run-stage7-1-acceptance.ps1

WHAT CHANGED:
  - Hard per-task affinity_mask.
  - Affinity-aware scheduler selection.
  - Affinity-aware explicit migration.
  - Affinity-aware automatic rebalancing.
  - Safe Ready-task affinity update API.
  - Userspace remains pinned to CPU0.
  - Scheduler affinity invariant validation.
  - Dedicated Stage 7.1 negative/positive regression test.

VALIDATE:
  .\run-stage7-1-acceptance.ps1

PASS MEANS:
  build + clippy + host tests pass,
  20/20 memory tests on 1 CPU,
  100/100 memory tests on 2 CPUs,
  50/50 memory tests on 4 CPUs,
  network tests pass on 1/2/4 CPUs.

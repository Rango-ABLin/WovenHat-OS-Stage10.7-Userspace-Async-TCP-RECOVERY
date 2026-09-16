# WovenHat OS Stage 6 — SMP Maturity Foundation

Status: bounded implementation complete; NUMA CPU-domain placement is
validated on the Windows/QEMU development host. Multi-node page placement and
hardware qualification remain open.

## Objective

Stage 6 turns the Stage-5 CPU-owned scheduler into a bounded multicore scheduler for audited kernel jobs. It does not yet make legacy userspace, filesystem, networking, allocator-sensitive, GUI, or device-I/O paths freely migratable.

## Completed mechanisms

1. Explicit `migratable` ownership on restricted kernel jobs.
2. Migration only while a task is `Ready` and while the global scheduler lock is held.
3. Per-CPU `Ready`, `Running`, and total runnable-load accounting, excluding idle tasks.
4. Least-loaded initial placement for `spawn_parallel` jobs.
5. Conservative automatic rebalancing: at most one Ready migratable job is pushed from the busiest CPU to the least-loaded CPU per pass, and a one-task difference is treated as balanced.
6. Dedicated xAPIC reschedule IPI on vector `0xe1`.
7. Reschedule IPIs after remote spawn, explicit migration, automatic migration, and explicit wakeup.
8. BSP timer-driven periodic balancing at a deliberately low cadence.
9. Migration, load-accounting, automatic-balancing, reschedule-IPI, and repeated migration stress regressions in `smptest`.
10. Release QEMU memory/storage harnesses require the new Stage-6 success markers.
11. ACPI SRAT CPU-affinity discovery records bounded NUMA proximity domains;
    initial placement and rebalancing prefer a local domain and fall back to
    deterministic load balancing when SRAT is absent.

## Ownership/safety contract

Only kernel jobs created through the audited migratable APIs may move. A migratable entry must not retain CPU-local pointers, per-CPU lock ownership, interrupt-state assumptions, or use legacy BSP-only services across scheduling points. Running tasks are never migrated. Userspace tasks remain non-migratable.

The scheduler lock is the ownership handoff boundary. The CPU field is changed only for a Ready task while this lock is held. The lock is dropped before sending the destination reschedule IPI.

## Balancing policy

The Stage-6 policy is intentionally simple and deterministic:

- load = non-idle Ready + Running tasks;
- find busiest and least-loaded online CPUs;
- do nothing when the load difference is 0 or 1;
- otherwise move one eligible Ready migratable kernel job;
- wake the destination with a reschedule IPI.

This is a foundation policy, not a final production scheduler. It has no NUMA model, cache-affinity scoring, heterogeneous-core model, deadline classes, or work stealing.

## Acceptance markers

A complete `smptest` should include:

- `[SMP] ready-task migration: PASSED`
- `[SMP] runnable-load accounting: PASSED`
- `[SMP] automatic rebalancing: PASSED`
- `[SMP] reschedule IPI: PASSED`
- `[SMP] migration stress: PASSED`
- `[SMP] remote stale-translation/refree: PASSED`
- `[SMP] per-CPU timer preemption: PASSED`
- `[SMP] scheduler/barrier: PASSED`
- `[SMP] acknowledged TLB shootdowns: PASSED`

The full `scripts/test-release.py` matrix must remain green before Stage 6 is formally accepted.

## Deferred beyond Stage 6

- general userspace task migration;
- multicore syscall/service execution;
- concurrent filesystem/network/device-I/O paths;
- NUMA-aware page allocation and memory locality policy (CPU-domain-aware
  scheduler placement is implemented; page placement remains deferred);
- x2APIC;
- CPU hotplug;
- scheduler classes beyond the current bounded priority model;
- production lock dependency tracking and priority inheritance.

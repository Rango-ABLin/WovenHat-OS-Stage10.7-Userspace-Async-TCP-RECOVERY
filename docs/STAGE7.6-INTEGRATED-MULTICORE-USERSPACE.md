# WovenHat OS Stage 7.6 — Integrated Multicore Userspace Closure

Status: **candidate until `run-stage7-6-acceptance.ps1` reaches PASS**.

## Goal

Stage 7.6 integrates the individually validated Stage 7.3, 7.4 and 7.5 Ring-3/SMP capabilities without converting every legacy userspace process into a freely migrating task.

The bounded Stage-7 contract is now:

- legacy `spawn_user_process()` remains CPU0-owned for compatibility;
- explicitly SMP-audited userspace can use `spawn_multicore_user_process()`;
- that API accepts a hard online affinity mask and selects the least-loaded allowed CPU;
- explicitly migratable Ready Ring-3 tasks participate in the same bounded scheduler rebalancer as migratable kernel work;
- migration remains Ready-only: Running/Switching/Blocked/Sleeping ownership is not moved;
- fork preserves CPU ownership, affinity and migratability from the parent;
- the Stage 7.5 pager/file-I/O workload executes through the new multicore API on an AP with a one-bit hard affinity, proving the integrated API through file faults and fork while preventing uncontrolled mid-I/O migration;
- global acknowledged TLB shootdowns remain the conservative address-space coherency mechanism.

## Main scheduler changes

### 1. Least-loaded placement

`Scheduler::least_loaded_cpu_for_mask()` uses the existing non-idle runnable-load snapshot and chooses the lowest-load CPU that is inside the requested affinity mask.

### 2. General audited multicore userspace API

`spawn_multicore_user_process(name, program, affinity_mask)` creates a Ring-3 task that is:

- Ready;
- explicitly migratable;
- constrained by a validated non-zero online affinity mask;
- initially owned by the least-loaded allowed CPU.

Remote placement publishes a reschedule request only to the selected remote CPU.

### 3. Automatic userspace Ready rebalancing

`rebalance_one()` no longer excludes Ring-3 tasks solely because `entry.is_none()`. It still requires `migratable == true`, `Ready`, online ownership and a destination inside affinity. Legacy userspace remains excluded because it is still initialized as non-migratable.

A dedicated `USERSPACE_REBALANCE_MIGRATIONS` counter allows acceptance to prove that the automatic migration was actually a Ring-3 transfer rather than an unrelated kernel-task rebalance.

### 4. Fork placement inheritance

`fork_current()` snapshots the parent task's owner CPU, migratability and affinity under the scheduler lock. The child inherits those properties after its fork context is initialized.

This fixes the old fallback where every fork child was silently reset to CPU0 affinity even when its parent was intentionally executing on an AP.

## Acceptance proof

The Stage 7.6 boot test proves:

1. Stage 7.2 scheduler-owned termination still passes.
2. Stage 7.3 AP Ring-3 execution still passes.
3. Stage 7.4 explicit Ready migration + hard affinity still passes.
4. Stage 7.5 AP pager/file-I/O still passes.
5. A burst of Ready Ring-3 probes starts CPU0-owned and causes at least one automatic **userspace** rebalance migration.
6. A normal audited multicore `/bin/true` process with all-online affinity is placed by the least-loaded policy onto an AP.
7. The AP mmap/pager workload runs through the new multicore API and its fork child inherits the AP placement contract.
8. 1/2/4 CPU network regressions still pass.

Expected final marker:

```text
=== STAGE 7.6 ACCEPTANCE: PASS ===
```

## Scope boundary

A PASS closes the bounded Stage-7 SMP/userspace foundation for the current 1–4 CPU architecture. It does **not** claim:

- CPU hotplug;
- NUMA-aware placement/allocation;
- x2APIC or large CPU topologies;
- multithreaded processes sharing one address space concurrently;
- PCID-aware TLB optimization;
- wide real-hardware driver coverage;
- that every future driver/service is safe for unrestricted migration.

Those are later scalability/runtime/driver milestones.

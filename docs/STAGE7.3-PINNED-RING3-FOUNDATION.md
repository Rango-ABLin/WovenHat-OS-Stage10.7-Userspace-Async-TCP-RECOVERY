# Stage 7.3 — Pinned Ring-3 Multicore Foundation

Stage 7.3 starts from the fully validated Stage 7.2 termination/lifecycle baseline.
It intentionally advances only one boundary: proving Ring-3 execution on application
processors (APs) without enabling general userspace migration or concurrent legacy I/O.

## Scope

Implemented:

- ordinary `spawn_user_process()` remains unchanged and CPU0-owned;
- a narrow `spawn_pinned_user_process_on(cpu, ...)` API can publish one userspace
  task hard-pinned to a selected online CPU;
- Stage 7.3 userspace is never migratable and its affinity mask must equal exactly
  the owner CPU bit;
- `/bin/true` can be constructed directly as a tiny in-memory Ring-3 probe;
- boot validation runs that probe sequentially on every AP and requires exit code 0;
- Stage 7.2 scheduler-owned termination remains an acceptance requirement;
- existing memory/boot and network gates remain intact.

Not implemented in Stage 7.3:

- general userspace load balancing;
- userspace Ready-task migration;
- fork inheritance across CPUs;
- concurrent filesystem/network/pager/device-service execution;
- changing normal userspace away from CPU0;
- NUMA, CPU hotplug, x2APIC, or broader hardware coverage.

## Why this is safer than enabling all multicore userspace at once

The old Stage 7.2 ownership rule protected unaudited service domains by keeping normal
userspace on CPU0. Stage 7.3 keeps that rule. Only the minimal `/bin/true` probe is
placed on APs. It performs `exit(0)` and therefore proves the privilege transition,
per-CPU TSS stack, address-space switch, syscall entry, process exit, scheduler handoff,
and reap path without exercising file/network/pager/device syscalls from AP userspace.

## New invariants

For every live userspace task in Stage 7.3:

1. `migratable == false`.
2. `affinity_mask == 1 << cpu`.
3. `cpu` is online.
4. Normal userspace created by `spawn_user_process()` is still CPU0-owned.
5. AP userspace can only be created through the narrow pinned API.

## Acceptance

Run:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage7-3-acceptance.ps1
```

Required new markers:

- 1 CPU: `[S7.3] single-CPU baseline preserved; no AP Ring-3 probe required`
- 2/4 CPUs: `[S7.3] pinned Ring-3 execution on every AP: PASSED`

The Stage 7.2 marker is still mandatory:

`[TERM] scheduler-owned Ready-task termination + deferred reap: PASSED`

Stage 7.3 is complete only when build, strict Clippy, host tests, the focused 1/2/4-CPU
matrix, and networking all pass.

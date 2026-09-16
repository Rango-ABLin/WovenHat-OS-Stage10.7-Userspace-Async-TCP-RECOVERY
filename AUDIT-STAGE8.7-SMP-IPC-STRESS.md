# Stage 8.7 Audit - SMP IPC Stress

## Scope

Stage 8.7 intentionally adds no new public userspace ABI. It is a production
runtime stress proof over the validated Stage 8.1-8.6 IPC stack.

## Concurrent topology

`stage8_7_runtime_probe()` creates one service endpoint, one shared-memory
object, one server receiver task, and one pinned producer on every online CPU.
All producers operate concurrently against the same global IPC state.

Each producer performs 32 rounds:

1. discover `woven.smp-stress` and obtain a fresh process-local service handle;
2. send an ordinary blocking message or, every fourth round, enqueue a
   reduced-rights shared-memory capability;
3. close the transient discovery handle;
4. yield periodically to increase scheduling interleavings;
5. unregister its IPC namespace immediately after its final send.

The receiver deliberately refuses to drain until a capability-bearing producer
has observed `QueueFull`. Because transfer rounds occur at 0, 4, 8, ... this
forces deterministic saturation of the 16-entry endpoint queue on 1/2/4 CPUs,
without relying on timer cadence or host speed.

## Safety and ownership properties checked

- **CPU participation:** a CPU mask must contain every online CPU.
- **Exact delivery:** each `(cpu, round)` tuple is accepted exactly once.
- **Blocking wakeups:** ordinary sends use the Stage 8.3 blocking path.
- **Queue rollback:** capability sends retry on QueueFull; Stage 8.5 must release
  the temporary retain on each failed enqueue.
- **Capability escrow:** worker namespaces are destroyed while capability-bearing
  messages may remain queued; the queue escrow must preserve object lifetime.
- **Rights reduction:** received transferred handles must be exactly
  `SHM_READ | INSPECT`.
- **Fresh receiver handles:** the receiver validates the installed server-local
  handle and closes it after inspection.
- **Shared-memory transfer/lifetime:** workers concurrently transfer reduced-rights
  references to one shared object; the CPU0 receiver validates the seeded bytes
  through each fresh receiver-local handle before closing it.
- **Service discovery concurrency:** every round performs fresh Stage 8.6 lookup
  and transient handle teardown.
- **Clean closure:** no waiter or queued message remains and all object/service
  counts return to baseline after final teardown.

## AP task contract

Pinned producer tasks use only atomics, the SMP-safe IPC lock, and scheduler
yield/wait/exit primitives. They do not enter the paging layer, manipulate page
tables, or call storage, network, GUI, userspace-loader, or heap services from
AP context.

## Acceptance

`run-stage8-7-acceptance.ps1` first reruns the entire validated Stage 8.6
acceptance chain, including strict `-D warnings`, host tests, 1 CPU x10,
2 CPU x30, 4 CPU x20, and network 1/2/4. It then requires the Stage 8.7 marker
in the final 1/2/4 CPU production serial logs.

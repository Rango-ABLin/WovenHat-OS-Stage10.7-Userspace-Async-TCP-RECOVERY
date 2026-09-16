# Stage 8.7 deterministic saturation fix

## Failure observed
The original Stage 8.7 stress harness passed 1 CPU but failed on the first 2-CPU run after Stages 8.1-8.6 had passed.

## Root cause
The receiver waited until a capability-bearing nonblocking send observed `QueueFull`. However, multiple workers were also issuing ordinary blocking sends before that condition was guaranteed. With two CPUs, the bounded endpoint could become full while both workers were blocked in ordinary sends. The receiver was still waiting for a later transfer attempt to report `QueueFull`, producing a circular wait in the test harness.

This was a Stage 8.7 orchestration bug, not evidence of an IPC kernel deadlock.

## Fix
Stage 8.7 now has two explicit phases:

1. **Deterministic saturation proof**: CPU0 alone discovers the service and uses only nonblocking capability-bearing sends until the endpoint reports `QueueFull`. The receiver intentionally does not drain until this proof exists.
2. **Mixed multicore stress**: the receiver drains and validates all prefill messages, then releases every online worker into the normal 32-round mixed blocking/nonblocking workload.

This guarantees the saturation proof without permitting ordinary blocking sends to create a circular wait first.

## Added diagnostics
On timeout or final invariant failure Stage 8.7 now prints:
- online/ready/done/success worker counts;
- CPU participation mask;
- QueueFull hits;
- prefill sent/drained counts;
- normal messages sent/received;
- capability transfers sent/received;
- receiver completion/failure code;
- per-CPU worker failure code, received count and round bitmap;
- waiter counts and IPC object/shared-memory/service/endpoint counts;
- individual final invariant booleans.

## Safety boundary
AP workers still do not enter paging, storage, networking, GUI, userspace-loader or heap paths. Shared-memory content validation remains on the receiver side.

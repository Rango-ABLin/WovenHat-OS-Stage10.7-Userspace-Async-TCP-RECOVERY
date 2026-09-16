# Stage 9.4D — Sandbox Lifecycle + SMP Closure

## Goal
Close Stage 9.4 by proving that a live TCB-bound sandbox can be tightened while tasks execute on every online CPU, and that the scheduler lock is the policy linearization point seen by all readers.

## Production proof
One pinned kernel worker is created per online CPU. The coordinator applies two progressively restrictive profiles while workers are live. Workers verify the exact profile after each phase, verify `DeviceIo` is stripped while `TimerRead` remains, and then verify that rebinding the broad `KERNEL_TRUSTED` profile does not resurrect the destructively removed `DeviceIo` authority.

The second tightening also changes service exposure to `ServicePolicy::NONE` and filesystem exposure to `FilePolicy::NONE`, exercising the combined Stage 9.4A/9.4B/9.4C profile as one coherent object.

## SMP contract
`bind_sandbox_profile()` and sandbox/capability readers serialize on the global scheduler lock. The profile replacement and destructive capability stripping therefore form one linearization point. Atomic phase publication occurs only after all target TCBs have been updated.

## Acceptance
The full Stage 9.4C acceptance chain is rerun unchanged. Stage 9.4D then requires the production marker on the final 1-, 2-, and 4-CPU serial logs.

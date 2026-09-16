# Stage 10.7 preservation fix: Stage 10.6 Ring-3 UDP readiness handshake

## Failure
During Stage 10.7 preservation, the previously accepted Stage 10.6 isolated UDP test could hang on SMP after the kernel printed its receive-ready marker. The marker was emitted immediately after spawning the userspace probe, before Ring-3 had actually created and bound UDP port 7001.

## Root cause
The host harness began UDP injection on a process-spawn marker rather than an actual socket-readiness event. SMP scheduling changes the interval between process spawn and the Ring-3 bind syscall. QEMU user networking could therefore observe forwarded UDP traffic before the destination socket existed, making preservation timing-dependent.

## Fix
The Stage 10.6 Ring-3 probe now writes `[S10.6] Ring-3 UDP/7001 bound: READY` through the normal userspace write syscall only after `bind(..., 7001)` succeeds. The host harness waits for this Ring-3 marker before sending the acceptance datagram. The old kernel marker was renamed to state only that the probe process was spawned.

The previously applied async-network wakeup hardening remains in place: network progress always signals the worker, relying on the scheduler's latched event semantics.

## Safety properties preserved
- No timeout increase.
- No assertion or acceptance weakening.
- No WovenGuard capability changes.
- No socket ownership/generation changes.
- No busy-polling added.
- Stage 10.6 still uses a real host-to-guest UDP datagram.
- Stage 10.7 still preserves the complete Stage 10.6 acceptance chain before its dedicated TCP tests.

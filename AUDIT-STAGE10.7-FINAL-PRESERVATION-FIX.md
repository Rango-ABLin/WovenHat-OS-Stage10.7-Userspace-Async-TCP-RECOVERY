# Stage 10.7 final preservation fix

## Failure class
Stage 10.6 preservation could stall before the Ring-3 UDP readiness marker after Stage 10.7 development changed the async-network readiness bridge.

## Root cause
The accepted Stage 10.6 worker was woken only when the bounded async-network queue contained a Pending request. A later Stage 10.7 SMP experiment changed `network_progress()` to signal the worker on every `network::poll()`. Because scheduler events are latched, this could leave a stale event permit while a network request was retrying. On a single CPU the worker could consume that permit and retry before smoltcp had useful new progress, disturbing the proven Stage 10.6 scheduling/readiness behavior.

A second test-contract weakness was that the host readiness marker was published after `bind(7001)` but before `AsyncNetRecv` was submitted. The final handshake now publishes readiness only after both the bind and async receive submission succeed.

## Fix
1. Restore Stage 10.6's accepted `QUEUE.has_pending()` gate in `network_progress()`.
2. Keep Ring-3-owned readiness, but emit it after syscall 78 successfully creates the pinned async receive request.
3. Keep the Stage 10.7 dedicated test isolated from the legacy live network regression.
4. Make `RUN-STAGE10.7.ps1` set persistent project-local Cargo build/target directories so Windows temporary-directory cleanup cannot invalidate bootloader compilation.

No timeout, security policy, capability, assertion, or acceptance criterion was weakened.

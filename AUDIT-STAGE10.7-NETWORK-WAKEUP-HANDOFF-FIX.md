# Stage 10.7 network wakeup handoff fix

## Failure evidence
The restored Stage 10.6 preservation probe passed on 1 and 2 CPUs but could hang on 4 CPUs after the UDP target-ready marker. This localized the defect to SMP readiness delivery after socket setup, not boot, userspace startup, or host injection.

## Root cause
The async network worker changes a request from `Pending` to `InProgress` while attempting the socket operation. `network_progress()` historically signaled the worker only when it observed a `Pending` request. On SMP, smoltcp could publish readiness on another CPU while the request was `InProgress`; the progress signal was then suppressed. If the worker's socket attempt still returned `WouldBlock`, it put the request back to `Pending` and slept with the readiness edge already lost.

## Fix
A small `AtomicBool` bridge, `NETWORK_WORK_IN_PROGRESS`, is armed before a pending request is removed from the queue and disarmed only after the request has either returned to `Pending` or completed. `network_progress()` signals when either a queued request is `Pending` or the worker is inside this protected InProgress window.

This provides continuous wakeup coverage across the state transition:

`Pending -> InProgress -> Pending -> wait_for_event`

A progress edge can therefore never fall into a state where both the queue test and the handoff bridge are false. The existing scheduler event latch still protects the final signal-before-wait window.

## Preserved properties
- no timeout relaxation
- no busy polling introduced
- no capability or WovenGuard weakening
- Stage 10.6 host harness remains the last accepted baseline
- Stage 10.7 TCP uses the same corrected worker path

# Stage 8.1 — Network TX Backpressure Preservation Fix

## Failure evidence
The Stage 8.1 candidate completed the full 1-CPU, 2-CPU and 4-CPU memory/SMP preservation suite. In the final 4-CPU network preservation run, virtio-net initialized and DHCP completed, but DNS timed out.

## Root cause
`VirtioSmolDevice::transmit()` unconditionally returned a `WovenTxToken`, advertising transmit capacity to smoltcp even when the legacy polling virtio-net transport's single TX descriptor was still outstanding. `WovenTxToken::consume()` then ignored the boolean result from `virtio_net::transmit()`. If the descriptor was busy, the packet was dropped after smoltcp had already accepted the token as usable. Timing could therefore lose an outgoing DNS packet.

## Correction
- Added `virtio_net::tx_available()`.
- It takes the transport lock, reaps a completed TX descriptor, and returns true only when the transport is initialized and the descriptor is free.
- `VirtioSmolDevice::transmit()` now returns `Some(WovenTxToken)` only when `tx_available()` is true; otherwise it returns `None`, allowing smoltcp to defer/retry normally.

## Scope
No Stage 8.1 IPC semantics, scheduler state transitions, Ring-3 migration rules, timer behavior, DNS timeout, or DHCP policy were changed. This is a transport backpressure correctness fix in the pre-existing network stack.

## Validation requirement
Stage 8.1 remains unvalidated until `run-stage8-1-acceptance.ps1` completes and prints `=== STAGE 8.1 ACCEPTANCE: PASS ===`.

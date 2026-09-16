WovenHat OS Stage 8.7 - SMP IPC Stress
======================================

Purpose
-------
Stage 8.7 is a closure/stress milestone for the Stage 8 IPC architecture. It
adds no new userspace ABI. Instead it proves that the already validated IPC
features remain coherent under real concurrent multicore pressure.

Production stress topology
--------------------------
- One server endpoint is published as "woven.smp-stress".
- One producer kernel task is pinned to every online CPU.
- One blocking receiver drains the common service endpoint.
- Every producer owns a restricted handle to one shared-memory object.
- Producers repeatedly discover the named service and send 32 messages each,
  transferring restricted capabilities to the same seeded shared-memory object.
- Every fourth message carries a reduced-rights shared-memory capability.
- Ordinary messages use Stage 8.3 blocking send/wakeup semantics.
- The receiver does not begin draining until a capability send has observed
  QueueFull, making queue saturation deterministic rather than timing-based.
- Capability messages retry on QueueFull, proving Stage 8.5 retain/rollback
  correctness.
- The receiver validates sender identity, every round exactly once, transferred
  object identity, exact delegated rights, and then closes transferred handles.
- Producers unregister their IPC namespaces immediately after sending, while
  queued capability escrows may still exist.

Acceptance invariants
---------------------
- Every online CPU must run its pinned producer.
- The bounded endpoint queue must actually reach QueueFull at least once.
- Every CPU must deliver all 32 unique rounds exactly once.
- Capability transfers must arrive as fresh server-local handles naming the
  expected shared-memory object with SHM_READ|INSPECT only.
- No endpoint waiters or queued messages may remain after the receiver finishes.
- Worker namespace teardown must not invalidate queued capability escrows.
- The server shared object must still be alive until final server teardown.
- Object/shared-memory/service counts must return exactly to their pre-probe
  baselines.

Run
---
  $env:CARGO_NET_OFFLINE = "true"
  powershell.exe -NoProfile -ExecutionPolicy Bypass `
      -File .\run-stage8-7-acceptance.ps1

Expected final marker
---------------------
  [S8.7] multicore IPC/service/capability stress: PASSED
  === STAGE 8.7 ACCEPTANCE: PASS ===

Status
------
Implemented candidate. Do not declare Stage 8.7 validated until the complete
Windows Cargo/Clippy/tests + 1/2/4 CPU QEMU acceptance script passes.

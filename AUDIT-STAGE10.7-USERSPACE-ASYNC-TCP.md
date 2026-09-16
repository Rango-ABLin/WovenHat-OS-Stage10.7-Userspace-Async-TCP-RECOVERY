# Stage 10.7 — Userspace Asynchronous TCP Audit

## Goal
Extend the accepted Stage 10.6 generation-pinned async socket foundation from
UDP to TCP connection-state transitions without introducing polling in Ring 3.

## Design
- Syscall 81 `AsyncTcpConnect(socket, packed_endpoint)` allocates an owner-bound
  `AsyncClass::Network` handle and pins the exact socket generation.
- The async-network worker owns connect readiness progression. `network::poll()`
  signals the worker after smoltcp progress; connect retries only while the TCP
  socket is active but not yet send-capable.
- TCP send distinguishes genuine connection closure from temporary TX
  backpressure.
- TCP receive returns `Ok(0)` after remote FIN once no buffered bytes remain,
  giving userspace conventional EOF semantics.
- Existing Stage 10.6 deferred close means descriptors can be closed while an
  async TCP operation still references the socket; the object is removed after
  its final async reference is released.
- Send bytes are copied synchronously into a kernel buffer. Receive bytes stay
  kernel-owned until retry-safe completion copyout. No Ring-3 pointer survives
  an asynchronous operation.
- `NetworkIo` remains required at submission and collection. Cancellation is
  owner-permitted so policy tightening cannot prevent safe teardown.

## Acceptance
1. Preserve the complete accepted Stage 10.6 chain.
2. On 1/2/4 CPUs, connect to a real host TCP server through QEMU user networking.
3. Connection A proves async connect + async send + close-before-completion.
4. Connection B proves async connect/send/receive and exact host-returned bytes.
5. Host half-close/close proves async receive completes with zero-byte EOF.
6. An unreachable endpoint proves deterministic cancellation and stale-handle
   rejection.
7. A second abandoned unreachable connect proves owner teardown reclaims both
   the generic async operation and pinned socket reference.

Stage 10.7 is closed only after `=== STAGE 10.7 ACCEPTANCE: PASS ===` on the
user's QEMU environment.

## Isolation fix after first acceptance run

The first isolated Stage 10.7 boot reached the legacy qemu-test live-network regression before the Stage 10.7 block. Because the Stage 10.7 host harness services the dedicated TCP scenario rather than the legacy UDP port 7000 echo, the kernel stopped at `[NETTEST] UDP: TIMEOUT` and never reached the asynchronous TCP probe.

The runtime guard in `kernel/src/main.rs` now excludes both `stage10-6-test` and `stage10-7-test` from the legacy NETTEST block. Stage 10.7 still initializes the same VirtIO/smoltcp stack inside its own dedicated block, starts the async-network worker there, and then executes only the Stage 10.7 TCP workload. The ordinary `network-test` regression and the complete Stage 10.6 preservation acceptance remain unchanged and still validate DHCP/DNS/ICMP/UDP/TCP separately.

This is an acceptance-isolation correction only; it does not alter TCP state handling, socket generation pinning, WovenGuard authority, cancellation, EOF semantics, or teardown behavior.

## SMP readiness wakeup correction

During Stage 10.7 preservation, the previously accepted Stage 10.6 UDP receive could
intermittently stall on 4 CPUs after the host-injection readiness marker. The root
cause was a check-then-signal race in `async_network::network_progress()`: it only
signaled the worker when the queue happened to contain a `Pending` request. The
worker temporarily marks a request `InProgress` while probing smoltcp. If network
progress arrived during that interval, the wakeup was suppressed; the worker could
then restore `Pending` after `WouldBlock` and sleep with the readiness edge already
lost.

The readiness bridge now signals the async-network worker unconditionally whenever
`network::poll()` reports progress. WovenHat scheduler events already latch a signal
that arrives before `wait_for_event()`, so the signal survives the worker's
`InProgress -> Pending -> wait` transition. This fixes the SMP lost-wakeup class for
both UDP (Stage 10.6) and TCP (Stage 10.7) without introducing a busy retry loop.

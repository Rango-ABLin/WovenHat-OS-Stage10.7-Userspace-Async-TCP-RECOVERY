# WovenHat OS Stage 10.6 Audit — Userspace Asynchronous Networking Foundation

## Scope
Stage 10.6 extends the accepted Stage 10.5 generic async substrate to ordinary User-domain UDP/socket I/O. It does not expose raw NIC rings or DMA to Ring 3 and does not weaken the `NetworkIo` capability gate.

## ABI
New syscalls:
- 77 `AsyncNetSend(socket, user_buffer, length)`
- 78 `AsyncNetRecv(socket, capacity)`
- 79 `AsyncNetPoll(handle, completion_ptr, data_ptr)`
- 80 `AsyncNetWait(handle, completion_ptr, data_ptr)`

All operations use the existing generation-tagged `AsyncClass::Network` generic handle. Completion is the stable 16-byte Stage 10.3 record (`status`, `reserved`, `value`), with `value` equal to bytes sent/received. Receive payload bytes are copied separately to the caller-provided data pointer only after successful completion.

## Pointer ownership
No Ring-3 pointer survives syscall submission. Send bytes are copied into a bounded kernel request immediately. Receive requests retain only capacity; data remains in the kernel request until collection. Completion/data copyout happens before request/handle consumption, preserving retry safety on a bad user pointer.

## Socket identity and close races
The process socket table now assigns each socket a monotonically changing generation and tracks async references. Stage 10.6 pins `(slot, generation, owner)` at submission. `close()` on a pinned socket marks it closing and prevents new synchronous or async use, but the underlying smoltcp socket remains alive until the last async reference drops. Slot reuse therefore cannot redirect an in-flight operation.

## Worker/readiness model
The bounded async-network queue has Pending/InProgress/Complete states. The worker tries socket send/receive. `WouldBlock`/buffer backpressure returns the request to Pending and the worker sleeps on the scheduler event. `network::poll()` signals the worker after smoltcp/device progress, so readiness retries are not a userspace busy-poll loop. Ring 3 sleeps independently on the generic async completion event.

## Cancellation and teardown
Owner cancellation detaches Pending requests immediately; if the worker has already copied an InProgress request, the queue clears its completion handle and the worker performs final socket-unpin cleanup when it returns. Process teardown calls `async_network::release_owner()` before generic async handle teardown. Normal process socket close marks any still-pinned descriptors closing, so final async unpin removes the underlying socket deterministically.

## WovenGuard
Submission and result collection both recheck `DeviceClass::Network`, which maps to `Capability::NetworkIo`. Cancellation remains owner-only and does not require a renewed authority check, preserving safe cleanup if policy tightens after submission.

## Acceptance isolation
`stage10-6-test` implies `qemu-test` but suppresses the older live NETTEST block during the dedicated boot so it does not compete for host traffic. The Stage 10.6 harness attaches a VirtIO NIC and QEMU user network, injects a real 16-byte UDP payload through a host-forward into guest port 7001, and validates 1/2/4 CPU boots. Before these dedicated boots, the complete Stage 10.5 acceptance chain is rerun unchanged.

The Ring-3 probe exercises:
1. asynchronous UDP send followed by immediate socket close, proving send-side socket pinning;
2. asynchronous UDP receive followed by immediate close, real host packet delivery, and byte-for-byte payload validation;
3. stale-handle rejection after consumption;
4. explicit cancellation of a pending receive;
5. an abandoned pinned receive reclaimed by process teardown;
6. zero leaked async operations and zero leaked user sockets at the end.

## Validation command
`powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\\RUN-STAGE10.6.ps1`

Stage 10.6 is not closed until that acceptance prints `=== STAGE 10.6 ACCEPTANCE: PASS ===`.

## Runtime fix: connected UDP auto-bind and permanent-error isolation

The first isolated Stage 10.6 boot exposed a liveness defect in the new async UDP send path. The Ring-3 probe opened a UDP socket and connected it to the QEMU DNS proxy without first binding a local port. smoltcp requires a local UDP endpoint before transmission. The pinned async send path collapsed every send error into `WouldBlock`, so an unbound/addressing failure was treated as transient readiness backpressure and retried forever.

The socket layer now gives connected UDP sockets normal implicit-bind semantics: `socket_connect()` allocates an ephemeral local port when a datagram socket is still unbound, retrying a bounded number of ports before returning `Address`. The pinned send path also verifies that the UDP socket is open before attempting transmission, so a permanent addressing defect cannot enter the async retry loop.

This is an architectural liveness fix rather than a probe-only workaround. It preserves generation pinning, deferred close, owner binding, WovenGuard `NetworkIo`, kernel-owned buffers, cancellation, and readiness-driven retries for genuinely transient send-buffer pressure.

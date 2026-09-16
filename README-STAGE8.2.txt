WovenHat OS Stage 8.2 — Handle-Addressed Message Passing

Status: IMPLEMENTED CANDIDATE — requires Windows/QEMU acceptance before validation.
Baseline: exact validated Stage 8.1 tree including Ready-window and virtio-net TX backpressure fixes.

Scope
-----
1. Endpoint objects now own bounded FIFO message queues.
2. send_handle(sender, handle, payload) requires SEND authority.
3. receive_handle(receiver, handle) requires RECEIVE authority.
4. Empty receive returns QueueEmpty; full send returns QueueFull. No blocking/wakeup yet.
5. grant_handle() is a kernel-mediated, rights-reducing peer-handle installation primitive.
   It exists so Stage 8.2 can prove cross-process endpoint authority without prematurely
   implementing general userspace handle transfer (reserved for Stage 8.5).
6. Messages record the sending process ID and preserve FIFO order.
7. Endpoint object lifetime is reference-counted across all granted handles.
8. Closing/unregistering one process cannot destroy an endpoint still referenced by another.
9. Legacy PID-addressed MessageSend/MessageReceive remains unchanged.

Not in Stage 8.2
----------------
- blocking send/receive or wait queues
- wakeups/events
- shared memory
- userspace-general handle transfer
- service registry/discovery
- concurrent SMP IPC stress (planned Stage 8.7)

Acceptance
----------
Run:
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage8-2-acceptance.ps1

Required final marker:
  === STAGE 8.2 ACCEPTANCE: PASS ===

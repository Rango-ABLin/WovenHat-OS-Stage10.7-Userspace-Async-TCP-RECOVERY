# WovenHat OS Stage 8.2 Audit — Handle-Addressed Message Passing

## Baseline
Built directly on the validated Stage 8.1 IPC endpoint/handle foundation, including the validated Stage 7.4 Ready-window correction and virtio-net TX backpressure correction.

## Architectural change
Stage 8.1 established endpoint object identity, generation-tagged process-local handles, rights metadata and deterministic teardown. Stage 8.2 makes an endpoint object carry a bounded FIFO queue and introduces nonblocking handle-addressed message operations.

### Data path
`Process A -> SEND handle -> endpoint object FIFO -> RECEIVE handle -> Process B`

The handle is resolved inside the caller's handle space while holding the global IPC lock. The handle entry names the endpoint object and carries rights. SEND and RECEIVE are checked before touching the queue.

## New guarantees
- A SEND-only handle cannot receive.
- A RECEIVE-only handle cannot send.
- Rights granted to another process must be a subset of the source handle rights and require TRANSFER authority on the source.
- Queue overflow returns `QueueFull`; no packet/message is silently discarded.
- Empty receive returns `QueueEmpty`; Stage 8.2 never sleeps.
- FIFO ordering is preserved.
- Messages preserve sender process identity.
- Object references are incremented when peer handles are granted and decremented on close/process unregister.
- Stale generation-tagged handles remain invalid after close.
- Legacy PID-addressed IPC is preserved.

## Deliberate boundary
`grant_handle()` is kernel-mediated plumbing, not the final public userspace transfer ABI. General handle transfer is intentionally deferred to Stage 8.5 so the authority-transfer model can be designed and audited independently.

## Runtime proof
The Stage 8.2 runtime probe creates isolated synthetic SERVER and CLIENT namespaces, creates one endpoint, grants SEND to CLIENT and RECEIVE to SERVER, verifies denied reverse operations, FIFO order, sender identity, QueueEmpty, QueueFull, stale-handle rejection, and final object cleanup.

## Validation status
Static/source audit only in the packaging environment. The stage is not validated until `run-stage8-2-acceptance.ps1` passes on the user's Rust/QEMU environment for the preserved Stage 8.1 suite and Stage 8.2 markers on 1/2/4 CPU boots.

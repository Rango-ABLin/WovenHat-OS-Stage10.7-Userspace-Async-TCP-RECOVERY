# Stage 8.5 audit - Capability handle transfer through IPC

## Baseline
Exact Stage 8.4 shared-memory stack-fix candidate that passed build, strict
Clippy, host tests, 1 CPU x10, 2 CPU x30, 4 CPU x20, and network 1/2/4.

## Security model
A sender never transmits its numeric handle value. A Stage 8.5 message can carry
an internal `TransferEscrow { object, rights }`. The source handle must include
`TRANSFER`; requested rights must be non-zero, object-kind-valid, and a subset
of source rights. The queue retain pins the object after sender closure.

At receive, allocation in the receiver's handle table occurs before dequeue.
If allocation fails (notably `HandleTableFull`), the queue entry and its escrow
remain unchanged. On success, the escrow reference is consumed by the newly
installed receiver handle without a redundant retain/release pair.

## Lifetime/rollback invariants
- Queue-full send rolls back the pre-enqueue object retain.
- Endpoint destruction walks queued messages and releases all transfer escrows.
- Sender closing its source handle after enqueue does not invalidate the queued
  capability.
- Receiver gets a fresh generation-tagged local handle.
- Closing the received handle releases the escrow-derived ownership normally.

## Endpoint cycle prevention
Owning escrow references can otherwise form refcount cycles (A queue -> B and B
queue -> A). Stage 8.5 performs a bounded graph walk over existing endpoint
transfer edges before enqueuing another endpoint capability. Any edge that would
create a cycle is rejected with `AccessDenied`. Shared-memory objects do not own
endpoint queues and therefore do not participate in this graph.

## Compatibility
`Message` gains an optional receiver-local transferred handle and a
`transferred_handle()` accessor. Existing ordinary messages retain the same
payload/sender semantics and return `None`. Stage 8.3 blocking receive uses the
same dequeue path and therefore inherits correct transfer installation.

## Production runtime proof
The Stage 8.5 probe validates:
1. rights amplification denial;
2. self/cycle edge denial;
3. shared-memory transfer with reduced rights;
4. sender-close survival through escrow;
5. receiver-local handle creation and read-only enforcement;
6. endpoint capability transfer;
7. abandoned-message escrow teardown;
8. queue-full retain rollback;
9. full receiver handle-table atomicity and retry;
10. final zero-object/zero-shared-memory cleanup.

## Acceptance rule
Stage 8.5 is not validated until the complete Stage 8.4 acceptance suite passes
unchanged and the Stage 8.5 marker is present in 1/2/4 CPU production logs.

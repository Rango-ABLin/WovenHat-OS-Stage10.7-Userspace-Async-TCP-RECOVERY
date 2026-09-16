# Stage 8.1 — IPC Endpoint Object + Process-Local Handle Foundation

## Purpose

Stage 7.6 proved that WovenHat can execute and manage Ring-3 processes across 1, 2 and 4 CPUs. Stage 8 begins the service-oriented userspace architecture by separating **kernel IPC objects** from **process authority to those objects**.

Stage 8.1 is deliberately narrow. It does **not** replace the existing PID-addressed message ABI and it does not yet implement handle-addressed send/receive, blocking waits, handle transfer, or shared memory. Those belong to later Stage 8 milestones.

## Architecture

There are now three distinct concepts in `kernel/src/ipc.rs`:

1. **LegacyEndpoint** — the existing per-process PID-addressed FIFO queue used by syscalls 11/12. Preserved unchanged at the ABI boundary.
2. **EndpointObject** — a kernel-owned object with an opaque `EndpointObjectId`, immutable creator/owner identity, and reference count.
3. **HandleSpace** — a process-local table mapping opaque `Handle` values to endpoint objects plus rights.

A process registration creates both its legacy queue endpoint and its empty handle space atomically under the global IPC lock.

## Handle format and stale-handle safety

`Handle` is a 32-bit process-local value:

- low 8 bits: handle-table slot + 1 (`0` is invalid),
- upper 24 bits: generation.

Every reuse of a slot increments its generation. A closed handle therefore cannot alias a later object allocated into the same slot.

The numeric handle is not globally authoritative. Lookup always requires both the process owner PID and the handle value.

## Rights

Each handle records a rights mask. Stage 8.1 defines:

- `SEND`
- `RECEIVE`
- `TRANSFER`
- `INSPECT`

A newly-created endpoint receives `ENDPOINT_OWNER`, which contains all four rights. Stage 8.1 records and validates these bits; later stages will enforce them on handle-addressed operations.

## Lifecycle

### Process creation

Existing process creation paths already call `ipc::register(pid)`. Stage 8.1 extends that same registration to create an empty process handle space, preserving the existing scheduler/process-table integration.

### Endpoint creation

`create_endpoint_object(owner)`:

1. validates that the process has an IPC namespace,
2. reserves a global object slot,
3. assigns a new opaque object ID,
4. creates the object with one reference,
5. installs an owner-rights handle in the process handle space,
6. rolls the object allocation back if the handle table is full.

### Close

`close_handle(owner, handle)` verifies that the handle belongs to that process and that its generation matches. It removes the handle and decrements the object reference count. The object slot is destroyed at zero references.

### Process reap

`ipc::unregister(pid)` first detaches the process handle space, closes all remaining object references, and then removes the legacy endpoint. This preserves the existing Stage 7.2 zombie/reap lifecycle: IPC authority remains associated with the process until reap, then is deterministically removed.

## Locking

All Stage 8.1 object and handle operations are serialized by the existing global `IPC STATE` spin mutex. No new scheduler/process-table lock nesting is introduced.

This is intentionally conservative for the foundation. Later scalability work may shard endpoint queues/handle spaces, but correctness comes first.

## Validation

`handle_object_self_test()` proves locally:

- object creation,
- owner identity and rights,
- process-local handle isolation,
- stale generation rejection,
- close destroys the last object reference,
- process unregister closes all remaining handles and objects.

`stage8_1_runtime_probe()` repeats the object/handle lifecycle against the real global IPC state after SMP is online and cleans up synchronously.

Boot marker:

```text
[S8.1] kernel endpoint objects + process-local handles: PASSED
```

The Stage 8.1 acceptance wrapper first reruns the complete validated Stage 7.6 acceptance suite, then requires the Stage 8.1 marker in the final 1/2/4 CPU memory logs.

## Explicit non-goals

Stage 8.1 does not yet add:

- handle-addressed message send/receive,
- blocking receive/wait queues,
- cross-process handle transfer,
- shared-memory objects,
- service discovery,
- userspace endpoint-create/close syscalls.

Keeping these out makes the new object/authority lifecycle independently testable before concurrency and blocking semantics are layered on top.

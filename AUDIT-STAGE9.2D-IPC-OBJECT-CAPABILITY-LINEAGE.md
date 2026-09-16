# Stage 9.2D — IPC/Object Capability Lineage Audit

## Security invariant
A process-local numeric handle is not sufficient authority when it was delegated. The handle must
also carry a live WovenGuard IPC lineage. Revocation therefore invalidates authority without a mutable
sweep through every process handle table.

## Design
Creator handles are intrinsic (`lineage = None`). The first delegation of an endpoint or shared-memory
object creates a WovenGuard root containing only `Capability::Ipc`. Delegated handles, queued transfer
escrows and service-discovery handles carry that lineage. Object-specific `u8` rights still control
SEND/RECEIVE/TRANSFER/SHM_* operations; WovenGuard lineage controls whether the delegation epoch is live.

A single object delegation epoch is intentionally shared transitively by all downstream transfers in
Stage 9.2D. This makes recall immediate and bounded: revoking the epoch invalidates every delegated copy,
including copies that crossed a queue. It also avoids allocating one global lineage slot per transfer.
After recall, the intrinsic object owner can establish a fresh epoch by delegating again.

## Lifecycle
Revoked handles remain closable through generation-checked raw removal, preventing security revocation
from becoming an object-reference leak. Destroying an object releases a live delegation root. A revoked
root is already absent from the lineage registry, so destruction tolerates stale metadata.

## Runtime proof
`stage9_2d_runtime_probe()` proves:
1. direct endpoint grant works before recall and fails immediately after recall;
2. creator intrinsic authority survives recall;
3. queued shared-memory transfer installs a lineage-backed fresh handle;
4. recall invalidates the transferred handle without invalidating the creator handle;
5. service discovery creates lineage-backed clients;
6. recall blocks both existing service clients and future discovery;
7. cleanup returns the lineage registry to its baseline count.

## Scope boundary
Already-installed shared-memory page mappings are not synchronously unmapped in 9.2D. Doing so safely
requires paging/TLB work outside the IPC lock and a mapping-specific revocation protocol. 9.2D gates all
new handle-mediated shared-memory I/O/mapping operations; active mapping recall is reserved for the later
resource/memory capability-gate stage.

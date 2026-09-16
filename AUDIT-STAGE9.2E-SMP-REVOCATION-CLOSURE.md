# Stage 9.2E — SMP Revocation Stress and Lineage Closure

Stage 9.2E does not add a second revocation mechanism. It stress-proves the Stage 9.2B–9.2D production mechanism at the multicore boundary.

## Linearization contract

`revoke_object_delegations()` takes the IPC state lock, resolves the object's delegation root, and invokes WovenGuard's atomic subtree revocation. When the call returns successfully, that lineage generation is no longer live. A later handle lookup checks `lineage_authorizes()` and must reject it.

Operations that began before the revocation linearization point may complete. Operations initiated after return may not regain delegated authority.

## SMP proof

One worker is pinned to every online CPU. Each worker receives a lineaged shared-memory handle and must successfully use it before recall. The coordinator then revokes the shared object's delegation tree. After the revocation returns, each worker performs repeated read and inspection attempts and requires `AccessDenied` every time.

## Teardown and generation proof

Workers close revoked handles and unregister their IPC namespaces. The owner then repeatedly delegates to itself, revokes, closes, and recreates the delegated handle. The previous handle value must become `InvalidHandle` after slot reuse while the fresh generation works only until its own delegation epoch is revoked.

## Closure invariant

The probe succeeds only when IPC objects, shared-memory objects, services, and WovenGuard lineage count exactly match their pre-test baseline.

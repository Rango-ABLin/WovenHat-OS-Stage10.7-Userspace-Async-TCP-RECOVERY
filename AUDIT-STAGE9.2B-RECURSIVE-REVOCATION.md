# WovenHat OS Stage 9.2B Audit - Recursive Capability Revocation

## Scope
Stage 9.2B builds only on the Stage 9.2A generation-tagged lineage registry.
It does not yet bind every task capability or IPC handle to a lineage token;
that integration remains a later WovenGuard step. This milestone validates the
revocation primitive independently before widening its enforcement surface.

## Invariants
1. Revocation authorization follows ancestry ownership.
2. A target owner may revoke the target subtree.
3. An owner of a live ancestor may revoke authority descended from it.
4. An unrelated sibling owner receives PermissionDenied.
5. Subtree discovery and invalidation occur under one LINEAGES registry lock.
6. No descendant can remain valid after a successful subtree revocation.
7. Generation advancement makes pre-revocation LineageId values stale.
8. Sibling branches outside the subtree remain valid and usable.
9. Only the target branch is detached from a surviving parent.
10. Revocation is bounded and allocation-free.

## Algorithm
- Validate target generation/slot.
- Walk target -> ancestors (max 64) and authorize if actor owns any node.
- Snapshot a 64-element boolean revoke set before any mutation.
- Mark target plus every live descendant using bounded ancestry checks.
- Decrement the surviving boundary parent's direct-child count once.
- Clear every marked slot and advance its generation.
- Decrement registry count by the exact number revoked.
- Emit CapabilityLineageRevoke audit event.

## Runtime proof
The production boot creates two sibling branches and a three-node target
subtree. It checks unauthorized sibling revocation, ancestor-authorized subtree
revocation, stale descendant rejection, sibling preservation, post-revocation
derivation rejection, boundary child-count correctness, root teardown, slot
generation reuse, and zero metadata leakage.

## Acceptance policy
Stage 9.2B must preserve the complete Stage 9.2A acceptance chain and show its
new marker on 1/2/4 CPU production boots. Build, strict Clippy, unit tests,
1/2/4 CPU memory stress, and 1/2/4 CPU live networking therefore remain part
of the inherited acceptance gate.

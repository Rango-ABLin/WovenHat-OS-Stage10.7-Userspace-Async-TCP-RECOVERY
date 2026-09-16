WovenHat OS Stage 9.2B - Recursive Capability Revocation
========================================================

Purpose
-------
Stage 9.2B turns the validated Stage 9.2A lineage tree into an active security
control. A lineage owner or an owner of any live ancestor may revoke a target
lineage branch. The target and every descendant are invalidated atomically.
Unrelated sibling branches remain valid.

Security properties
-------------------
- Revocation is bounded by MAX_CAPABILITY_LINEAGES (64 entries).
- No heap allocation is required.
- The lineage registry spin::Mutex serializes authorization + invalidation.
- An owner may revoke its own token/subtree.
- An ancestor owner may revoke authority derived from that ancestor.
- Sibling owners cannot revoke each other's branches.
- Revoked IDs are generation-invalidated, so stale tokens fail lookup.
- Only the surviving boundary parent's direct-child count is adjusted.
- Internal child counts of revoked nodes never escape because those nodes are
  invalidated in the same transaction.
- Audit action CapabilityLineageRevoke records allow/deny outcome.

Public Stage 9.2B API
---------------------
revoke_lineage_subtree(actor, target) -> Result<usize, LineageError>

The returned usize is the number of lineage nodes invalidated.

Runtime validation topology
---------------------------
root(A)
|- branch(B)
|  `- leaf(D)
|     `- deep leaf(E)
`- sibling(C)

The proof verifies:
1. sibling C cannot revoke branch B;
2. root owner A revokes B + D + E as one subtree;
3. B/D/E become stale immediately;
4. derivation from revoked B is rejected;
5. root A and sibling C remain valid;
6. root direct-child count falls from 2 to 1;
7. sibling C can still derive and revoke its own child;
8. root revocation then removes root + remaining sibling;
9. reused storage has a new generation;
10. lineage metadata returns to its baseline count.

Focused validation
------------------
$env:CARGO_NET_OFFLINE = "true"
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\run-stage9-2b-focused-1cpu.ps1

Expected marker:
[S9.2B] WovenGuard recursive capability revocation: PASSED

Full acceptance
---------------
powershell.exe -NoProfile -ExecutionPolicy Bypass `
    -File .\run-stage9-2b-acceptance.ps1

Stage 9.2B is not validated until the full acceptance script reports:
=== STAGE 9.2B ACCEPTANCE: PASS ===

# Stage 9.2A Audit — Capability Lineage Foundation

## Security objective
Create a bounded, allocation-free ancestry primitive for authority delegation so
future revocation can invalidate descendants without guessing where authority
came from.

## Invariants
- A `LineageId` is generation-tagged; stale IDs fail after slot reuse.
- A derived token can never contain rights absent from its parent.
- Empty derived authority is rejected.
- Only the current token owner can derive from or release that token.
- A parent cannot be released while direct/indirect descendants remain because
  every live descendant keeps the ancestry chain connected through child counts.
- Lineage storage is fixed-capacity and protected by one `spin::Mutex`.
- No recursive revocation exists in this stage; release is leaf-only cleanup.

## Why revocation is deferred
Ancestry and revocation are separated on purpose. Stage 9.2A proves identity,
generation safety, subset rules and lifecycle bookkeeping. Stage 9.2B can then
add recursive invalidation without simultaneously debugging the lineage model.

## Compatibility
The Stage 9.1 task capability bitset remains authoritative in this stage. The new
lineage registry is a security primitive that is validated independently before
being bound into all task/IPC delegation paths. Therefore the Stage 8.7 and 9.1
behavioral baseline should remain unchanged.

WovenHat OS Stage 9.2A - Capability Lineage Foundation
======================================================

Purpose
-------
Stage 9.2A builds the identity/ancestry primitive needed before WovenGuard can
perform safe recursive revocation. It intentionally does NOT revoke descendant
capabilities yet. First we prove that capability ancestry itself is bounded,
rights-reducing, stale-safe and SMP-safe.

Implemented
-----------
1. Generation-tagged LineageId
   - low byte: slot+1
   - upper 24 bits: generation
   - stale IDs cannot silently refer to a reused slot

2. Bounded fixed-capacity lineage registry
   - MAX_CAPABILITY_LINEAGES = 64
   - spin::Mutex protected for SMP safety
   - no heap allocation
   - deterministic TableFull behavior

3. Capability-set lineage nodes
   Each node records:
   - parent LineageId (or None for a root)
   - owner identity
   - CapabilitySet rights
   - direct child count

4. Rights-reducing derivation
   - derived rights must be non-empty
   - child rights must be a subset of parent rights
   - only the current owner may derive from a token
   - rights amplification is rejected

5. Ancestry queries
   - bounded parent walk
   - supports parent -> child -> grandchild proofs

6. Deterministic lifecycle cleanup
   - only leaf tokens may be released in Stage 9.2A
   - parent release is blocked while descendants exist
   - releasing a child decrements the parent's child count
   - slot reuse advances generation

7. Audit hooks
   - CapabilityLineageIssue
   - CapabilityDerived
   - CapabilityLineageRelease

8. Production runtime proof
   Validates:
   - two-level ancestry
   - reduced-rights derivation
   - anti-amplification
   - wrong-owner rejection
   - parent lifetime protection
   - leaf-first cleanup
   - stale-token rejection
   - generation change on slot reuse
   - zero lineage leakage after test

Not in Stage 9.2A
-----------------
- recursive descendant revocation
- binding every TaskControlBlock capability bit to a lineage token
- capability epoch invalidation
- IPC handle/capability-escrow revocation
- service identity / signed software
- sandbox profiles
- device capability gates
- persistent security ledger

These are deliberately deferred. Stage 9.2B will add recursive revocation after
this ancestry primitive is validated.

Validation order
----------------
First run:

  $env:CARGO_NET_OFFLINE = "true"
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-2a-focused-1cpu.ps1

After focused 1 CPU passes, run direct 2 CPU and 4 CPU memory boots and verify the
S9.2A marker. Then run:

  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-2a-acceptance.ps1

Required final marker:

  === STAGE 9.2A ACCEPTANCE: PASS ===

Do not freeze/tag Stage 9.2A until the complete acceptance chain passes.

WovenHat OS Stage 9.2C - Real Task Authority Enforcement
=======================================================

Purpose
-------
Stage 9.2A created generation-tagged capability lineages.
Stage 9.2B added atomic recursive subtree revocation.
Stage 9.2C connects those security identities to real task capability checks.

Key behavior
------------
1. Each TCB has a fixed, allocation-free per-capability lineage binding.
2. Intrinsic/bootstrap capability: capability bit + no lineage binding.
3. Delegated capability: capability bit + live lineage binding.
4. current_has() now evaluates effective authority. A stale/revoked lineage denies use even if the raw bit is still set.
5. grant() creates/uses a lineage root and derives a rights-reducing child token for the target task.
6. revoke() invalidates the delegated subtree, clears the target bit/binding, and releases an empty temporary root.
7. Fork copies lineage bindings with inherited capability state, preventing a fork from laundering delegated authority into an untracked bit.
8. Reaping a dead task recalls lineage nodes owned by that task and their descendants.

Security boundary
-----------------
This stage binds WovenGuard to TASK capabilities only.
IPC handle/service/shared-memory capability lineage integration is intentionally left for the next enforcement stage.

Validation
----------
Focused:
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-2c-focused-1cpu.ps1

Full:
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-2c-acceptance.ps1

Expected marker:
  [S9.2C] WovenGuard task capability lineage enforcement: PASSED

BUILD CLEANUP:
- Removed obsolete, unreferenced paging::protect_user_range_in() wrapper.
- The ELF loader continues to use protect_user_range_in_inactive() for unpublished CR3 construction.
- No warning suppression was added; strict -D warnings remains authoritative.

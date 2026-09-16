WovenHat OS Stage 9.2D — IPC/Object Capability Lineage

Purpose
-------
Bind delegated Stage 8 IPC object authority to WovenGuard lineage provenance.

Implemented
-----------
* Process-local HandleEntry carries optional WovenGuard LineageId.
* Intrinsic creator handles remain unbound and retain owner authority.
* First delegation creates one object delegation epoch rooted in Capability::Ipc.
* Direct grant_handle installs a lineage-backed target handle.
* send_handle_with_transfer stores lineage in queue escrow; receive installs the same
  revocation identity in the fresh receiver-local handle, preventing authority laundering.
* Service publication records the endpoint delegation lineage; discovery installs lineage-backed
  client handles and refuses discovery after revocation.
* Effective handle lookup rejects stale/revoked lineage with AccessDenied.
* close_handle uses raw generation-safe lookup so revoked handles can still be closed and objects reclaimed.
* revoke_object_delegations(owner, owner_handle) recalls the current delegation epoch while leaving
  the creator's intrinsic owner handle usable. A later delegation starts a fresh epoch.
* Endpoint/shared-memory destruction releases a still-live delegation root.
* Object-specific u8 HandleRights remain separate from WovenGuard CapabilitySet.

Scope boundary
--------------
Stage 9.2D revokes handle-mediated authority. Existing already-installed shared-memory page mappings
are not synchronously torn down here because mapping teardown crosses paging/TLB synchronization and
must not be performed while the IPC state lock is held. Active mapping recall belongs in the later
resource/memory capability gate work; new mapping/read/write operations through a revoked handle are denied now.

Validation
----------
Run:
  $env:CARGO_NET_OFFLINE = "true"
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-2d-acceptance.ps1

Required final marker:
  === STAGE 9.2D ACCEPTANCE: PASS ===

WovenHat OS Stage 9.2E — SMP Revocation Stress + Lineage Closure

Purpose
-------
Close the Stage 9.2 capability-provenance layer under real SMP contention.

What is proved
--------------
* Every online CPU actively uses a delegated shared-memory capability before recall.
* The owner recursively revokes the object's WovenGuard delegation lineage once.
* After revoke_object_delegations() returns, all CPUs must observe AccessDenied.
* Revoked handles remain closable so teardown cannot leak references.
* Repeated grant/revoke/close cycles reuse handle + lineage slots safely.
* Old handle generations cannot resurrect authority after slot reuse.
* Intrinsic owner authority survives delegated-tree recall.
* IPC object, shared-memory, service, and WovenGuard lineage counts return to baseline.

Acceptance
----------
Run:
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-2e-acceptance.ps1

Required marker:
  [S9.2E] WovenGuard SMP revocation + lineage closure: PASSED

WovenHat OS Stage 9.1 - WovenGuard Domain + Policy Foundation
=============================================================

Purpose
-------
Stage 9.1 introduces the first enforceable WovenGuard security layer on top of
Stages 7-8. It deliberately does not attempt the whole security roadmap at once.

Implemented
-----------
1. Security domains stored directly in each TaskControlBlock:
   - Kernel
   - SystemService
   - User
   - Restricted

2. Domain assignment:
   - bootstrap kernel -> Kernel
   - kernel workers/services -> SystemService
   - Ring-3 tasks -> User
   - idle/minimal infrastructure -> Restricted
   - fork children inherit the parent's domain

3. Least-privilege domain ceilings:
   - Kernel: unrestricted bootstrap ceiling
   - SystemService: cannot receive raw InterruptControl or MemoryInspect
   - User: Console/FileRead/FileWrite/Ipc/ProcessCreate only
   - Restricted: TimerRead only

4. Central grant authorization:
   A capability grant now requires:
   - actor owns TaskControl
   - actor already owns the capability being delegated
   - target security domain permits that capability
   - non-kernel actors cannot grant into Kernel domain

5. Revocation authorization:
   - requires TaskControl
   - revocation cannot amplify authority

6. Audit integration:
   - WovenGuardAllow / WovenGuardDeny events are emitted
   - existing CapabilityGrant / CapabilityRevoke events are preserved

7. Security hardening:
   - generic kernel task initialization explicitly clears stale capabilities
     before assigning the SystemService domain.

Not in Stage 9.1
----------------
- dynamic capability revocation across IPC handle graphs
- security-domain transition API
- sandbox profiles
- resource quotas
- signed service identities/software
- device capability gates
- persistent security ledger
- secure/measured boot

Those belong to later Stage 9 sub-stages.

Validation order
----------------
First run the focused gate:

  $env:CARGO_NET_OFFLINE = "true"
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-1-focused-1cpu.ps1

Only after the focused gate passes, run full acceptance:

  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-1-acceptance.ps1

Required final marker:

  === STAGE 9.1 ACCEPTANCE: PASS ===

Do not freeze/tag Stage 9.1 until the complete acceptance chain passes.

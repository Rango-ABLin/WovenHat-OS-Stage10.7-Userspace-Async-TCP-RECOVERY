# Stage 9.4A — Sandbox Profile & Capability-Ceiling Foundation

## Objective
Introduce a per-task WovenGuard sandbox profile below the Stage 9.1 SecurityDomain ceiling.

## Security model
Effective authority now requires all three conditions:

1. the task owns the raw capability bit;
2. any delegated lineage is still live; and
3. the bound sandbox profile permits that capability.

A sandbox profile is valid only when its capability ceiling is a subset of the task's SecurityDomain ceiling.

## Binding semantics
A TaskControl-authorized controller may bind a valid profile. Tightening is destructive: capabilities and lineage bindings outside the new ceiling are removed/revoked so later profile changes cannot resurrect hidden authority. Bind and deny outcomes are emitted to the Stage 9.3 security ledger.

## Fork semantics
ForkSecurityContext copies the SandboxProfile together with capabilities, lineage bindings and SecurityDomain. A fork child therefore cannot escape its parent's sandbox ceiling.

## Runtime proof
The production boot probe temporarily binds the idle task to an empty Restricted-valid profile, verifies TimerRead delegation is denied, verifies an invalid broader User profile is rejected for the Restricted domain, restores the Restricted profile, proves TimerRead delegation works again, revokes it, and checks the baseline profile is restored.

## Validation
Run `run-stage9-4a-acceptance.ps1`. Stage 9.4A is not frozen until the full inherited Stage 9.3 suite and the 1/2/4 CPU Stage 9.4A marker pass.

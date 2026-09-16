# Stage 9.1 Audit — WovenGuard Domain + Policy Foundation

## Goal

Establish the first central, enforceable WovenGuard policy layer without
changing the validated Stage 8 IPC ABI.

## Security model

A task now has two independent security properties:

1. `SecurityDomain` — maximum class of authority the task is eligible to hold.
2. `CapabilitySet` — authority the task currently holds.

This separation prevents a privileged actor from simply copying any capability
into an inappropriate target. The grant must pass the target domain ceiling.

## Domain assignments

- `Kernel`: bootstrap task only.
- `SystemService`: kernel workers/services created by the generic kernel-task path.
- `User`: Ring-3 processes.
- `Restricted`: idle/minimal infrastructure.

Fork preserves the parent's security domain as well as its capability set.

## Delegation checks

`task::grant()` now delegates authorization to `wovenguard::authorize_grant()`.
A successful grant requires all of:

- actor has `TaskControl`;
- actor already owns the delegated capability;
- target domain permits the capability;
- a non-Kernel actor cannot inject authority into the Kernel domain.

Denied grants do not mutate the target capability set.

## Audit behavior

Every known-target grant/revoke decision records a WovenGuard allow/deny audit
event followed by the existing capability grant/revoke audit event. This keeps
older audit invariants compatible while adding explicit policy-decision evidence.

## Regression-sensitive change

Generic kernel-task initialization now clears the capability set before assigning
`SystemService`. This prevents a reused TCB slot from retaining capabilities from
an earlier task.

## Runtime proof

Production boot requires:

`[S9.1] WovenGuard domains + least-privilege policy: PASSED`

The Stage 9.1 acceptance script first executes the complete Stage 8.7 acceptance
suite and then checks the Stage 9.1 marker on 1, 2 and 4 CPU boots.

## Validation status

IMPLEMENTED CANDIDATE — not validated until user/QEMU acceptance passes.

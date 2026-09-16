# WovenHat OS Stage 9.4B — IPC / Service Exposure Policy

## Goal
Stage 9.4B extends the validated Stage 9.4A sandbox profile from a capability ceiling into a named-service exposure policy. A task can hold generic `Capability::Ipc` authority while still being denied access to classes of services it does not need.

## Security model
Effective service use now requires both generic IPC authority at the task/syscall layer and an allowed service class in the task's current sandbox profile. Service classes are allocation-free policy buckets: Core, Network, Storage, UI, AI, Application, and Test.

Existing Stage 9.4A profiles default to `ServicePolicy::ALL`, preserving all validated Stage 8/9 behavior. New application/service profiles can narrow publication and discovery independently without changing the global IPC ABI.

## Enforcement points
- `publish_service`: validates the publisher's live TCB sandbox service-publish mask before registry mutation.
- `discover_service`: validates the client's live service-discovery mask before a handle is returned.
- service-derived handles carry `ServiceClass` metadata.
- direct handle grants preserve that metadata and reject grants into a real task whose current profile denies the class.
- queued capability transfer preserves the service-class tag.
- `send_handle`, `send_handle_with_transfer`, and `send_handle_blocking` re-check the live sandbox profile before use of a service-derived handle.
- `close_handle` and `unpublish_service` remain teardown-safe even after a profile is tightened.

This recheck is intentional: a profile change must take effect immediately. WovenGuard does not rely only on the policy that existed when a service handle was first discovered.

## Audit ledger
Stage 9.3 is extended with:
- `SandboxServicePublish`
- `SandboxServiceDiscover`
- `SandboxServiceUse`

The ledger target is a stable FNV-1a hash of the service name for publish/discovery events. The detail field packs the sandbox profile ID and service class. Service-handle use records the handle value plus the same profile/class detail.

## Production runtime proof
The Stage 9.4B boot probe uses the real idle TCB and real IPC service registry, then restores the Stage 9.4A baseline. It proves:
1. a Network-only service policy may publish a Network service;
2. the same profile cannot publish an AI service;
3. it can discover and send through the allowed Network service;
4. it cannot discover an AI service;
5. after tightening the live profile to `ServicePolicy::NONE`, the already-open Network service handle immediately returns `AccessDenied` on send;
6. the deny is visible as the latest `SandboxServiceUse` ledger event;
7. the sandbox profile, service count, IPC namespace, and object count return to baseline.

Production marker:
`[S9.4B] WovenGuard IPC/service exposure policy: PASSED`

## Acceptance
`run-stage9-4b-acceptance.ps1` first executes the full Stage 9.4A acceptance chain, then requires the Stage 9.4B production marker in 1-, 2-, and 4-CPU memory regression logs.

## Scope boundary
Stage 9.4B governs named service exposure and service-derived IPC handles. Filesystem/resource path policy remains Stage 9.4C. Device-resource capability gates remain Stage 9.5.

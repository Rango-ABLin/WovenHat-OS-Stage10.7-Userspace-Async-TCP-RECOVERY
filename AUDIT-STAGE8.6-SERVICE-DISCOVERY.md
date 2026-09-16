# Stage 8.6 Audit — Named Service Registry + Capability Discovery

## Baseline
Built directly on the validated Stage 8.5 capability-handle-transfer tree.
No Stage 8.1–8.5 acceptance criteria are weakened.

## Problem
Stage 8.5 can securely pass endpoint and shared-memory capabilities, but a client
still needs an out-of-band way to learn which endpoint represents a system
service. Raw PIDs are not stable capability identities and would couple clients
to process placement/lifetime.

## Design
The IPC `State` now includes a bounded service registry. Each entry contains:

- bounded `ServiceName` (max 32 bytes),
- publisher/owner process ID,
- underlying endpoint `ObjectRef`,
- exact rights discoverers may receive.

Publishing retains one endpoint object reference. Discovery retains the object
again and allocates a fresh generation-tagged process-local handle. If handle
allocation fails, the retain is rolled back. Unpublishing or publisher teardown
releases the registry-held reference.

## New right
`HandleRights::PUBLISH_SERVICE` uses bit 7. Endpoint owners receive it with the
owner handle. A process holding only SEND/INSPECT cannot publish that endpoint.
Capability transfer may delegate PUBLISH_SERVICE only when the sender actually
possesses it and explicitly transfers that right; Stage 8.5 subset checks still
prevent rights amplification.

## Client rights ceiling
`HandleRights::SERVICE_CLIENT = SEND | TRANSFER | INSPECT`.
The registry rejects publication configurations containing RECEIVE or
PUBLISH_SERVICE. Discovery therefore cannot mint a second receiver/server or a
new publisher authority merely from a service lookup.

## Namespace semantics
Names must be non-empty, at most 32 bytes, and contain only ASCII alphanumeric,
'.', '-', or '_'. Registered names are globally unique. The registry is bounded
by `MAX_IPC_SERVICES = 32` and remains allocation-free.

## Process teardown
`State::unregister(owner)` removes all service entries published by `owner` and
releases their escrow references before draining ordinary process handles. This
prevents stale discoverable services after process exit.

## Runtime probe
The Stage 8.6 production probe verifies:

1. invalid/unsafe names are rejected;
2. RECEIVE cannot be delegated through service discovery;
3. a restricted endpoint lacking PUBLISH_SERVICE cannot be registered;
4. a valid service publishes successfully;
5. service metadata reports exact owner/object/client rights;
6. duplicate global names are rejected;
7. discovery returns a fresh client-local handle to the same endpoint object;
8. the discovered handle cannot receive;
9. a discovered client can send and the server receives the exact payload;
10. registry escrow keeps the service alive after the source handle closes;
11. non-owner unpublish is rejected;
12. unpublish removes discovery and releases its registry reference;
13. publisher process teardown automatically removes a service whose original
    endpoint handle was already closed;
14. final service/object counts return to their pre-probe baselines.

## Non-goals
Stage 8.6 does not yet add:

- a userspace syscall ABI for service registry operations;
- hierarchical namespaces or per-user namespaces;
- ACLs/policies beyond capability rights;
- service version negotiation;
- subscriptions to service arrival/departure;
- persistent service manifests;
- sustained concurrent SMP discovery stress (reserved for Stage 8.7).

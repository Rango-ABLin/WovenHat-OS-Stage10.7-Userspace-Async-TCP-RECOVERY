WovenHat OS Stage 8.6 — Named Service Registry + Capability Discovery
=====================================================================

Status: IMPLEMENTED CANDIDATE — requires Windows/QEMU acceptance before validation.
Baseline: exact validated Stage 8.5 capability-handle-transfer package.

Purpose
-------
Stage 8.6 gives processes a bounded, capability-controlled way to find services by
name without hard-coding process IDs. A service publisher registers an endpoint
under a short global name and chooses the exact client-side endpoint rights that
discovery may mint.

Core API
--------
publish_service(owner, name, endpoint_handle, client_rights)
unpublish_service(owner, name)
discover_service(client, name) -> fresh process-local Handle
service_info(name)
service_count()

Security / lifetime model
-------------------------
* Names are 1..32 ASCII characters using letters, digits, '.', '-', '_'.
* Names are globally unique while registered.
* Only endpoint handles carrying PUBLISH_SERVICE may publish.
* Discovery may only mint SERVICE_CLIENT rights: SEND | TRANSFER | INSPECT.
  RECEIVE and PUBLISH_SERVICE remain server-side authority.
* The service registry holds an escrow reference to the endpoint, so discovery
  remains valid after the publisher closes the original source handle.
* Unpublish releases the registry reference.
* Process unregister automatically removes all services published by that owner.
* Each discovery creates a fresh generation-tagged handle in the caller's own
  handle table. Allocation failure rolls back the temporary object retain.

Production boot marker
----------------------
[S8.6] named service registry + capability discovery: PASSED

Acceptance
----------
Run from the extracted package root:

  $env:CARGO_NET_OFFLINE = "true"
  powershell.exe -NoProfile -ExecutionPolicy Bypass `
      -File .\run-stage8-6-acceptance.ps1

The script first reruns the full validated Stage 8.5 acceptance suite, including
build, strict clippy, host tests, 1/2/4 CPU memory regressions and 1/2/4 CPU live
network regression, then checks the Stage 8.6 marker on all production boots.

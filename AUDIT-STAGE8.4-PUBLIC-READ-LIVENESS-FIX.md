# Stage 8.4 Public Shared-Memory Read Liveness Fix

## Failure observed
The first Stage 8.4 acceptance build stopped under the existing strict `-D warnings` policy because `ipc::read_shared_memory()` was public but not exercised by production code, so rustc reported it as dead code.

## Fix
The Stage 8.4 production runtime probe now calls `read_shared_memory()` immediately after `write_shared_memory()` and verifies the returned bytes. The existing independent peer-address-space read remains in place.

This intentionally does **not** add `allow(dead_code)` / `expect(dead_code)` and does not weaken the build or Clippy policy.

## Added proof
The Stage 8.4 runtime probe now proves both:
1. kernel object-layer write -> kernel object-layer read, and
2. the same backing bytes -> peer user mapping visibility.

No scheduler, IPC wait, mapping, rights, timeout, or earlier-stage semantics were changed.

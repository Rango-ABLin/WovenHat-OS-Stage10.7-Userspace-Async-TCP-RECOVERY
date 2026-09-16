# Stage 8.1 Public API Liveness Fix

## Failure
The first Stage 8.1 candidate failed the project's strict `-D warnings` build because the new public endpoint/handle APIs were only exercised through private `State` methods inside the runtime probe. Rust therefore reported the public wrappers and ID accessors as dead code.

## Correction
`stage8_1_runtime_probe()` now exercises the production public API surface directly:
- `create_endpoint_object()`
- `handle_info()`
- `close_handle()`
- `handle_count()`
- `object_count()`
- `Handle::as_u32()`
- `EndpointObjectId::as_u64()`

The probe still verifies object creation, rights/ownership, handle accounting, close behavior, stale-handle rejection, generation change on slot reuse, and unregister-driven teardown.

## Non-changes
No `#[allow(dead_code)]` or `#[expect(dead_code)]` was added. No scheduler, SMP, process lifecycle, legacy PID IPC ABI, object allocation, handle generation, rights model, or teardown semantics were changed.

## Validation status
Statically audited and packaged. Full Rust/Clippy/QEMU validation must run on the user's configured Windows toolchain through `run-stage8-1-acceptance.ps1`.

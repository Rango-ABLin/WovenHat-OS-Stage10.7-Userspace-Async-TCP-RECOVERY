# WovenHat OS Stage 8.1 Audit — IPC Handle Foundation

## Baseline

Implemented on the Stage 7.6 placement-snapshot tree that passed the complete Stage 7.6 acceptance suite (build, strict Clippy, host tests, 1CPU x10, 2CPU x30, 4CPU x20, network 1/2/4).

## Changed production files

- `kernel/src/config.rs`
  - `MAX_IPC_OBJECTS = 64`
  - `MAX_IPC_HANDLES_PER_PROCESS = 16`
- `kernel/src/ipc.rs`
  - preserves legacy PID queue ABI,
  - adds endpoint objects, generation-tagged process handles, rights, lifecycle, tests.
- `kernel/src/main.rs`
  - adds local Stage 8.1 invariant gate,
  - adds real global-state Stage 8.1 post-SMP runtime probe marker.

No Stage 7 scheduler logic is modified.

## Key invariants

1. A registered process has both a legacy queue endpoint and handle space.
2. Endpoint object IDs are kernel-issued and never supplied by userspace.
3. A handle is meaningful only inside its owner process handle space.
4. Handle slot reuse changes generation; stale handles fail lookup.
5. Object allocation rolls back if handle installation fails.
6. Closing the last reference removes the endpoint object.
7. Process unregister closes all remaining handles before namespace removal.
8. Existing PID message queues continue to pass their original self-test.
9. Global runtime probe leaves zero synthetic resources behind.

## Locking

Stage 8.1 introduces no new cross-subsystem lock order. All object/handle metadata stays beneath the existing IPC `spin::Mutex<State>`.

## Validation status

Static source audit completed in the packaging environment. Rust/Cargo is not installed there, so compiler, strict Clippy and QEMU validation must be performed on Anthony's Windows WovenHat toolchain with `run-stage8-1-acceptance.ps1`.

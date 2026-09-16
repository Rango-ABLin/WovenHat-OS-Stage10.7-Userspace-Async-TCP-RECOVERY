# Stage 8.2 Clippy op_ref Fix

## Failure
Strict `cargo clippy -D warnings` rejected one Stage 8.2 runtime-probe comparison in `kernel/src/ipc.rs` because the right-hand array operand was unnecessarily borrowed.

## Change
Changed:

```rust
message.payload() != &[index as u8]
```

to:

```rust
message.payload() != [index as u8]
```

This follows Clippy's `op_ref` guidance and relies on Rust's slice/array `PartialEq` implementation.

## Scope
No IPC object, handle, rights, queue, refcount, scheduler, SMP, network, or lifecycle semantics changed. This is compile/lint hygiene only.

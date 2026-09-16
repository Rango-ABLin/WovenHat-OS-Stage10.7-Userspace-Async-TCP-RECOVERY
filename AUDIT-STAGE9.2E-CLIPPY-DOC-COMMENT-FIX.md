# Stage 9.2E Clippy Documentation Placement Fix

## Failure
Strict `cargo clippy -- -D warnings` rejected `kernel/src/ipc.rs` with
`clippy::empty_line_after_doc_comments`.

## Root cause
The Stage 9.2D documentation comment was left above the newly inserted Stage
9.2E constants. Rust therefore attached that `///` documentation to
`STAGE9_2E_OWNER` instead of `stage9_2d_runtime_probe()`.

## Fix
The Stage 9.2D documentation block is now immediately adjacent to
`stage9_2d_runtime_probe()`. No lint suppression was added and no runtime,
security, stress, timeout, or acceptance semantics were changed.

## Validation policy
Run the unchanged `run-stage9-2e-acceptance.ps1`. Stage 9.2E remains pending
until that complete acceptance suite passes.

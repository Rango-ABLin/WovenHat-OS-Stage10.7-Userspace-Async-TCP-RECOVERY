# Stage 9.4A build fix

The first acceptance run stopped during `cargo build` because `-D warnings` rejected two unused Stage 9.4A helpers: `SandboxProfile::ceiling()` and `default_sandbox()`.

Fix:
- Removed the unnecessary public `ceiling()` getter; policy continues to use `allows()` and `is_valid_for()`, avoiding exposure of a raw ceiling accessor that had no production caller.
- Wired `default_sandbox(domain)` into real task/scheduler initialization so the canonical domain-to-default-profile mapping is used rather than duplicated constants.
- No lint suppression, acceptance weakening, capability-policy change, or runtime timeout change.

Re-run the unchanged `run-stage9-4a-acceptance.ps1`.

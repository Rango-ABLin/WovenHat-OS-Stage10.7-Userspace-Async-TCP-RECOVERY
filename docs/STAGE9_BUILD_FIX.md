# WovenHat OS 0.7.0 Stage 9 — Build Fix

Fixes the compiler failures reported after consolidating Stages 7–9.

- Converts literal `\n` source tokens around Stage 7–9 `global_asm!` blocks into real Rust source newlines.
- Removes the duplicate `process_count()` definition while retaining the Stage 8 process-observation implementation.
- Retains the Ring-3 argv stack fix using `program.image.stack_top`.
- Keeps `warnings = "deny"`.

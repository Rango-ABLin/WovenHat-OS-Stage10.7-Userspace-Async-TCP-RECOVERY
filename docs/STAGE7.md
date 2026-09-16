# WovenHat OS 0.5.0 Stage 7 — Process Environment

Stage 7 adds a bounded per-process environment table that is inherited across fork/spawn.
Default entries are `PATH=/bin`, `HOME=/`, `SHELL=/bin/sh`, and `TERM=wovenhat`.

New syscalls:
- 51 EnvGet
- 52 EnvSet
- 53 EnvCount
- 54 EnvEntry

New Ring-3 utility: `/bin/env`.

Validation: `cargo build`, boot, `userland` -> 19/19, `sh`, then `env`.

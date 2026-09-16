# Stage 9.4C SMP File-Authority Fix

Observed acceptance evidence: build/clippy/host tests passed; 1 CPU x10 passed; 2 CPU passed runs 1-19, then run 20 timed out.

Correction:
- file descriptor read/write/seek now evaluate effective FileRead/FileWrite authority and the current FilePolicy under one SCHEDULER lock acquisition;
- file-backed mmap snapshots effective read/write authority and SandboxProfile atomically before taking PROCESS_TABLE;
- msync uses the same atomic file-authority snapshot;
- no timeout, stress-count, warning, or acceptance requirement was weakened;
- filesystem scope semantics and the Stage 9.4C production marker are unchanged.

This closes a TOCTOU window between capability and sandbox-profile checks and reduces scheduler-lock contention in SMP file-I/O paths.

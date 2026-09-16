# WovenHat OS 0.6.0 Stage 8 — Process Observability

Stage 8 adds a stable process snapshot ABI for Ring-3 diagnostics.

New syscalls:
- 55 ProcessCount
- 56 ProcessInfo

New utilities:
- `/bin/ps` — PID, PPID and state code
- `/bin/uptime` — scheduler timer ticks since boot

Validation: `userland` -> 21/21, `sh`, `ps`, `uptime`.

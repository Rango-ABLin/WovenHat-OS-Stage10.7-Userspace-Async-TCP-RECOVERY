WovenHat OS Stage 10.7 — Userspace Asynchronous TCP

Adds owner-bound asynchronous TCP connect to the Stage 10.6 async networking
worker and hardens TCP send/receive readiness semantics. Ring-3 uses syscall 81
(AsyncTcpConnect) followed by the existing async network send/receive/poll/wait
ABI. In-flight operations pin generation-tagged sockets, so close/reuse cannot
redirect work. TCP receive completes successfully with value=0 on remote EOF.

Acceptance preserves the complete Stage 10.6 chain and then runs isolated real
host<->guest TCP tests on 1/2/4 CPUs. The host server validates two 16-byte
transmissions, returns a 16-byte payload on the second connection, then closes
its write side so WovenHat must observe EOF. Separate unreachable connections
exercise deterministic cancellation and owner teardown.

Run:
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\RUN-STAGE10.7.ps1

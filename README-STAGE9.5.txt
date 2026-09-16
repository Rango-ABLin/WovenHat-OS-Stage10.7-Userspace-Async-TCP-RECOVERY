WovenHat OS Stage 9.5 — Resource / Device Capability Gates

Baseline: fully validated Stage 9.4D Sandbox Lifecycle + SMP Closure.

Stage 9.5 decomposes broad device authority into explicit resource classes:
- NetworkIo
- StorageIo
- DisplayIo
- InputIo
- legacy Generic DeviceIo

WovenGuard now combines:
1. SecurityDomain capability ceiling
2. current effective capability / lineage authority
3. SandboxProfile capability ceiling
4. SandboxProfile DevicePolicy class mask

Real enforcement begins on networking: socket, bind, connect, send, recv,
net-info, DNS, DHCP and ping syscalls require the Network device gate.
Socket close remains permitted for teardown safety.

Normal userspace receives NetworkIo so validated networking behavior remains
compatible. Direct StorageIo/DisplayIo/InputIo are not in the User domain and
are intended for controlled system services as the driver architecture grows.

Acceptance:
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\RUN-STAGE9.5.ps1

Expected final marker:
  [S9.5] WovenGuard resource/device capability gates: PASSED
on 1, 2 and 4 CPU production boots.

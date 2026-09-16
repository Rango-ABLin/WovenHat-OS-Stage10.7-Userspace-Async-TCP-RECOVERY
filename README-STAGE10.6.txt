WovenHat OS Stage 10.6 — Userspace Asynchronous Networking Foundation

Run from this directory:
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\RUN-STAGE10.6.ps1

The runner first preserves the complete accepted Stage 10.5 chain, then boots the dedicated Stage 10.6 asynchronous UDP/socket probe with 1, 2, and 4 CPUs. The host harness injects real UDP bytes into the guest and requires the Stage 10.6 marker plus QEMU success exit.

Expected final marker:
  === STAGE 10.6 ACCEPTANCE: PASS ===

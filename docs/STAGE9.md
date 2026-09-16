# WovenHat OS 0.7.0 Stage 9 — Ring-3 TCP Service

Stage 9 proves the TCP server side of the existing process-owned socket ABI.

New utility: `/bin/tcpd [port]` (default 8080). It creates a TCP socket, binds/listens, waits cooperatively, and echoes received bytes back to the peer while mirroring them to stdout.

For a host-side QEMU test, add `hostfwd=tcp::8080-:8080` to the user-mode netdev and connect to localhost:8080.

Validation: `userland` -> 22/22, `sh`, `tcpd 8080`.

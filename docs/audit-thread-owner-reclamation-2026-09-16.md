# Thread owner reclamation audit — 2026-09-16

Stage 11.2 now builds the generation-safe thread registry in the normal kernel
configuration. Process termination calls `reap_owner`, which clears TLS,
errno, exit state and ownership for every record and advances its generation.
The structural gate verifies that a stale handle cannot access a replacement
thread record.

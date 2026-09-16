WovenHat OS Stage 8.1 — IPC Endpoint Object + Process-Local Handle Foundation

Base: validated Stage 7.6 Integrated Multicore Userspace Closure.

Adds:
- kernel-owned endpoint object IDs,
- per-process generation-tagged handle spaces,
- explicit IPC rights metadata,
- stale-handle rejection,
- deterministic close and reap cleanup,
- local invariant test plus post-SMP production-state runtime probe.

Compatibility:
- existing PID-addressed IPC MessageSend/MessageReceive remains intact,
- no Stage 7 scheduler, affinity, pager, block-I/O, fork, or networking semantics changed.

Required validation:
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage8-1-acceptance.ps1

Stage 8.1 is not frozen until that script prints:
  === STAGE 8.1 ACCEPTANCE: PASS ===

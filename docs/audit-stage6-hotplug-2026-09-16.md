# Stage 6 bounded CPU-offline audit — 2026-09-16

The Stage 6 SMP control plane now supports a bounded, one-way offline
transition for the highest-numbered online AP. The request is accepted only
for a contiguous online prefix and only after the scheduler evacuates every
non-running task that has a legal online destination. The AP then reaches its
CPU-owned idle checkpoint, masks its local timer, parks with interrupts
disabled, and publishes the reduced online mask and count before the requester
acknowledges completion.

The transition is deliberately conservative. Running tasks are never moved;
dead tasks are transferred to the BSP for reclamation; pinned or otherwise
non-migratable work rejects the request. Cancellation and rejection states are
explicit and bounded, and there is no attempt to restart an offline AP.

Validation command:

```powershell
python scripts/test-stage6-hotplug.py
python scripts/test-stage6-hotplug.py --cpus 4
```

Result: PASS on the development QEMU profiles (`-smp 2` and `-smp 4`, exit
status 33). The serial evidence contains the corresponding online-prefix
markers:

- `[SMP] online=2 expected=2`
- `[SMP] topology/NUMA affinity: PASSED`
- `[S6.HOTPLUG] offline AP: PASSED online=1 mask=0x1`

The durable logs are retained under
`audit-artifacts/stage6-hotplug-2cpu-1789575540254189600/` and
`audit-artifacts/stage6-hotplug-4cpu-1789575572844417400/`.

Warning-denying freestanding Clippy also passed with the hotplug feature:

```powershell
cargo clippy -p wovenhat-kernel --target x86_64-unknown-none --features stage6-hotplug-test -- -D warnings
```

This closes the software gap for bounded AP offline control. AP re-online/AP
restart, hotplug across non-contiguous topology, APIC-ID-above-255 hardware,
and multi-node NUMA qualification remain separate production requirements.

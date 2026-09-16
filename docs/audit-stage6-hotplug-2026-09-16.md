# Stage 6 bounded CPU-offline audit — 2026-09-16

The Stage 6 SMP control plane now supports a bounded offline/re-online
transition for the highest-numbered online AP. The request is accepted only
for a contiguous online prefix and only after the scheduler evacuates every
non-running task that has a legal online destination. The AP then reaches its
CPU-owned idle checkpoint, masks its local timer, parks with interrupts
disabled, and publishes the reduced online mask and count before the requester
acknowledges completion.

The transition is deliberately conservative. Running tasks are never moved;
dead tasks are transferred to the BSP for reclamation; pinned or otherwise
non-migratable work rejects the request. Cancellation and rejection states are
explicit and bounded. Re-online reuses the parked AP bootstrap context and
reclaims the prior offline-idle slot before restoring the local timer. The
focused gate repeats this lifecycle twice to prove bounded slot reuse.

Validation command:

```powershell
python scripts/test-stage6-hotplug.py
python scripts/test-stage6-hotplug.py --cpus 4
```

Result: PASS on the development QEMU profiles (`-smp 2` and `-smp 4`, exit
status 33). Each run proves offline and re-online with the corresponding
online-prefix markers:

- `[SMP] online=2 expected=2`
- `[SMP] topology/NUMA affinity: PASSED`
- `[S6.HOTPLUG] offline AP: PASSED online=1 mask=0x1`
- `[S6.HOTPLUG] lifecycle: PASSED cycles=2 online=2 mask=0x3` (2 CPU run)
- `[S6.HOTPLUG] lifecycle: PASSED cycles=2 online=4 mask=0xf` (4 CPU run)

The durable logs are retained under
`audit-artifacts/stage6-hotplug-2cpu-1789580521035747800/` and
`audit-artifacts/stage6-hotplug-4cpu-1789580543179207500/`.

Warning-denying freestanding Clippy also passed with the hotplug feature:

```powershell
cargo clippy -p wovenhat-kernel --target x86_64-unknown-none --features stage6-hotplug-test -- -D warnings
```

This closes the software gap for the bounded contiguous-prefix AP
offline/re-online lifecycle. Non-contiguous hotplug topology, APIC-ID-above-255
hardware, and multi-node NUMA qualification remain separate production
requirements.

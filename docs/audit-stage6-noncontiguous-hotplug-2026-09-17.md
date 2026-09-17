# Stage 6 non-contiguous CPU lifecycle audit — 2026-09-17

The earlier hotplug path accepted only the highest online AP and treated
`online_count()` as the last usable CPU index. Removing a middle CPU would
have omitted higher online CPUs from scheduler placement and TLB shootdowns,
and would have waited for an offline CPU's shootdown acknowledgement.

The online population and mask are now separate concepts. A parked AP keeps
its logical slot, APIC identity, and bootstrap stack. The control plane
serializes requests, accepts any online non-BSP AP for removal, decrements
the population on publication, and later permits that same slot to rejoin.
The AP publishes mask changes under the TLB-shootdown serialization lock;
shootdowns snapshot the mask and wait only for CPUs in that snapshot.
Scheduler load placement, explicit migration, kernel spawning, and affinity
selection iterate actual online slots rather than the population prefix.
A draining AP, and an AP between online publication and idle-task rebuild,
remain eligible for interrupt routing but are excluded from new scheduler
placement.

Offline preparation now validates every task before committing any owner or
affinity change. A pinned task or task without an allowed destination rejects
the request without partially moving earlier tasks. The CPU-owned idle
checkpoint repeats this validation before parking. The feature gate checks
the rejection invariant with a movable task followed by a pinned one.

The 4-CPU QEMU gate repeats CPU 1 offline/online twice. While CPU 1 is parked,
it checks `online=3` and mask `0xd`, rejects scheduling to CPU 1, accepts
rescheduling CPU 3, runs a new CPU-3 worker, and completes an acknowledged
TLB shootdown before restoring mask `0xf`. The existing 2/4-CPU tail-AP
lifecycle remains part of the same gate. Dedicated test logs are retained in
dated `audit-artifacts/stage6-hotplug-{2,4}cpu-*` directories.
The final source passed warning-denying freestanding Clippy with the hotplug
feature, `python scripts/test-release.py` (host tests, 1/2/4-CPU debug
memory/storage/network, legacy PIC, 4-CPU release gates, and shell smoke),
and the dedicated twice-cycled 2/4-CPU hotplug gates. The release report is
`target/release-validation/results.json`; the final dedicated logs are under
`audit-artifacts/stage6-hotplug-2cpu-1789634636887171200/` and
`audit-artifacts/stage6-hotplug-4cpu-1789634661181304400/`.

This is software qualification in the QEMU topology. Physical CPU removal
and return with a non-contiguous topology, APIC IDs above 255, and multi-node
NUMA placement are still hardware-only gates. Concurrent hotplug requests
are rejected while one request is active; broad stress of request
cancellation, concurrent process creation, and device I/O during a real
hardware transition remains open.

## Request cancellation and retry follow-up

An AP now claims an offline request with `1 -> 2` and an online request with
`6 -> 8`. A requester may cancel only the unclaimed state, atomically restoring
the prior stable online state or offline state `5`. If the AP has claimed the
operation, the requester waits for success or a terminal rejection rather
than reporting a cancelled request while the AP can still change topology.
The AP publishes rejection only after restoring its interrupt state. The
requester then rearms the stable state, so both operations can be retried.
If a claimed transition never reaches a terminal state within the second
deadline, the hotplug control plane stays locked against new requests; this
contains the uncertainty but requires operator diagnosis of the failed AP.

The QEMU gate uses feature-only hooks to force an AP rejection of each
operation and to hold each request unclaimed through its timeout. It checks
the AP actually visited both rejection hooks and both hold hooks, verifies
the online count/mask stayed stable, and successfully retries offline and
online operations afterward. These probes run on both 2- and 4-CPU profiles
before the original tail and middle-CPU lifecycle cycles. The focused logs
include `[S6.HOTPLUG] rejection recovery: PASSED`, the state-1 and state-6
timeout records, and `[S6.HOTPLUG] timeout recovery: PASSED`.
The follow-up source passed all 33 checks in
`target/release-validation/results.json`, including warning-denying Clippy,
host tests, 1/2/4-CPU debug QEMU gates, release QEMU gates, and shell smoke.
The focused follow-up evidence is under
`audit-artifacts/stage6-hotplug-2cpu-1789635217388184700/` and
`audit-artifacts/stage6-hotplug-4cpu-1789635182453579800/`.

This covers deterministic cancellation and retry under QEMU. Concurrent
process creation, device I/O, and physical hardware faults during a claimed
transition still require stress and hardware qualification.

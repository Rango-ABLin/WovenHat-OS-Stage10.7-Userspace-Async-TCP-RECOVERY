# Stage 6 frame reclamation audit — 2026-09-17

The physical allocator previously kept only 4,096 returned frames in a static
table. A valid `deallocate_frame` call after that table filled returned false,
so runtime address-space teardown could permanently lose reusable RAM. The
table was adequate for a single 8 MiB heap-map rollback, but it was not a
general reclamation policy.

Each usable physical range now reserves its own page-aligned allocation bitmap
from the start of that same range. The bitmap pages never enter the frame
pool, and allocator statistics count only the remaining allocatable frames.
Every successful single-frame or contiguous allocation marks its bits. A
return checks range ownership, that the frame was handed out below the
allocation cursor, and that its bit is still set. This rejects a double free
even when the returned frame no longer fits the hot cache.

The existing 4,096-entry table remains a fast reuse cache. When it fills,
further returns clear their bitmap bits and increment an overflow count;
allocation scans the used portion of the affected ranges after draining the
cache. Each range retains the lowest overflow address as a scan hint, avoiding
a repeated search from the start. Domain preference is preserved for both
cached and overflow frames.
The allocator no longer fails a valid return solely because the cache filled.

The QEMU boot probe allocates and returns 4,352 frames, verifies that the
overflow path was reached, reacquires the same frame set, and rejects a
duplicate return. The memory gate requires its serial marker. This proves
the overflow path on the 256 MiB QEMU profiles; high-RAM scan latency,
low-memory behavior, and physical NUMA placement still need qualification.

The final source passed all 33 checks in
`target/release-validation/results.json`: warning-denying kernel and host
Clippy, host tests, 1/2/4-CPU QEMU debug boot/storage/network suites, legacy
PIC, four-CPU release suites, release build, and shell smoke. Dedicated
twice-cycled hotplug gates also passed, with evidence under
`audit-artifacts/stage6-hotplug-2cpu-1789636337456573500/` and
`audit-artifacts/stage6-hotplug-4cpu-1789636368058079500/`.

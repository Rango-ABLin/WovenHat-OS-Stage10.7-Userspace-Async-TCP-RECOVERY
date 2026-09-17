# Stage 6 runtime heap growth audit — 2026-09-17

The heap previously mapped at most 8 MiB during boot and could not expand.
The global allocator can run while paging, frame, process, or device guards
are held, so calling the rank-10 pager while the rank-50 heap guard is held
would invert the lock order and can deadlock under SMP.

Boot mapping remains RAM-scaled and capped at 8 MiB. Runtime growth now maps
256 KiB chunks, with one growth owner at a time. The owner reads the current
boundary under the heap guard, releases it, maps the next chunk through the
pager, and publishes the new boundary only after mapping succeeds. The owner
keeps local task preemption deferred while growing, while IRQs stay available
between pager critical sections for remote TLB acknowledgement. A failed
chunk leaves the previously published heap intact; completed earlier chunks
remain usable. Growth stays within a 64 GiB virtual window in the original
heap P4 slot shared by already-created user address spaces.

A failed allocation can grow synchronously and retry when the caller has no
ranked guard and local IRQs are enabled. Allocations under guards still use
only mapped pages. Successful allocations request proactive growth when the
untouched tail or total free space falls below 1 MiB; the BSP idle task maps
ahead in bounded batches. Total free bytes are maintained incrementally, so
this pressure check and heap statistics do not traverse the free list.

The host heap test checks that a failed large allocation leaves the original
arena unchanged, then succeeds after a mapped extension while an older live
payload remains intact. The QEMU memory gate allocates beyond the initial
8 MiB boundary after SMP startup, checks that the mapping grew, validates
the payload and exact free-byte recovery, then exercises reserve maintenance
under pressure. Its serial record includes the final mapped-byte count.
The 256 MiB QEMU profiles grew from the boot mapping of 8,388,608 bytes to
10,223,616 bytes during that probe.

The final source passed all 33 checks in
`target/release-validation/results.json`: warning-denying kernel/host Clippy,
host tests, 1/2/4-CPU debug QEMU memory/storage/network, legacy PIC, 4-CPU
release gates, release build, and shell smoke. Dedicated twice-cycled hotplug
gates passed with evidence under
`audit-artifacts/stage6-hotplug-2cpu-1789638980824115500/` and
`audit-artifacts/stage6-hotplug-4cpu-1789639011999404000/`.

The synchronous path requires a safe paging context. A sudden allocation
under a higher-ranked guard can still fail if its demand exceeds mapped
headroom before idle maintenance runs. Low-memory rollback, broad concurrent
allocator throughput, and physical NUMA placement remain open qualification
work; runtime capacity also depends on available physical frames.

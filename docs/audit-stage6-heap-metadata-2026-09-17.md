# Stage 6 heap metadata follow-up — 2026-09-17

The kernel heap previously stored live allocations in 2,048 static slots and
free intervals in a paired 2,049-slot table. A small allocation could fail
when those slots filled even with mapped heap bytes still available. The
number of allocatable objects was therefore an independent capacity limit.

The heap now places an allocation header immediately before each aligned
payload. The header records the complete reserved span, including alignment
padding and any short remainder absorbed from a free interval. Freed spans
carry their next pointer and size in their own mapped bytes. The free list is
address ordered, coalesces neighboring spans, and rewinds the bump cursor
when the highest span becomes free. Neither live-object nor free-interval
tracking consumes a fixed external slot table. The rank-50 IRQ-safe heap
guard remains the sole metadata writer and still never calls the rank-10
pager while held.

The host harness now backs the production heap state machine with a real
page-aligned arena. It verifies more than 2,048 simultaneous live objects,
alignment padding recovery, 4,096-way fragmentation/coalescing, and a
deterministic mixed-size/alignment workload that checks every live payload
before release. The QEMU memory gate requires a boot probe with 4,096
simultaneous `Box<u64>` objects and exact allocation/free-byte recovery.

The final source passed all 33 release checks in
`target/release-validation/results.json`, including warning-denying Clippy,
host tests, 1/2/4-CPU debug QEMU memory/storage/network gates, legacy PIC,
four-CPU release gates, and shell smoke. Dedicated twice-cycled hotplug gates
passed with evidence in
`audit-artifacts/stage6-hotplug-2cpu-1789637845298615200/` and
`audit-artifacts/stage6-hotplug-4cpu-1789637873756294400/`.

The mapped heap remains capped at its boot-time 8 MiB maximum, and free-list
search remains linear in the number of holes. Runtime mapping growth, low-RAM
behavior, and broader multicore allocator throughput remain open.

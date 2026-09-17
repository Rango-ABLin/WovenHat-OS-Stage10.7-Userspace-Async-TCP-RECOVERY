# Stage 6 interrupt-safe lock-order audit — 2026-09-16

`irq_lock::IrqMutex` now records a bounded held-lock stack for each logical
CPU. A lock may declare a monotonic rank; lower-ranked nesting is rejected
before spinning. Recursive acquisition, more than eight nested locks, and
non-LIFO guard release are deterministic failures. Rank zero remains an
explicit unranked compatibility mode for independent single-lock domains.

The audited asynchronous operation path declares the production order:

```text
async operation table (10) -> completion-port allocation (10) -> port slot (20)
```

The scheduler and memory-management domains now declare the complementary
order:

```text
scheduler (10) -> process table (20)
paging (10) -> COW table (30) -> physical-frame allocator (40)
pager queue/state (10) -> scheduler (10)
teardown registries (notifications, threads, async events) (10)
pipe table (10) -> scheduler wake/block handoff
IPC namespace (10) -> WovenGuard lineage (10) -> paging/frame release (10/40)
diagnostic/audit ledger (40)
ATA device and isolated catalog registries (10)
```

The tracker is active in the freestanding kernel. Host test doubles implement
the same `with_rank` constructor so host behavior cannot silently diverge from
the production API.

Validation completed:

```powershell
cargo test --test irq_lock
cargo clippy -p wovenhat-kernel --target x86_64-unknown-none -- -D warnings
python scripts/test-release.py
```

The interrupt-lock tests passed (2/2), warning-denying freestanding Clippy
passed, and the complete release matrix passed, including async runtime,
memory/storage/network 1/2/4-CPU gates, legacy PIC, release gates, and shell/
SMP smoke. The Stage 6 hotplug lifecycle also passed twice on both 2- and
4-CPU QEMU profiles after the tracker was enabled.

During release qualification the tracker caught a termination-path inversion
where completion-port ownership was released while `PROCESS_TABLE` was held.
The termination path now marks the process and takes its file table under the
rank-20 guard, then performs completion, event, notification, VFS, and pipe
teardown after the guard is dropped. The complete release matrix was rerun
after this fix and passed. The same ownership-transfer rule now covers the
self-exit and wait/reap paths: VFS references and IPC endpoint unregistering
occur only after the process-table guard has been released.

The 2026-09-17 follow-up converted the IPC namespace and WovenGuard lineage
registries to the same ranked IRQ-safe mutex. Qualification then caught and
fixed IPC registration under `PROCESS_TABLE` during userspace spawn and fork;
all registration rollback paths now release the process-table guard before
touching IPC. The complete release matrix and both 2/4-CPU hotplug cycles were
rerun after that correction and passed.

After the bounded audit/ATA/snapshot/WovenFS/driver/PCI registry conversion,
the complete release matrix was rerun on 2026-09-17. All host tests, 1/2/4-CPU
debug suites, legacy PIC, 4-CPU release suites, release build, and shell smoke
passed; `target/release-validation/results.json` retains the result.

The next follow-up placed the file-frame cache under a rank-10 IRQ mutex and
added call-site lines to non-LIFO lock diagnostics. Fork and descriptor
duplication now retain VFS/pipe references outside `PROCESS_TABLE` and
revalidate the source descriptor before publication. Open-file descriptions
and anonymous pipes now carry non-wrapping slot epochs, so a close/reuse race
cannot turn a stale kernel handle into a reference to a different object.
Pipe creation also reserves both descriptor slots before publishing either
end and releases the new pipe on failure.

The final follow-up passed `python scripts/test-release.py`: warning-denying
kernel/host Clippy, host tests, memory/storage/network QEMU suites on 1/2/4
CPUs, legacy PIC, 4-CPU release suites, release build, and shell/SMP smoke.
`target/release-validation/results.json` records the gate results. Dedicated
hotplug cycles passed twice on both 2 and 4 CPUs; their serial evidence is in
`audit-artifacts/stage6-hotplug-*`.

A trial VFS IRQ-lock conversion exposed an existing slow-I/O boundary:
disk-backed node materialization and descriptor reads ran while VFS
registry/open-description guards were held. The conversion was withdrawn
until the I/O was split into an unlocked transaction. Disk reads now snapshot
the node, backing path, version, and shared offset, release both guards for
I/O, then revalidate before returning bytes or advancing the offset. Lazy
materialization does the same for unlink, shared mappings, and writes. Bounded
retries reject conflicting changes and generation checks reject slot reuse.
Renames advance node versions, and prefix callbacks run after a bounded path
snapshot is taken. The VFS registry and open-description table now use rank-10
IRQ mutexes. The file-size path explicitly drops its nested registry guard
before the open-description guard.

This closes the bounded lock-order coverage gap for the audited scheduler,
process, paging, COW, frame-allocation, pager, async-operation, completion-port,
file/block/network worker, pipe, IPC namespace, WovenGuard lineage, audit, ATA,
file-frame cache, VFS, and isolated catalog domains. Other subsystem locks
still use compatibility mutexes; broader filesystem mutation transactions and
priority inheritance remain separate Stage 6 work.

The subsequent pipe wait audit found a wake/block race in `wake_task` plus
`block_current`: a peer could wake a still-running task immediately before it
blocked, losing the only wakeup. Pipe readers and writers now use the
scheduler's latched `signal_event`/`wait_for_event` handshake. A full waiter
table returns `Full` instead of silently dropping a waiter that would then
sleep forever. The pipe boot self-test covers duplicate registration and the
bounded waiter capacity.
The full release matrix passed after this change, including warning-denying
lint, host tests, 1/2/4-CPU memory/storage/network gates, legacy PIC, 4-CPU
release gates, and shell smoke. Dedicated 2/4-CPU hotplug passed twice-cycled
offline/re-online tests again.

After the VFS I/O split and rank-10 conversion, `python scripts/test-release.py`
passed warning-denying lint, host tests, 1/2/4-CPU memory/storage/network
gates, legacy PIC, 4-CPU release gates, release build, and shell smoke.
Dedicated 2/4-CPU twice-cycled hotplug also passed. One earlier focused
1-CPU storage boot stopped at the FAT32 mkdir mutation check while the VFS
refactor was in progress; a subsequent run, the full release matrix, and two
additional focused runs passed. The storage self-test now logs the underlying
persist error if that intermittent failure recurs. Five more sequential
1-CPU FAT32 storage boots passed with automatic failure-log preservation.
The original failed serial log was overwritten before it could be copied, so
the cause of that one transient failure remains unproven; it is not treated
as evidence that unrestricted concurrent filesystem mutation is complete.

The heap metadata lock is now rank-50 IRQ-safe. Boot-time heap page mapping
occurs before that guard is taken, so runtime allocation never nests a
lower-ranked paging lock beneath the heap. The full release matrix passed
after this change, as did twice-cycled 2/4-CPU hotplug. The 256 KiB heap
capacity and fixed metadata tables remain separate scalability limits.

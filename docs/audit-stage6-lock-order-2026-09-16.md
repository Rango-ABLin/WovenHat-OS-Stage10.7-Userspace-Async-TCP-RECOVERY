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

This closes the bounded lock-order coverage gap for the audited scheduler,
process, paging, COW, frame-allocation, pager, async-operation, completion-port,
and file/block/network worker, and pipe domains. Locks in other subsystems still use their
existing compatibility mutexes, and priority inheritance remains separate
Stage 6 work.

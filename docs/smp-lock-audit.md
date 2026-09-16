# SMP ownership and lock audit - 0.8.0

## Scheduling invariant

Every task has an immutable CPU assignment for its lifetime. `current_slot` and
round-robin cursors are per-CPU. The scheduler mutex protects task metadata.
`prepare_switch` only selects tasks assigned to the caller; `reap_dead` only
reclaims the caller's non-current dead tasks. Publishing a Ready state before
saving RSP is safe only because another CPU cannot select that task. Migration
must not be added without a context-save handoff protocol.

AP idle contexts keep their original bootstrap stacks alive. GDT/TSS/IST state
is private to each CPU. Timer handlers use `try_lock`, acknowledge the LAPIC
before switching, and cannot schedule while their CPU holds the scheduler lock.

## Shared domains

| Domain | Ownership / synchronization | Multicore boundary |
| --- | --- | --- |
| Scheduler/task stacks | Global metadata mutex; CPU-owned contexts and reap | Parallel kernel jobs supported; no migration |
| GDT/TSS/IST | One static slot per CPU | Local interrupt-masked TSS updates |
| Page tables | PAGING mutex; page-frame and COW locks below it | All invalidations acknowledged before frame reuse |
| Frame allocator | ALLOCATOR mutex | Low MiB excluded; bootstrap pages retained |
| Shootdown | Atomic serialization; per-CPU atomic ACK | NMI handler takes no locks and does not schedule |
| Process table/userspace | Existing process lock plus BSP ownership | Not exposed to AP jobs |
| VFS/storage/cache/ATA | Existing locks plus BSP execution | Driver/pager workers remain CPU 0 |
| File-fault I/O depth | BSP-only preemption guard | Does not suppress AP timer preemption |
| Network/virtio | BSP polling and existing locks | Not exposed to AP jobs |
| Terminal/GUI/keyboard decoder | BSP ownership | IOAPIC routes keyboard to BSP |
| Serial output | Port I/O, no whole-message lock | AP failures may interleave diagnostic output |

## Lock ordering and TLB progress

Paging paths take PAGING before the frame allocator/COW locks. No shootdown
handler acquires these locks or the scheduler mutex. This is essential: a CPU
may be waiting for PAGING with ordinary interrupts disabled when another CPU
requests invalidation. NMI delivery allows it to acknowledge without releasing
or re-entering any lock.

The shootdown initiator masks local interrupts before taking its atomic lock,
publishes a generation after PTE stores, invalidates locally, sends remote NMIs,
and waits with acquire loads for matching acknowledgements. A timeout panics;
it never silently frees a possibly cached frame. Local interrupt state is
restored after releasing the serialization lock.

`spawn_on` and `spawn_parallel` are unsafe: AP entries are restricted to atomic
computation and task yield/sleep/exit primitives. They must not call allocator,
paging, file, userspace, network or GUI services; a spin mutex alone does not
make a service safe against preemption on the same CPU. The scheduler uses least-loaded placement without migration.
Existing `spawn` and userspace spawn retain CPU 0 affinity. The release does not
claim that every legacy kernel service is safe to call concurrently from APs.

## Regression evidence

- A job runs on every online CPU and enters an atomic cross-CPU barrier.
- Non-yielding jobs require their CPU's timer to schedule a peer.
- Remote CPUs cache a kernel virtual address; its physical frame is replaced
  eight times, invalidated, and retired; each reader must see the new value.
- The existing userspace/fork/mmap/pager and ATA/FAT32 suite runs while APs and
  their timer handlers remain online.

## Stage 7 stabilization addendum - 2026-09-10

The original text above predates ready-task migration and therefore describes task CPU ownership as
immutable. The current scheduler now supports restricted migratable kernel jobs. A running task is
never directly exposed as `Ready` during a stack hand-off: it passes through `TaskState::Switching`,
which is neither schedulable nor migratable. The incoming context finalizes the outgoing task back to
`Ready` after the physical stack switch. Runnable-load accounting counts `Switching` as running.
Userspace and legacy I/O/service work remain pinned to CPU0.

Process-table access is locally IRQ-safe through `process_table_lock()`. The intended nested order is
`SCHEDULER -> PROCESS_TABLE`; code that begins from process metadata must release that guard before
acquiring the scheduler. Pager request publication uses one scheduler critical section to mark the
faulting requester Blocked and make the pager Ready, preventing a lost wakeup.

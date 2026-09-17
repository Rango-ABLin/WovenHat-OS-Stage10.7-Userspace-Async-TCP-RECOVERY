# WovenHat OS Architecture & Codebase Guide

## Scope and source of truth

This guide describes the Stage 10.7 source, not the proposed 1.0 system. WovenHat
currently has a monolithic Rust kernel, a UEFI boot image builder, embedded Ring-3
programs, and host/QEMU acceptance harnesses. The supplied
[master development roadmap](master-development-roadmap.md) defines future stages.
[Stage status](stage-status.md) records which gates have actually passed.

## Repository map

| Files | Responsibility |
| --- | --- |
| `Cargo.toml`, `kernel/Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `.cargo/config.toml` | Workspace, pinned toolchain, freestanding kernel artifact and feature selection |
| `build.rs`, `src/main.rs` | Package the kernel with the UEFI bootloader; expose the generated image path to harnesses |
| `kernel/src/main.rs`, `config.rs` | Initialization order, subsystem wiring, configuration bounds and boot acceptance orchestration |
| `hal/`, `ap_start.S`, `smp.rs`, `gdt.rs`, `interrupts.rs`, `pic.rs`, `timer.rs` | CPU/platform discovery, AP startup, descriptor tables, interrupts and timekeeping |
| `task.rs`, `userspace.rs`, `elf.rs`, `syscall.rs` | Scheduling, process lifecycle, Ring-3 images, ELF validation and syscall dispatch |
| `memory.rs`, `paging.rs`, `heap.rs`, `swap.rs`, `file_frames.rs`, `file_mapping.rs`, `page_cache.rs` | Physical/virtual memory, allocation, backing storage and file mappings |
| `block.rs`, `block_cache.rs`, `block_io.rs`, `ata.rs`, `partition.rs`, `gpt.rs`, `storage.rs` | Block devices, request execution, caching, partitions and storage initialization |
| `vfs.rs`, `fat32.rs`, `async_file.rs` | Filesystem operations and positional asynchronous file requests |
| `async_op.rs`, `async_network.rs`, `irq_lock.rs` | Completion ownership, asynchronous sockets and interrupt-safe network/completion locks |
| `network.rs`, `virtio_net.rs` | smoltcp IPv4 sockets and the transitional VirtIO-net transport |
| `capability.rs`, `wovenguard.rs`, `audit.rs`, `entropy.rs` | Authority, domains, resource policy, security ledger and entropy support |
| `ipc.rs`, `pipe.rs` | IPC objects, endpoint/message operations and byte pipes |
| `device.rs`, `keyboard.rs`, `serial.rs`, `console.rs`, `terminal.rs`, `shell.rs` | Device registry, input, diagnostics and command interfaces |
| `graphics.rs`, `gui.rs` | Existing framebuffer/GUI support; not the future compositor |
| `panic.rs`, `benchmark.rs` | Failure reporting and existing benchmark support |
| `tests/`, `scripts/`, `run-stage*-acceptance.ps1` | Host regressions, QEMU orchestration and stage preservation gates |

## Stage 6 filesystem concurrency boundary

`vfs.rs` keeps node metadata and shared open descriptions in separate rank-10
IRQ mutexes; when both are needed, open descriptions precede nodes. Handles
carry non-wrapping slot epochs, and node references carry generations. Disk-
backed reads and lazy materialization snapshot identity, backing path, version,
and (for shared reads) offset, drop both VFS guards for ATA I/O, then revalidate
before publishing data or changing the seek position. A conflict retries a
bounded number of times. Rename increments node versions. Prefix iteration
copies bounded path names before calling external code. File-frame cache
updates may nest under the VFS guards at rank 10.

The global heap's metadata guard is rank 50 and disables local interrupts
during allocation and deallocation. Boot-time heap page mapping occurs before
acquiring that guard, preserving the paging (10) to heap (50) order. Its eager
mapped size is one sixteenth of available physical memory, clamped to 256 KiB
through 8 MiB. The allocator tracks 2,048 live objects, retains alignment
padding with its allocation, coalesces freed intervals, and reserves enough
free-list slots for every possible hole at that live-object bound. Kernel
range mapping rolls back newly mapped pages on a partial failure. Later heap
growth still requires a design that can honor lock order when allocation
occurs under paging, frame, process, or audit guards.
The swap-state guard is rank 40 and releases before ATA transfers. Device,
keyboard decoder, journal-intent, mount-record, and key-vault metadata guards
are rank 10; none performs a blocking transfer while held. Keyboard captures
the caller's interrupt state before taking its guard to distinguish normal
IRQ-driven input from early-boot legacy polling.

The clean FAT32 page cache uses a rank-20 IRQ mutex for resident-page lookup,
copying, publication, and invalidation. A miss loads the 4 KiB page after
dropping that guard. Invalidation advances an epoch; a load that spans an
invalidation returns an I/O error instead of republishing stale cache data.

The shell's current-directory state is a short rank-10 IRQ-mutex section. It
copies the path into a fixed buffer before allocation or console output. The
global framebuffer terminal instead uses a rank-10 preemption mutex: it
prevents a local task switch while held but leaves device interrupts live
during long scroll and clear operations. Its rank-tracker updates briefly
mask local interrupts so an IRQ cannot observe a partial held-lock stack.
The terminal is never called from an IRQ handler and must not block or yield
while its rendering guard is held. Syscall writes use `file_fault_io` to
restore live interrupts around rendering even when syscall entry had IF=0.

This lock split covers the VFS/ATA slow-I/O boundary. FAT32 mutation still
crosses the storage and VFS layers without one transaction, so unrestricted
concurrent filesystem operations remain a separate production requirement.

## Stage 6 CPU lifecycle

Logical CPU slots and APIC identities remain stable while the online mask may
have holes. `online_count` is a population count, not an upper bound on valid
CPU indices. Scheduler placement uses a separate schedulable mask that excludes
APs draining or rebuilding their idle task; TLB shootdown snapshots the
physical online mask under the same transition lock that publishes an AP's
offline/online state. An offline request first plans all Ready-task moves under
the scheduler lock and commits them only if every task has a legal destination.
The CPU-owned idle checkpoint revalidates that plan before parking. A parked
AP retains its bootstrap stack and rejoins its original logical slot.
The requester cancels only a request the AP has not claimed; claimed offline
and online transitions use separate states and must reach a terminal result.
A rejected AP transition is rearmed for retry after AP cleanup. If a claimed
transition never completes, further hotplug requests remain disabled rather
than proceeding with uncertain CPU ownership.
The 4-CPU QEMU gate offlines CPU 1 while CPU 3 remains active, runs a worker on
CPU 3, and performs an acknowledged shootdown across the resulting hole.

## Stage 10.7 objects and data flow

`async_op::Handle` identifies a slot and a 32-bit generation. Its raw ABI uses
bits 0..15 for the slot, bits 32..63 for generation, and requires reserved bits
16..31 to be zero. A `Completion` is 16 bytes: signed status, reserved word and
64-bit value. The operation table records `TaskId` ownership, class, state and a
wait registration. Today ownership is task-bound; a shared per-process completion
port is Stage 10.8 work, not an existing property.

`network::UserSocket` associates a process owner and descriptor slot with a
smoltcp socket, peer, generation, async reference count and closing flag.
`SocketToken` carries slot, generation and owner for in-flight kernel operations.
Closing a pinned descriptor hides it from further descriptor operations; the
socket remains available to its existing tokens until the last pin is released.

`async_network::Request` stores owner, request identity, socket token, operation,
endpoint, length, kernel-owned bytes, result and generic completion handle.
The bounded queue holds pending, in-progress and completed requests. The worker
copies one request under the queue lock and releases that lock before entering
the socket runtime. Cancellation/teardown detach an in-progress request rather
than freeing its socket under the executing worker. The worker subsequently
releases that pin on completion or retry.

The submission path is:

1. `syscall.rs` checks WovenGuard network-device authority and validates arguments.
2. Send copies bytes from userspace immediately; receive retains only capacity.
3. `async_network.rs` pins the socket and allocates an owner-bound Network handle.
4. Queue publication precedes signaling the worker's latched scheduler event.
5. The worker attempts each queue slot at most once per pass. `WouldBlock` leaves
   that request pending and permits later requests to run. After the bounded
   pass, the worker waits for an event.
6. `network::poll()` drives smoltcp, releases its runtime lock, then notifies the
   worker if one locked queue snapshot sees pending or in-progress work.
7. Completion publication wakes a registered owner. Poll/wait copy completion
   and receive bytes to validated userspace destinations before consuming the
   request, releasing its socket pin and releasing the generic handle.

Failed copyout preserves the kernel result for retry. No asynchronous request
retains a userspace pointer. Cancellation does not roll back bytes already
accepted by the socket transport.

## Existing syscall boundary

| Number | API | Behavior |
| --- | --- | --- |
| 67 | Async cancellation | Owner-authorized cancellation; class-specific request cleanup |
| 77 | `AsyncNetSend` | Socket descriptor, input pointer and length; returns a completion handle |
| 78 | `AsyncNetRecv` | Socket descriptor and capacity; returns a completion handle |
| 79 | `AsyncNetPoll` | Nonblocking completion/data collection |
| 80 | `AsyncNetWait` | Event-driven wait followed by retry-safe collection |
| 81 | `AsyncTcpConnect` | Socket descriptor and packed endpoint; completes on connection readiness |

The audit corrections add no syscall numbers or capabilities. Existing network
policy gates remain at submission and collection; ownership checks remain on
handles. Teardown does not depend on retaining network permission.

## Concurrency and lock order

`irq_lock::IrqMutex` disables local maskable interrupts before taking its spin
mutex. Its guard releases the mutex before restoring the original interrupt
state and cannot be transferred to another thread. Nested guards preserve IF=0.
This addresses same-CPU preemption deadlock, which a plain spin mutex cannot
prevent even when the workload runs on only one CPU.

`irq_lock::PreemptMutex` is reserved for CPU-only sections that need live
interrupts. It increments a per-CPU no-preempt depth before acquiring the
spin mutex, and its guard releases the mutex and rank token before decrementing
that depth. The timer preemption path checks both this depth and the separate
I/O depth. It cannot be used in an IRQ handler or around a voluntary switch.

The network request queue, worker identity, generic completion table, socket
runtime and VirtIO-net transport use this guard. The runtime is rank 20 and
the transport is rank 30, matching runtime-to-transport packet flow. No task
may sleep or switch while holding it. The worker drops queue/runtime guards
before generic completion
publication or scheduler event waiting. Teardown can follow scheduler -> request
queue -> socket runtime. Network polling drops runtime before querying the queue;
transport polling releases its lock before socket-runtime acquisition. Runtime
packet handling can take runtime -> transport.

The socket worker remains CPU0-pinned. These changes do not introduce parallel
smoltcp workers, per-CPU network queues or unrestricted multicore I/O services.
The existing network runtime remains globally serialized. The guard does not
prove every cross-subsystem lock order; broader path audits remain necessary.

## Tests and evidence

`tests/async_network.rs` includes the production worker with deterministic host
socket/scheduler doubles. It covers blocked-before-ready requests, bounded passes,
parking, in-progress readiness notification, cancellation and racing owner cleanup.
`tests/irq_lock.rs` includes the production guard with host interrupt/mutex doubles.
`tests/test_tcp_harness.py` verifies host-server success and failure reporting.

The Ring-3 TCP probe queues receive before send on its second connection, forcing
the worker to handle a blocked operation without starving the send that enables
the host reply. It also checks invalid completion/data destinations and retries,
close while pinned, EOF, cancellation and owner teardown. The host requires both
TCP exchanges, required serial markers and QEMU debug-exit status 33.

`RUN-STAGE10.7.ps1` captures the chained acceptance transcript and snapshots QEMU
logs under `audit-artifacts/`. Stage 7.6 stress logs are retained per invocation.
`build.rs` tracks the actual kernel artifact so writing a log does not itself
trigger image reconstruction. Kernel changes still invalidate the image.

## Boundaries before later stages

Network progress still depends on the existing polling driver, not a new hardware
interrupt-driven NIC architecture. Send completion means local socket acceptance,
not remote acknowledgement; closing does not promise graceful draining. Public
socket descriptors remain process-local integer slots, while in-flight tokens
carry generations. Generation counters remain finite. Generic completions have
single-task ownership. These are
explicit architectural boundaries, not claims of a finished production network stack.

## Stage 10.8 completion ports

`completion_queue.rs` is the allocation-free reservation/FIFO core shared with
host tests. `completion_port.rs` adds process ownership, quotas, generation-tagged
handles, waiter registration and retry-safe batch claims. `async_op.rs` publishes
completion or cancellation into an associated port. Association and completion
serialize on the operation table before locking a port; scheduler notification
occurs after both are unlocked.

Syscalls 82 through 86 create, close, associate, poll and wait. See the
[ABI contract](completion-port-abi.md) for record layout, timeout and capacity
semantics. Port dequeue does not consume the original operation's data result.
`task::wait_for_event_until` extends the existing scheduler event latch with an
absolute deadline and timer-driven wakeup. Normal and forced process exit reclaim
ports after detaching owned operations.

`tests/async_runtime.rs` exercises production operation and port state with host
scheduler/lock doubles. `stage10_8.S` tests the real Ring-3 ABI;
`async_acceptance.rs` exercises multicore producers and a waiting consumer.
`RUN-STAGE10.8.ps1` preserves Stage 10.7 and adds 1/2/4-CPU completion-port boots.
Timers, event objects, IPC completion integration and per-thread cleanup remain
later roadmap work.

## Stage 10.9 timers and events

`async_events.rs` owns bounded generation-tagged timer and manual-reset event
slots. Timer deadlines use the monotonic tick source and are pumped by an
event-driven worker. Syscalls 87–94 expose create, wait, set, close and
sleep-until operations; timer and event waits produce the same generic async
completion records as other operations. Owner teardown cancels outstanding
waiters and releases every slot.

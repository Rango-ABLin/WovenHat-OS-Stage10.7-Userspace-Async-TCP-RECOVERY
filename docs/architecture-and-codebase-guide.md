# WovenHat OS Architecture & Codebase Guide

<<<<<<< HEAD
## TCP close/drain repair (2026-09-24)

A closed TCP descriptor retains its transport after the final async reference
until graceful close completes. `network::poll` reaps those existing bounded
slots; a 30-second grace expires on a subsequent poll for unresponsive peers.
Slots remain counted and unavailable for reuse until retirement. This fixes
queued-data loss when a send completion was consumed before transmission.
See [the TCP repair audit](audit-tcp-close-drain-2026-09-24.md) for failure
evidence, locking/ownership review and validation limits.

## Selected physical machine and inventory boot (2026-09-24)

The latest user instruction selects this HP Pavilion x360 with AX201
`8086:A0F0`, superseding the earlier AX200 target preference. Native AX201
transport is not implemented. The existing AX200 activation boundary remains
unchanged until the appropriate device-specific implementation is reviewed.

`physical-probe` is an independent root/kernel Cargo feature. After early
RAM and paging checks, `kernel_main` enters `physical_probe::run`, enumerates
PCI through the existing HAL, prints network identities and AX201 BAR/capability
details to the framebuffer and serial, and halts. It does not enter driver
activation, storage/network runtime, AP startup or candidate QEMU tests. No
device configuration or MMIO register writes are added by this mode; PCI
configuration reads use the existing ECAM/legacy address-selection access.
It is an inventory aid and never radio acceptance. See the
[AX201 machine audit](audit-ax201-machine-2026-09-24.md).

## Physical Wi-Fi firmware prerequisite (2026-09-24)

The preceding increment targeted AX200; AX201 support is not enabled.
`wifi_ax200_image.rs` preflights runtime LMAC/UMAC/paging region ordering,
payload size and total DMA capacity. `Intel22000DmaContextInfo::from_runtime_firmware`
uses that plan to copy payloads into independently owned DMA buffers. Failure
drops the unpublished partial context; success still requires queue setup and
a reviewed physical publication/lifecycle owner. See the
[runtime-loader audit](audit-ax200-runtime-loader-2026-09-24.md).

`wifi_firmware.rs` delegates Intel container parsing to `wifi_tlv.rs`, an
allocation-free, immutable-borrow parser shared with host tests. It validates
a bounded complete blob, exposes paging-size metadata separately, and
streams runtime/init SEC records including secure variants and explicit
CPU/paging separators. The DMA stager rejects separators before allocation.
This closes real-container parsing failures; it does not implement the
physical transport. The inspected host is AX201, while the candidate match
is AX200. See [the physical preflight audit](audit-physical-wifi-preflight-2026-09-24.md)
for firmware hashes, validation and the remaining lifecycle/transport work.

## Stage 13.10AC integration boundary (2026-09-23)

The sections below describe previously recorded foundations. The current
Wi-Fi candidate is additional, feature-gated source: `main.rs` includes
`wifi_hw` and the other Wi-Fi modules only with `stage13-9-test`. It does not
yet provide the normal boot path with a working physical AX200 driver.
The session and smoltcp adapter now share that boundary. `network.rs` keeps
the candidate transport selector in a feature-gated module, while ordinary
builds use `VirtioSmolDevice` directly. This corrects the previously inconsistent
module graph without changing the VirtIO packet path.

`interrupts.rs` installs the reserved PCI Wi-Fi vector `0xd0`. Its hard IRQ
publishes an atomic work flag and acknowledges the LAPIC. `wifi_hw.rs` exposes
`service_deferred_ax200_interrupt`, which consumes that flag and calls
`IntelRxInterruptController::service` outside the hard IRQ. The controller
masks CSR delivery, reads interrupt status, acknowledges enabled RX causes,
and restores its configured mask; fatal hardware/firmware status returns an
error with delivery masked. The flag coalesces notifications and is not a
counted queue or a scheduler wakeup. The only current deferred-service caller
is the synthetic self-test. A production worker, device lifecycle ownership,
teardown/recovery, and physical interrupt/DMA qualification remain open.

The synthetic CSR test uses an owned DMA page and verifies written values;
ordinary RAM does not emulate write-one-to-clear hardware semantics. The
MSI discovery branch explicitly skips physical programming when AX200 is
absent. Neither synthetic success nor that skip proves physical Wi-Fi works.

`scripts/test-stage10-runtime.py` now pins all 54 Wi-Fi acceptance markers
through 13.10AC in addition to the SMP and async ABI markers and exit code 33.
`tests/test_runtime_harness.py` checks complete evidence, each missing Wi-Fi
marker, the former incomplete marker set, and unsuccessful QEMU exit.
The Stage 13.9 launcher uses the same project-local Cargo directories as
the Stage 10.7 preservation launcher.

## Candidate feature boundaries (2026-09-24)

Ordinary builds retain the VirtIO network transport, PCI discovery and legacy
I/O bus-master path, the reserved Wi-Fi interrupt vector, and PS/2 keyboard
input. Candidate-only APIs are compiled alongside their existing callers:

- The Wi-Fi secure-pool candidate and MSI programming module use
  `stage13-9-test`; the normal best-effort network RNG is unchanged. The
  deterministic test pool is not a production cryptographic entropy source.
- PCI MMIO bus-master enablement follows the existing NVMe, AHCI, xHCI, HDA,
  and Wi-Fi candidate features. Two uncalled private configuration-write
  wrappers were removed; driver-used configuration transactions are retained.
- Audio registration uses `stage13-8-test`, matching `woven_audio` and HDA.
- Extended input event candidates and their self-test use `stage13-7-test`.
  No non-keyboard producer exists in the ordinary build. Its byte input ABI,
  bounded queue, overflow accounting, and IRQ-safe locking are unchanged.
- Wi-Fi deferred-work consumers and BSP MSI destination selection follow the
  Wi-Fi candidate feature. The installed interrupt handler is unchanged.

These boundaries do not supply missing production drivers. They add no
syscalls, capabilities, kernel objects, locks, or asynchronous ownership paths.
Existing candidate acceptance tests remain enabled under their original
features; no new lint suppression was introduced.

The Stage 13.6 HID report-error branch now reports the HID error directly;
a duplicated HDA discovery block was removed. Stage 13.8 retains its original
audio codec/topology checks. The normal-release shell harness isolates the
bootloader dependency's nested Cargo installer in `target/bootloader`, while
an explicit parent `--target-dir` keeps the main artifact tree unchanged.
This avoids the parent/child release artifact-lock deadlock. The validated
results and physical-driver limits are recorded in the continuation audit.
=======
## Intel firmware container parsing follow-up (2026-09-23)

`wifi_firmware_tlv.rs` is the shared safe, allocation-free container parser.
It borrows immutable image bytes, bounds images at 4 MiB and sections at 1 MiB,
and returns at most 64 data sections, matching the existing Intel DMA owner
capacity. The generic firmware image contract retains its 16-section limit.
TLV 32 declares paging size; secure types 24/25 carry runtime/init sections.
Per-image separator state assigns LMAC, UMAC and paging groups without exposing
separator records as DMA data. Host tests include a pinned upstream AX200
container; the kernel probe exercises the same parser. See the
[follow-up audit](audit-stage13-10ac-firmware-parser.md) for validation and limits.
Successful parsing does not select a device ABI, authenticate firmware or
activate hardware; physical startup and RX integration remain outstanding.

The QEMU acceptance launchers explicitly use TCG with a 128-MiB translation
cache to bound host commit use. Guest RAM and CPU matrices are unchanged;
the cache setting does not alter guest allocator capacity or acceptance limits.
Prior-stage PowerShell calls use child script scopes in the same process rather
than retaining 27 nested shell processes. Exceptions and native exit checks
preserve fail-fast behavior. The follow-up aggregate remains blocked by host
guest-memory allocation failure; see the audit before treating it as accepted.

## Stage 13.10AC deferred Wi-Fi service (2026-09-23)

Historical AX200 labels 13.10A–AC extend Wi-Fi roadmap Stage 13.9. They do
not implement Bluetooth Stage 13.10. The [remediation audit](audit-stage13-10ac-remediation-2026-09-23.md)
records implementation, ownership, source changes, gates, and remaining limits.
Older sections below describe their recorded stage snapshots.

The opt-in Wi-Fi build now contains `wifi_runtime.rs`, a scheduler worker with
one owned CSR subscription and one coalescing completion. Vector `0xd0` records
an interrupt, publishes work, sends LAPIC EOI, and signals the worker through
the scheduler event latch. `irq_mailbox.rs` tags pending work with a non-reused
epoch. The worker claims that epoch, services the existing Intel CSR controller
under the rank-20 slot guard, drops the guard, and notifies the owner. No guard
crosses blocking or scheduler notification. Fatal errors retire the subscription
and keep RX disabled. Explicit disable clears both the CSR and saved mask.

Owner exit/termination retires queued work and removes the subscription under
the service guard, then frees the backing page outside it. The post-SMP QEMU
probe binds synthetic CSR storage, injects software interrupts through the real
IDT on the last online CPU, and tests worker wakeup, error propagation, stale
epochs, and owner cleanup. It complements the earlier pre-SMP CSR tests.

This introduces no syscall, user pointer, capability, or WovenGuard change.
The experimental transport remains under `stage13-9-test`; default networking
uses VirtIO. Physical discovery is read-only and does not authorize DMA/MSI.
Firmware/RX-ring bring-up, actual RX/ALIVE delivery, and physical AX200 testing
remain open. Hardware vector reuse additionally requires device/interrupt
quiescence: software epochs cannot distinguish late untagged PCI MSIs.

Optional entropy, input, audio, and PCI programming declarations now follow
the feature boundaries of their consumers. `hal/pci/msi.rs` contains the
existing MSI protocol. The heap growth boot probe is excluded only from host
`cfg(test)` builds; host allocator tests and QEMU growth checks both remain.

The host image builder pins bootloader 0.11.17 through `vendor/bootloader`.
Its UEFI build script isolates the nested Cargo installer under its own
`OUT_DIR`, avoiding a wait on the parent's project-local artifact lock.
Runtime bootloader code is unchanged; provenance and the patch scope are in
`vendor/bootloader/WOVENHAT-PATCH.md`.

TCP/UDP descriptor close and owner teardown retain the bounded socket slot
after the last async pin while queued output drains. Normal network polling
reaps the entry after TCP acknowledgment/closure or UDP dispatch, with a
30-second bound for an unresponsive peer. Draining slots still count in socket
statistics and cannot be reused early. Stage 10.7 forces close-before-poll to
regress payload loss from premature socket removal.
>>>>>>> ad20d1a331df81e46ae48575036f1f520d5a6270

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
through 8 MiB. The allocator retains alignment padding with each allocation
and coalesces freed intervals. Kernel range mapping rolls back newly mapped
pages on a partial failure. Runtime growth maps 256 KiB chunks only after
releasing the heap guard; the boundary is published under that guard after
mapping succeeds. A rank-free, IRQ-enabled caller can grow synchronously on
allocation failure. BSP idle maintains mapped headroom when free space falls
below 1 MiB so guarded callers usually stay on the allocation-only path.
The growth window remains in the P4 slot already shared by user address
spaces. See the [growth audit](audit-stage6-heap-growth-2026-09-17.md).
The rank-40 physical-frame allocator stores one allocation bitmap in reserved
pages at the start of each usable RAM range. Returned frames beyond its 4,096
entry hot cache remain reusable through a bitmap scan; the bit also rejects
duplicate returns. Bitmap pages are excluded from allocatable-frame counts.
The heap now stores each live allocation's span in a header before its
payload, and stores free-list links inside freed spans. Its rank-50 guard
serializes those metadata writes; no fixed live-object or free-interval table
remains. The current mapped-byte ceiling and linear free-list search remain
separate limits.
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

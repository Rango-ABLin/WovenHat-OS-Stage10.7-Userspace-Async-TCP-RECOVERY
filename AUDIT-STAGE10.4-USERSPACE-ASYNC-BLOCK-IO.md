# Stage 10.4 Audit — Userspace Asynchronous Block I/O

## Boundary introduced
Stage 10.4 connects Ring-3 to the existing real block worker without exposing user
virtual addresses to asynchronous worker context. The API is intentionally a
privileged storage-service surface, not a general application raw-disk API.

## Invariants
- User-domain capability ceiling is unchanged; raw storage requires `StorageIo`.
- Block handles are minted only by `AsyncBlockRead`/`AsyncBlockWrite`.
- Every request has a TaskId owner and generation-tagged async handle.
- The worker owns/copies a 512-byte kernel bounce buffer; it never retains a user pointer.
- Completion copyout occurs before queue/handle consumption and is retry-safe.
- Cancellation removes the queue association before invalidating the async handle.
- Exit and forced termination remove owner queue entries before generic async owner reap.
- Queue slot reuse is protected by request-id matching; late workers cannot attach to a new request.
- The acceptance write targets LBA `u64::MAX`; ATA bounds validation rejects it before media I/O.

## Security
A new `spawn_user_system_service` path creates Ring-3 service tasks only when the
requested capability set is within the SystemService domain ceiling. Stage 10.4's
probe receives only `StorageIo`. Ordinary User-domain programs keep the existing
userspace capability set and cannot call the raw block API successfully.

## Acceptance evidence required
The Stage 10.4 serial marker is emitted only after a dedicated Ring-3 service exits
0 and telemetry proves at least four queued operations, at least two worker
completions, zero residual block queue entries, zero generic async slots, and an
owner-reap increment. The outer PowerShell acceptance then requires that marker on
1, 2 and 4 CPU production boots after the full Stage 10.3 regression chain.


## Preservation build dead-code fix
The isolated Stage 10.4 probe is feature-gated. Its helper spawn API, ELF-construction function, and linker-symbol declarations are now gated with `#[cfg(feature = "stage10-4-test")]` as well. This keeps ordinary qemu-test preservation builds warning-free under `-D warnings` while compiling the helpers normally in dedicated Stage 10.4 boots. No `allow(dead_code)` suppression was added.

## 1-CPU blocking-syscall handoff fix
The isolated Stage 10.4 run exposed the first genuinely pending Ring-3 async wait. `int 0x80` enters through an interrupt gate with IF cleared, while WovenHat's stack switch saves RSP/callee-saved registers but not RFLAGS. `task::wait_for_event()` previously switched to the replacement task with IF still cleared, allowing the replacement/bootstrap task to reach HLT with interrupts disabled and stall indefinitely on one CPU.

The event-wait handoff now enables interrupts only for the switched-away execution interval. When the blocked syscall task resumes after `switch_stacks`, its original IF state is restored before returning through the syscall frame. This preserves the syscall's interrupt-gate semantics while ensuring the scheduled replacement task can receive timer/IPI interrupts.

Dedicated `stage10-4-test` boots now exit through the QEMU debug-exit port immediately after the Stage 10.4 marker. Full Stage 10.3 production validation is run first by the acceptance chain, so later legacy boot assertions are not contaminated by the extra storage-service process. The QEMU runner also prints the serial tail automatically on any future timeout.

The same IF-handoff correction is applied to `sleep_current()`, because Ring-3 nanosleep reaches the scheduler through the same interrupt-gate boundary and depends on timer interrupts to wake. This fixes the root scheduler class rather than special-casing block I/O.

## Final scheduler IF isolation fix

The first blocking Ring-3 block-I/O wait exposed that the low-level scheduler context switch saved only RSP plus callee-saved registers, not RFLAGS. Because syscall/interrupt gates clear IF, a resumed task could inherit another task's interrupt-disabled state. An attempted high-level workaround enabled interrupts before `switch_stacks`, but that violated the pre-existing GDT/TSS invariant requiring the privilege-stack hand-off to happen with interrupts disabled.

The final fix is architectural: `wovenhat_context_switch` now saves/restores RFLAGS as part of every schedulable kernel context. All synthetic first-dispatch stacks (kernel task, user task, fork child) include the corresponding RFLAGS slot with IF initially clear. High-level event/sleep paths no longer enable interrupts before `switch_stacks`; they preserve the atomic GDT/CR3/stack transition and rely on the incoming task's own saved flags. This keeps interrupt state task-local and preserves the scheduler's existing critical-section contract.


FINAL SUBMISSION-PATH FIX
- Ring-3 async block submission no longer calls the synchronous `should_queue()` predicate.
  int 0x80 enters with IF clear by design; asynchronous submission may safely copy and enqueue
  kernel-owned request state with IF=0, then return or block later through AsyncBlockWait.
- Stage 10.4 acceptance failures exit QEMU immediately with isa-debug-exit failure status instead
  of halting until the host timeout, so future faults are reported directly rather than disguised
  as hangs.

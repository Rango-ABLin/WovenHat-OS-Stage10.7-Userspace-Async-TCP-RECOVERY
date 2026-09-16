# WovenHat OS Stage 10.3 Audit — Userspace Async Completion ABI

## Objective

Expose the Stage 10.2 generic request/completion substrate safely to Ring 3 while
preserving event-driven waiting, WovenGuard least privilege, generation safety,
and deterministic process teardown.

## Architectural changes

### `kernel/src/async_op.rs`

- Adds stable `Handle::to_raw/from_raw` encoding with reserved-bit validation.
- Makes the completion ABI explicitly 16 bytes (`status`, reserved, `value`).
- Adds non-consuming `peek_current` and `wait_ready` so syscall copyout can fail
  without destroying a ready result.
- Adds owner cancellation and task-owner reclamation.
- Adds owner-managed completion only for `AsyncClass::Service`.
- Keeps general `complete()` available to trusted kernel producers such as
  block I/O workers.

### `kernel/src/syscall.rs`

Adds syscalls 64–68: create, poll, wait, cancel and service-complete. Block,
Network and Device creation is gated through the existing Stage 9.5 resource
policy; Service uses current effective IPC authority. Poll/wait recheck current
authority so a previously created handle cannot bypass later policy tightening.
Cancellation intentionally remains owner-available after tightening so teardown
cannot be blocked by revocation.

### `kernel/src/userspace.rs`

The real bootstrap Ring-3 program exercises the ABI through `int 0x80`, including
pending poll, completion-before-wait, completion copyout, stale rejection,
cancellation, and one intentionally abandoned operation. libc-style stubs are
also exported for syscalls 64–68.

### `kernel/src/task.rs`

Both normal process exit and scheduler/signal-driven termination call
`async_op::release_owner(task_id)`, preventing leaked operation-table capacity.
The async table lock is the lifecycle linearization point versus producer
completion.

### `kernel/src/main.rs`

Boot acceptance requires Ring-3 syscall coverage, zero live async operations,
and owner-reap evidence before emitting the Stage 10.3 marker.

## Safety invariants

1. A raw handle cannot name generation zero or use reserved bits.
2. Only the owning task may inspect, wait, cancel, or service-complete a handle.
3. Hardware-class completion remains kernel-only.
4. Completion-before-wait is latched in table state.
5. Copyout failure leaves the completed slot intact for retry/cancel.
6. Cancellation and exit teardown invalidate the generation before late producer
   completion can affect a future occupant.
7. Current WovenGuard authority is re-evaluated on create/poll/wait.
8. Policy revocation cannot prevent owner cleanup.

## Acceptance

`RUN-STAGE10.3.ps1` preserves the complete Stage 10.2 chain and then requires the
Stage 10.3 production marker in 1-, 2- and 4-CPU serial logs.

## SMP preservation fix — 2026-09-14
A Stage 7.6 preservation stress run timed out on 2 CPUs at run 8 after seven consecutive passes. The Stage 10.3 Ring-3 probe originally allocated and unmapped a dedicated page solely to hold the 16-byte completion record. That unnecessarily coupled the new async ABI test to the VM allocator during every legacy userspace bootstrap. The probe now uses 32 bytes of scratch space below the existing mapped userspace stack pointer. This preserves the exact Ring-3 copyout validation while removing unrelated mmap/munmap paging churn from the preservation stress path.

WovenHat OS Stage 10.3 — Userspace Async Completion ABI
========================================================

Stage 10.3 promotes the Stage 10.2 generic async-operation substrate into a
Ring-3-visible lifecycle API without allowing userspace to forge hardware I/O
completions.

Syscall ABI
-----------
64 AsyncCreate(class)
   class: 0 Block, 1 Network, 2 Device, 3 Service
   returns generation-tagged owner-bound handle or UINT64_MAX.

65 AsyncPoll(handle, Completion*)
   returns 0 while pending, 1 after a completion is copied and consumed,
   UINT64_MAX on error/stale/policy denial.

66 AsyncWait(handle, Completion*)
   blocks on the scheduler event path (no busy polling), copies the completion,
   consumes it, and returns 1.

67 AsyncCancel(handle)
   owner-safe teardown; permitted even after policy tightening so revoked
   authority can always be destroyed safely.

68 AsyncComplete(handle, status, value)
   only an owner-held Service-class operation may be completed from Ring 3.
   Block/Network/Device completions remain kernel-producer-only.

Completion ABI (16 bytes)
-------------------------
bytes 0..3   i32 status
bytes 4..7   reserved (zero)
bytes 8..15  u64 value

Security/lifetime guarantees
----------------------------
* handles encode slot + generation; stale generations cannot alias later work;
* operations are task-owner-bound;
* create/poll/wait recheck the class's current WovenGuard authority;
* cancellation remains available after policy tightening for safe teardown;
* invalid Ring-3 copyout never consumes a ready result;
* exit() and remote/signal termination both reclaim abandoned async slots;
* service-owned completion cannot be used to forge device/network/block I/O.

Runtime proof
-------------
The normal Ring-3 bootstrap program performs create -> poll(pending) ->
service-complete -> wait(copyout), verifies the completion bytes, verifies stale
handle rejection, tests explicit cancellation, then deliberately abandons one
operation. Kernel boot validation requires the syscall coverage bits, zero live
async slots, and owner-reap evidence before printing the Stage 10.3 marker.

Validation
----------
Run from the project root:

  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\RUN-STAGE10.3.ps1

Expected final line:

  === STAGE 10.3 ACCEPTANCE: PASS ===

# WovenHat OS Stage 8.3 Audit — Race-Free IPC Events / Waits

## Baseline
Stage 8.3 is based on the exact Stage 8.2 CLIPPY-FIX source tree that passed the
full Stage 8.2 acceptance gate.

## Problem being solved
A naive blocking receive has a classic lost-wakeup interval:

1. receiver checks endpoint: empty;
2. receiver prepares to sleep;
3. sender queues a message and attempts to wake receiver;
4. receiver is not Blocked yet, so a plain wake is ignored;
5. receiver finally marks itself Blocked and may sleep forever.

Holding the IPC lock while publishing scheduler Blocked state would close this
race but is rejected here because the codebase already contains scheduler -> IPC
registration paths. Adding IPC -> scheduler acquisition would create a global
lock-order cycle.

## Chosen protocol
Stage 8.3 introduces a scheduler-owned one-bit event permit in each TCB.

- `signal_event(id)`:
  - if target is Blocked: Blocked -> Ready and reschedule owner CPU;
  - otherwise: set `event_pending = true`.
- `wait_for_event()`:
  - under the scheduler lock, consume `event_pending` and continue; or
  - publish Running -> Blocked and context-switch.

Therefore a signal racing before the block is latched, while a signal racing
after the block sees Blocked. There is no unrepresented gap.

## IPC waiter ownership
Each endpoint carries bounded FIFO waiter arrays:
- readable waiters for empty receive;
- writable waiters for full send.

Each waiter records process owner, exact generation-tagged handle and TaskId.
This permits exact-handle cleanup on close and owner cleanup on unregister.

## Preserved contracts
- Stage 8.2 `send_handle` remains nonblocking and returns QueueFull.
- Stage 8.2 `receive_handle` remains nonblocking and returns QueueEmpty.
- SEND/RECEIVE rights checks are unchanged.
- endpoint queue FIFO semantics are unchanged.
- generation-tagged stale-handle rejection is unchanged.
- legacy PID-addressed IPC ABI is unchanged.
- no IPC lock is held while calling scheduler event signal/wait functions.

## Runtime proof
The Stage 8.3 production probe creates one endpoint and restricted SEND/RECEIVE
handles in synthetic namespaces.

Phase A — empty queue:
- receiver calls blocking receive;
- sender waits until a read waiter is visibly registered;
- sender queues the wake message;
- receiver resumes and verifies payload/sender.

Phase B — full queue:
- sender fills the bounded queue;
- sender calls blocking send for one extra message;
- receiver waits until a write waiter is visibly registered;
- receiver removes one message, releasing one slot and signaling sender;
- sender resumes and enqueues the final message;
- receiver verifies complete FIFO ordering.

On 2/4 CPU boots, sender is pinned to the highest AP to exercise a remote CPU
scheduler wake. The probe has a bounded timer deadline and cleans IPC state on
success.

## Static audit status
- No changes to paging, process table, filesystem, networking or userspace ABI.
- No Stage 8.2 semantic weakening.
- No timeout increase or polling substitution for the blocking primitive.
- No IPC -> scheduler nested lock acquisition.
- Public Stage 8.3 functions are exercised by the production runtime probe to
  remain live under strict `-D warnings` builds.

## Validation status
IMPLEMENTED / STATIC-AUDITED ONLY until `run-stage8-3-acceptance.ps1` reaches:

`=== STAGE 8.3 ACCEPTANCE: PASS ===`

# Kernel IPC API

WovenHat provides bounded, capability-gated message queues for user processes. The
implementation lives in `kernel/src/ipc.rs`; syscall validation is in
`kernel/src/syscall.rs`.

## Limits and lifecycle

Limits are defined in `kernel/src/config.rs`:

- One endpoint is created for every user process.
- At most `MAX_IPC_ENDPOINTS` (currently 32) endpoints exist.
- Each endpoint holds `IPC_QUEUE_DEPTH` (currently 16) FIFO messages.
- Each message carries at most `MAX_MESSAGE_SIZE` (currently 256) bytes and records the sender PID.
- Reaping a process removes its endpoint and all queued messages.
- Duplicate endpoints, unknown destinations, full queues, and oversized messages fail
  deterministically.
- No heap allocation occurs in the IPC path.

## Capability

Both sender and receiver syscalls require `Capability::Ipc`. The bootstrap kernel and
the default userspace capability set currently receive this capability. Future process
launch policy can remove it or delegate it explicitly.

## Syscalls

| Number | Name | arg0 | arg1 | arg2 | Success |
| --- | --- | --- | --- | --- | --- |
| 11 | `MessageSend` | receiver PID | user payload pointer | payload length | `0` |
| 12 | `MessageReceive` | user payload pointer | buffer capacity | user sender-PID pointer | payload length |

Failures return `u64::MAX`.

The receive path peeks before copying. Capacity and user-memory failures therefore leave
the message queued. After both payload and sender PID are copied successfully, the
message is removed atomically on the current single-core kernel.

## Security properties

- Payload lengths are checked before stack-buffer use.
- All user reads and writes pass through page-table permission validation.
- Sender identity comes from the scheduler and cannot be supplied by userspace.
- A process can receive only from its own endpoint.
- Endpoint registration and cleanup follow process creation and reaping.

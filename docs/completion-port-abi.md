# Completion ports: Stage 10.8

Ports belong to a process. An operation retains its existing task ownership and
class-specific submission and collection permissions. Association requires both
the operation and the port to belong to the caller. No capability is granted by
creating or using a port.

| Syscall | Arguments (rdi, rsi, rdx) | Result |
| --- | --- | --- |
| 82 Create | none | Port handle |
| 83 Close | port | 0 |
| 84 Associate | operation, port, cookie | 0 |
| 85 Poll | port, destination, capacity | Number of records |
| 86 Wait | port, destination, options pointer | Number of records |

All calls return `u64::MAX` on error. Capacity is 1 through 8 records. Capacity 1
implements wait-one; larger capacities dequeue a batch of currently completed
operations. Wait returns as soon as at least one record is available, rather than
waiting for the requested capacity to fill. Poll returns zero when empty.

Wait options contain two little-endian u64 values: capacity and relative timeout
in monotonic kernel ticks (100 Hz). Zero is nonblocking; `u64::MAX` is indefinite.
Other timeouts are converted once to an absolute deadline; arithmetic overflow
is rejected. Empty waits return zero at expiry. Timeout ends this wait only; it
does not cancel the associated operations. Wakeups recheck the queue and the
original deadline, so unrelated scheduler events cannot extend the timeout.

Each record is 40 bytes, explicitly encoded with zeroed padding:

| Offset | Type | Meaning |
| --- | --- | --- |
| 0 | u64 | Original generation-tagged operation handle |
| 8 | u64 | Application cookie supplied at association |
| 16 | u32 | Existing async operation class |
| 20 | u32 | Flags: bit 0 means cancellation |
| 24 | i32 | Completion status; cancellation is -2 |
| 28 | u32 | Reserved, zero |
| 32 | u64 | Completion value |

Dequeue consumes the notification, not the underlying I/O result. Applications
still use the appropriate existing collector to copy data and release operation
resources. A canceled operation has already been released and must not be
collected. Completion that wins a cancellation race remains a completion event.

There are 32 ports globally, at most four per process, with 32 reserved or ready
records per port. Association reserves capacity before succeeding. Full ports
reject new associations without modifying the operation; accepted producers
never lose notifications to queue overflow. Each operation may be associated
only once. Association after completion immediately queues its notification.
Records are ordered by publication. Sequence exhaustion preserves ordering by
renumbering the bounded live set.

Port handles have a 16-bit slot, the tag `0xc001` in bits 16 through 31 and a
nonzero 32-bit generation. Exhausted port and operation generations retire the
slot instead of wrapping. Closing a port discards notifications and wakes its
waiters; it does not cancel underlying I/O. A later port cannot receive an old
producer's completion. Owner teardown removes associations and ports without
recursively acquiring the scheduler lock.

A consumer claims a batch while copying to userspace, without retaining a kernel
lock across paging. A failed copy leaves the records queued for retry. Concurrent
consumers encountering an active claim receive an error and may retry; this is
bounded serialization, not a fairness guarantee. Future multithreaded processes
must add individual thread-exit cleanup for claims and wait registrations.

The lock order is operation table -> individual port. Port allocation additionally
uses allocation lock -> individual port. Both use IRQ-safe locks. Producer
notification occurs only after all these locks are released. Queue inspection
and waiter registration are atomic under the port lock, and the scheduler's
event latch closes the registration-to-sleep race.

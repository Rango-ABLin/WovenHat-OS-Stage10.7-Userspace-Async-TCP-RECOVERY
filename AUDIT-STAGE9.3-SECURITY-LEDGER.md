# Stage 9.3 — WovenGuard Security Audit Ledger

## Purpose
Convert the pre-existing diagnostic audit ring into a kernel security ledger
that can become the data source for a later graphical WovenHat Security Center.

## Architecture
The ledger remains fixed-size and allocation-free. Writers from syscalls,
WovenGuard lineage code, task policy and fault paths commit under a single SMP
spin mutex with local interrupts disabled, preventing same-CPU interrupt
re-entry deadlock while preserving global event ordering across CPUs.

Each retained event records a monotonic sequence, timer tick, CPU, actor, action,
target, action-specific detail and explicit allowed/denied outcome.

## Retention semantics
Capacity is 128 events. Once full, the oldest event is overwritten. The ledger
tracks the number of overwritten records so consumers can distinguish a complete
window from a truncated history. `recent_into` copies events oldest-to-newest
without allocating.

## Stage 9.3 proof
The local self-test proves wraparound, ordering and overwrite accounting without
mutating production history. The production runtime probe writes two sentinel
WovenGuard decisions through the real global path and verifies sequence order,
CPU attribution, structured fields, outcomes and bounded retention.

## Non-goals
Persistent storage, cryptographic sealing, user-space export policy and the GUI
Security Center are intentionally deferred. Stage 9.3 establishes the trusted
kernel event substrate first.

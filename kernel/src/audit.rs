use crate::irq_lock::IrqMutex as Mutex;

/// Fixed-size, allocation-free WovenGuard security ledger.
///
/// The ledger intentionally overwrites the oldest event when full. Security
/// logging must never allocate, block on storage, or make the kernel fail just
/// because diagnostic history reached capacity.
pub const CAPACITY: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    CapabilityGrant,
    CapabilityRevoke,
    IpcSend,
    FileWrite,
    ProcessFault,
    ProcessExec,
    ProcessFork,
    WovenGuardAllow,
    WovenGuardDeny,
    CapabilityLineageIssue,
    CapabilityDerived,
    CapabilityLineageRelease,
    CapabilityLineageRevoke,
    SandboxProfileBind,
    SandboxProfileDeny,
    SandboxServicePublish,
    SandboxServiceDiscover,
    SandboxServiceUse,
    SandboxFileRead,
    SandboxFileWrite,
    SandboxDeviceAccess,
}

/// One immutable security-ledger record.
///
/// `sequence` supplies total ordering across CPUs. `tick` provides the kernel
/// time base, while `cpu` identifies the CPU that committed the record.
/// `detail` is action-specific structured metadata (zero for legacy callers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub sequence: u64,
    pub tick: u64,
    pub cpu: u16,
    pub actor: u64,
    pub action: Action,
    pub target: u64,
    pub detail: u64,
    pub allowed: bool,
}

/// Read-only metadata for Security Center / diagnostics consumers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerStatus {
    pub retained: usize,
    pub capacity: usize,
    pub next_sequence: u64,
    pub overwritten: u64,
}

struct Log {
    events: [Option<Event>; CAPACITY],
    next: usize,
    count: usize,
    sequence: u64,
    overwritten: u64,
}

impl Log {
    const fn new() -> Self {
        Self {
            events: [None; CAPACITY],
            next: 0,
            count: 0,
            sequence: 0,
            overwritten: 0,
        }
    }

    fn record(&mut self, mut event: Event) {
        event.sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        if self.count == CAPACITY {
            self.overwritten = self.overwritten.wrapping_add(1);
        }
        self.events[self.next] = Some(event);
        self.next = (self.next + 1) % CAPACITY;
        self.count = core::cmp::min(self.count + 1, CAPACITY);
    }

    fn latest(&self) -> Option<Event> {
        if self.count == 0 {
            None
        } else {
            self.events[(self.next + CAPACITY - 1) % CAPACITY]
        }
    }

    /// Copy retained events oldest -> newest into a caller-provided bounded
    /// buffer. If the destination is smaller than the ledger, return the most
    /// recent destination-length events while preserving chronological order.
    fn recent_into(&self, output: &mut [Option<Event>]) -> usize {
        let copied = core::cmp::min(self.count, output.len());
        for slot in output.iter_mut() {
            *slot = None;
        }
        if copied == 0 {
            return 0;
        }

        let oldest = (self.next + CAPACITY - self.count) % CAPACITY;
        let skip = self.count - copied;
        for (destination, offset) in output.iter_mut().take(copied).zip(skip..self.count) {
            *destination = self.events[(oldest + offset) % CAPACITY];
        }
        copied
    }

    const fn status(&self) -> LedgerStatus {
        LedgerStatus {
            retained: self.count,
            capacity: CAPACITY,
            next_sequence: self.sequence,
            overwritten: self.overwritten,
        }
    }
}

/// Fixed-size audit ledger. Rank 40 allows security records to be committed
/// from scheduler, IPC, and lineage transactions without reversing those
/// lower-ranked ownership locks.
static LOG: Mutex<Log> = Mutex::with_rank(Log::new(), 40);

/// Commit one bounded ledger operation with local interrupts disabled for the
/// complete guard lifetime. Audit records can be emitted from exception
/// handlers as well as ordinary task/syscall paths.
fn with_log<R>(operation: impl FnOnce(&mut Log) -> R) -> R {
    let mut log = LOG.lock();
    operation(&mut log)
}

pub fn record(actor: u64, action: Action, target: u64, allowed: bool) {
    record_detail(actor, action, target, 0, allowed);
}

pub fn record_detail(actor: u64, action: Action, target: u64, detail: u64, allowed: bool) {
    let event = Event {
        sequence: 0,
        tick: crate::timer::ticks(),
        cpu: crate::smp::cpu_index() as u16,
        actor,
        action,
        target,
        detail,
        allowed,
    };
    with_log(|log| log.record(event));
}

pub fn latest() -> Option<Event> {
    with_log(|log| log.latest())
}

pub fn count() -> usize {
    with_log(|log| log.count)
}

pub fn status() -> LedgerStatus {
    with_log(|log| log.status())
}

pub fn recent_into(output: &mut [Option<Event>]) -> usize {
    with_log(|log| log.recent_into(output))
}

/// Allocation-free local proof for ring ordering, retention and overwrite
/// accounting. This does not mutate the production global ledger.
pub fn self_test() -> bool {
    let mut log = Log::new();
    for index in 0..(CAPACITY + 7) {
        log.record(Event {
            sequence: 0,
            tick: index as u64,
            cpu: (index % 4) as u16,
            actor: 1,
            action: Action::CapabilityGrant,
            target: 2,
            detail: index as u64,
            allowed: index % 2 == 0,
        });
    }

    let mut recent = [None; 4];
    let copied = log.recent_into(&mut recent);
    let first_expected = (CAPACITY + 3) as u64;
    let status = log.status();

    status.retained == CAPACITY
        && status.capacity == CAPACITY
        && status.next_sequence == (CAPACITY + 7) as u64
        && status.overwritten == 7
        && copied == recent.len()
        && recent.iter().enumerate().all(|(index, entry)| {
            entry.is_some_and(|event| {
                event.sequence == first_expected + index as u64
                    && event.detail == first_expected + index as u64
            })
        })
        && log.latest().is_some_and(|event| {
            event.sequence == (CAPACITY + 6) as u64
                && event.tick == (CAPACITY + 6) as u64
                && event.actor == 1
                && event.target == 2
                && event.detail == (CAPACITY + 6) as u64
                && event.allowed
        })
}

/// Stage 9.3 production-ledger proof.
///
/// Two sentinel decisions are committed through the same global API used by
/// WovenGuard, syscalls and fault paths. The proof checks globally monotonic
/// ordering, CPU attribution, action/result fidelity, bounded retention and
/// chronological readout without clearing prior security history.
pub fn stage9_3_runtime_probe() -> bool {
    const ACTOR: u64 = u64::MAX - 0x9300;
    const ALLOW_TARGET: u64 = u64::MAX - 0x9301;
    const DENY_TARGET: u64 = u64::MAX - 0x9302;
    const ALLOW_DETAIL: u64 = 0x9300_0001;
    const DENY_DETAIL: u64 = 0x9300_0002;

    let before = status();
    let cpu = crate::smp::cpu_index() as u16;

    record_detail(
        ACTOR,
        Action::WovenGuardAllow,
        ALLOW_TARGET,
        ALLOW_DETAIL,
        true,
    );
    record_detail(
        ACTOR,
        Action::WovenGuardDeny,
        DENY_TARGET,
        DENY_DETAIL,
        false,
    );

    let after = status();
    let mut recent = [None; 2];
    if recent_into(&mut recent) != 2 {
        return false;
    }
    let Some(allow) = recent[0] else {
        return false;
    };
    let Some(deny) = recent[1] else {
        return false;
    };

    let expected_overwrites = before
        .overwritten
        .saturating_add((before.retained + 2).saturating_sub(CAPACITY) as u64);

    after.capacity == CAPACITY
        && after.retained == core::cmp::min(before.retained + 2, CAPACITY)
        && after.next_sequence == before.next_sequence.wrapping_add(2)
        && after.overwritten == expected_overwrites
        && allow.sequence == before.next_sequence
        && deny.sequence == before.next_sequence.wrapping_add(1)
        && allow.cpu == cpu
        && deny.cpu == cpu
        && allow.actor == ACTOR
        && deny.actor == ACTOR
        && allow.action == Action::WovenGuardAllow
        && deny.action == Action::WovenGuardDeny
        && allow.target == ALLOW_TARGET
        && deny.target == DENY_TARGET
        && allow.detail == ALLOW_DETAIL
        && deny.detail == DENY_DETAIL
        && allow.allowed
        && !deny.allowed
        && deny.tick >= allow.tick
}

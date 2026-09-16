//! Bounded process-thread registry used by the Stage 11.2 runtime boundary.
use spin::Mutex;

pub const MAX_THREADS: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ThreadId(u64);

impl ThreadId {
    pub const fn raw(self) -> u64 { self.0 }
    const fn new(slot: usize, generation: u32) -> Self {
        Self(((generation as u64) << 32) | slot as u64)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State { Free, Running, Exited }

#[derive(Clone, Copy)]
struct Record {
    state: State,
    generation: u32,
    owner: u64,
    exit: i32,
    errno: i32,
    tls: u64,
}

impl Record {
    const EMPTY: Self = Self { state: State::Free, generation: 1, owner: 0, exit: 0, errno: 0, tls: 0 };
}

struct Table { records: [Record; MAX_THREADS] }
impl Table { const fn new() -> Self { Self { records: [Record::EMPTY; MAX_THREADS] } } }
static TABLE: Mutex<Table> = Mutex::new(Table::new());

fn lookup(table: &mut Table, id: ThreadId) -> Option<&mut Record> {
    let slot = (id.0 & 0xffff_ffff) as usize;
    let generation = (id.0 >> 32) as u32;
    let record = table.records.get_mut(slot)?;
    (record.generation == generation && record.state != State::Free).then_some(record)
}

pub fn create(owner: u64) -> Option<ThreadId> {
    let mut table = TABLE.lock();
    let (slot, record) = table.records.iter_mut().enumerate().find(|(_, r)| r.state == State::Free)?;
    record.state = State::Running;
    record.owner = owner;
    record.exit = 0;
    record.errno = 0;
    record.tls = 0;
    Some(ThreadId::new(slot, record.generation))
}

pub fn terminate(id: ThreadId, exit: i32) -> bool {
    let mut table = TABLE.lock();
    let Some(record) = lookup(&mut table, id) else { return false };
    if record.state != State::Running { return false }
    record.state = State::Exited;
    record.exit = exit;
    true
}

pub fn join(owner: u64, id: ThreadId) -> Result<i32, JoinError> {
    let mut table = TABLE.lock();
    let Some(record) = lookup(&mut table, id) else { return Err(JoinError::Invalid) };
    if record.owner != owner { return Err(JoinError::NotOwner) }
    if record.state == State::Running { return Err(JoinError::Running) }
    let exit = record.exit;
    record.state = State::Free;
    record.generation = record.generation.checked_add(1).filter(|g| *g != 0).unwrap_or(u32::MAX);
    Ok(exit)
}

/// Reclaim every thread record owned by a process that is terminating.
///
/// The generation is advanced for each reclaimed slot, so a stale handle
/// from the terminated process cannot address a subsequently created thread.
pub fn reap_owner(owner: u64) -> usize {
    let mut table = TABLE.lock();
    let mut reclaimed = 0;
    for record in &mut table.records {
        if record.state != State::Free && record.owner == owner {
            record.state = State::Free;
            record.owner = 0;
            record.exit = 0;
            record.errno = 0;
            record.tls = 0;
            record.generation = record
                .generation
                .checked_add(1)
                .filter(|generation| *generation != 0)
                .unwrap_or(u32::MAX);
            reclaimed += 1;
        }
    }
    reclaimed
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JoinError { Invalid, NotOwner, Running }

pub fn set_tls(id: ThreadId, value: u64) -> bool { let mut t = TABLE.lock(); lookup(&mut t, id).map(|r| r.tls = value).is_some() }
pub fn tls(id: ThreadId) -> Option<u64> { let mut t = TABLE.lock(); lookup(&mut t, id).map(|r| r.tls) }
pub fn set_errno(id: ThreadId, value: i32) -> bool { let mut t = TABLE.lock(); lookup(&mut t, id).map(|r| r.errno = value).is_some() }
pub fn errno(id: ThreadId) -> Option<i32> { let mut t = TABLE.lock(); lookup(&mut t, id).map(|r| r.errno) }

#[cfg(feature = "stage11-2-test")]
pub fn structural_self_test() -> bool {
    let Some(id) = create(1) else { return false };
    set_tls(id, 0xfeed_beef) && set_errno(id, 37) && tls(id) == Some(0xfeed_beef)
        && errno(id) == Some(37) && join(1, id) == Err(JoinError::Running)
        && terminate(id, 23) && join(1, id) == Ok(23) && tls(id).is_none()
        && {
            let Some(stale) = create(9) else { return false };
            set_tls(stale, 0x1234) && reap_owner(9) == 1 && tls(stale).is_none()
                && create(9).is_some_and(|fresh| fresh != stale)
        }
}

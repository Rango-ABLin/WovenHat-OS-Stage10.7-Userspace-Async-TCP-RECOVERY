//! Stage 10.2/10.3 generic asynchronous operation/completion substrate.
//!
//! Producers allocate a bounded generation-tagged operation for the current
//! task, perform work elsewhere, and complete it through [`complete`].  The
//! owner can then [`wait`] without polling.  Wait registration and completion
//! are serialized by one table lock, so completion-before-sleep cannot be lost.
//!
//! Stage 10.3 adds a stable raw-handle ABI, non-consuming readiness inspection,
//! owner cancellation, service-owned completion, and deterministic owner
//! teardown.  The split between `wait_ready` and `release_current` is deliberate:
//! userspace syscall code can copy a completion to Ring 3 before consuming the
//! kernel slot, so a bad userspace pointer never destroys a completed result.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::irq_lock::IrqMutex as Mutex;

use crate::{
    config::MAX_ASYNC_OPERATIONS,
    task::{self, TaskId},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AsyncClass {
    Block = 0,
    Network = 1,
    Device = 2,
    Service = 3,
    File = 4,
    Timer = 5,
    Event = 6,
}

impl AsyncClass {
    pub const fn from_u64(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::Block),
            1 => Some(Self::Network),
            2 => Some(Self::Device),
            3 => Some(Self::Service),
            4 => Some(Self::File),
            5 => Some(Self::Timer),
            6 => Some(Self::Event),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    slot: u16,
    generation: u32,
}

impl Handle {
    /// Stable Stage 10.3 userspace representation.
    ///
    /// Bits 0..15 are the bounded table slot, bits 32..63 are the generation,
    /// and bits 16..31 are reserved and must be zero. Generation zero is never
    /// allocated and therefore cannot name a live operation.
    pub const fn to_raw(self) -> u64 {
        ((self.generation as u64) << 32) | self.slot as u64
    }

    pub const fn from_raw(raw: u64) -> Option<Self> {
        if raw & 0x0000_ffff_0000 != 0 {
            return None;
        }
        let generation = (raw >> 32) as u32;
        let slot = (raw & 0xffff) as u16;
        if generation == 0 || slot as usize >= MAX_ASYNC_OPERATIONS {
            return None;
        }
        Some(Self { slot, generation })
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    pub status: i32,
    pub reserved: u32,
    pub value: u64,
}

impl Completion {
    pub const OK: Self = Self {
        status: 0,
        reserved: 0,
        value: 0,
    };

    pub const fn new(status: i32, value: u64) -> Self {
        Self {
            status,
            reserved: 0,
            value,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Full,
    NoCurrentTask,
    InvalidHandle,
    NotOwner,
    AlreadyComplete,
    WrongClass,
    AlreadyAssociated,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Free,
    Pending,
    Complete,
}

#[derive(Clone, Copy)]
struct Slot {
    generation: u32,
    state: State,
    class: AsyncClass,
    owner: TaskId,
    waiting: bool,
    completion: Completion,
    port: Option<u64>,
}

impl Slot {
    const fn empty() -> Self {
        Self {
            generation: 0,
            state: State::Free,
            class: AsyncClass::Block,
            owner: TaskId::from_u64(0),
            waiting: false,
            completion: Completion::OK,
            port: None,
        }
    }

    fn release(&mut self) {
        self.state = State::Free;
        self.waiting = false;
        self.completion = Completion::OK;
        self.port = None;
    }
}

struct Table {
    slots: [Slot; MAX_ASYNC_OPERATIONS],
}

impl Table {
    const fn new() -> Self {
        Self {
            slots: [const { Slot::empty() }; MAX_ASYNC_OPERATIONS],
        }
    }

    fn allocate(&mut self, owner: TaskId, class: AsyncClass) -> Option<Handle> {
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.state != State::Free || slot.generation == u32::MAX {
                continue;
            }
            slot.generation += 1;
            slot.state = State::Pending;
            slot.class = class;
            slot.owner = owner;
            slot.waiting = false;
            slot.completion = Completion::OK;
            return Some(Handle {
                slot: index as u16,
                generation: slot.generation,
            });
        }
        None
    }

    fn get_mut(&mut self, handle: Handle) -> Option<&mut Slot> {
        let slot = self.slots.get_mut(handle.slot as usize)?;
        (slot.state != State::Free && slot.generation == handle.generation).then_some(slot)
    }

    fn active_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.state != State::Free)
            .count()
    }
}

static TABLE: Mutex<Table> = Mutex::with_rank(Table::new(), 10);
static ALLOCATED: AtomicU64 = AtomicU64::new(0);
static COMPLETED: AtomicU64 = AtomicU64::new(0);
static WAITS: AtomicU64 = AtomicU64::new(0);
static SIGNALS: AtomicU64 = AtomicU64::new(0);
static RELEASED: AtomicU64 = AtomicU64::new(0);
static CANCELLED: AtomicU64 = AtomicU64::new(0);
static OWNER_REAPED: AtomicU64 = AtomicU64::new(0);
static STALE_REJECTIONS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stats {
    pub allocated: u64,
    pub completed: u64,
    pub waits: u64,
    pub signals: u64,
    pub released: u64,
    pub cancelled: u64,
    pub owner_reaped: u64,
    pub stale_rejections: u64,
    pub active: usize,
}

pub fn allocate_current(class: AsyncClass) -> Result<Handle, Error> {
    let owner = task::current_task_id_if_running().ok_or(Error::NoCurrentTask)?;
    let handle = TABLE.lock().allocate(owner, class).ok_or(Error::Full)?;
    ALLOCATED.fetch_add(1, Ordering::Relaxed);
    Ok(handle)
}

pub fn class_current(handle: Handle) -> Result<AsyncClass, Error> {
    let owner = task::current_task_id_if_running().ok_or(Error::NoCurrentTask)?;
    let mut table = TABLE.lock();
    let Some(slot) = table.get_mut(handle) else {
        STALE_REJECTIONS.fetch_add(1, Ordering::Relaxed);
        return Err(Error::InvalidHandle);
    };
    if slot.owner != owner {
        return Err(Error::NotOwner);
    }
    Ok(slot.class)
}

/// Completes an operation from any producer/worker context.
///
/// A scheduler event is emitted only when the owner has already registered a
/// wait. If completion wins the race, the completed state itself is the latch;
/// a later wait observes the result directly and no stale event permit is
/// created.
pub fn complete(handle: Handle, completion: Completion) -> Result<(), Error> {
    let (waiter, port_waiters) = {
        let mut table = TABLE.lock();
        let Some(slot) = table.get_mut(handle) else {
            STALE_REJECTIONS.fetch_add(1, Ordering::Relaxed);
            return Err(Error::InvalidHandle);
        };
        if slot.state == State::Complete {
            return Err(Error::AlreadyComplete);
        }
        slot.completion = completion;
        slot.state = State::Complete;
        let wake = slot.port.map(|port| crate::completion_port::publish(
            port, handle.to_raw(), slot.class as u32, 0, completion.status, completion.value));
        (slot.waiting.then_some(slot.owner), wake)
    };

    COMPLETED.fetch_add(1, Ordering::Relaxed);
    if let Some(waiter) = waiter {
        SIGNALS.fetch_add(1, Ordering::Relaxed);
        let _ = task::signal_event(waiter);
    }
    if let Some(wake) = port_waiters { crate::completion_port::notify(wake); }
    Ok(())
}

/// TABLE -> port ordering linearizes association against producer completion.
/// A reservation is made first; full ports reject association without losing
/// either the operation or a future completion. Already-complete ops enqueue now.
pub fn associate_current(handle: Handle, port: u64, cookie: u64) -> Result<(), Error> {
    let owner = task::current_task_id_if_running().ok_or(Error::NoCurrentTask)?;
    let process = task::current_process_id();
    let wake = {
        let mut table = TABLE.lock();
        let slot = table.get_mut(handle).ok_or(Error::InvalidHandle)?;
        if slot.owner != owner { return Err(Error::NotOwner); }
        if slot.port.is_some() { return Err(Error::AlreadyAssociated); }
        crate::completion_port::reserve(process, port, handle.to_raw(), cookie).map_err(|_| Error::Full)?;
        slot.port = Some(port);
        if slot.state == State::Complete {
            Some(crate::completion_port::publish(port, handle.to_raw(), slot.class as u32, 0,
                slot.completion.status, slot.completion.value))
        } else { None }
    };
    if let Some(wake) = wake { crate::completion_port::notify(wake); }
    Ok(())
}

/// Userspace-service completion path. Hardware/device completion remains a
/// kernel producer responsibility: Ring 3 may complete only its own Service
/// class operations.
pub fn complete_current_service(handle: Handle, completion: Completion) -> Result<(), Error> {
    let owner = task::current_task_id_if_running().ok_or(Error::NoCurrentTask)?;
    {
        let mut table = TABLE.lock();
        let Some(slot) = table.get_mut(handle) else {
            STALE_REJECTIONS.fetch_add(1, Ordering::Relaxed);
            return Err(Error::InvalidHandle);
        };
        if slot.owner != owner {
            return Err(Error::NotOwner);
        }
        if slot.class != AsyncClass::Service {
            return Err(Error::WrongClass);
        }
    }
    complete(handle, completion)
}

/// Nonblocking readiness inspection. A completed result is *not* consumed.
/// This is the safe primitive for Ring-3 copyout: callers may retry after an
/// invalid userspace destination without losing the completion.
pub fn peek_current(handle: Handle) -> Result<Option<Completion>, Error> {
    let owner = task::current_task_id_if_running().ok_or(Error::NoCurrentTask)?;
    let mut table = TABLE.lock();
    let Some(slot) = table.get_mut(handle) else {
        STALE_REJECTIONS.fetch_add(1, Ordering::Relaxed);
        return Err(Error::InvalidHandle);
    };
    if slot.owner != owner {
        return Err(Error::NotOwner);
    }
    Ok((slot.state == State::Complete).then_some(slot.completion))
}

/// Blocks until a completion is ready but leaves the slot allocated.
/// Userspace syscall code releases only after successful copyout.
pub fn wait_ready(handle: Handle) -> Result<Completion, Error> {
    let owner = task::current_task_id_if_running().ok_or(Error::NoCurrentTask)?;
    loop {
        let completion = {
            let mut table = TABLE.lock();
            let Some(slot) = table.get_mut(handle) else {
                STALE_REJECTIONS.fetch_add(1, Ordering::Relaxed);
                return Err(Error::InvalidHandle);
            };
            if slot.owner != owner {
                return Err(Error::NotOwner);
            }
            if slot.state == State::Complete {
                Some(slot.completion)
            } else {
                // Serialized with complete(): once true, completion either
                // signals this owner or is visible on the next table check.
                slot.waiting = true;
                None
            }
        };

        if let Some(completion) = completion {
            return Ok(completion);
        }

        WAITS.fetch_add(1, Ordering::Relaxed);
        task::wait_for_event();
        // Unrelated events are permitted. Recheck generation-tagged state.
    }
}

/// Waits for and consumes one operation owned by the current task.
pub fn wait(handle: Handle) -> Result<Completion, Error> {
    let completion = wait_ready(handle)?;
    release_current(handle)?;
    Ok(completion)
}

/// Releases an operation owned by the current task, whether pending or already
/// complete. Producer rollback and successful userspace completion copyout both
/// use this primitive.
pub fn release_current(handle: Handle) -> Result<(), Error> {
    let owner = task::current_task_id_if_running().ok_or(Error::NoCurrentTask)?;
    let mut table = TABLE.lock();
    let Some(slot) = table.get_mut(handle) else {
        STALE_REJECTIONS.fetch_add(1, Ordering::Relaxed);
        return Err(Error::InvalidHandle);
    };
    if slot.owner != owner {
        return Err(Error::NotOwner);
    }
    let wake = if slot.state == State::Pending {
        slot.port.map(|port| crate::completion_port::publish(port, handle.to_raw(), slot.class as u32,
            crate::completion_queue::CANCELLED, -2, 0))
    } else { None };
    slot.release();
    drop(table);
    if let Some(wake) = wake { crate::completion_port::notify(wake); }
    RELEASED.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Explicit owner cancellation. If a producer races the cancellation, the
/// table lock defines the linearization point. A producer arriving after
/// cancellation observes a stale handle and cannot complete a later occupant.
pub fn cancel_current(handle: Handle) -> Result<(), Error> {
    let owner = task::current_task_id_if_running().ok_or(Error::NoCurrentTask)?;
    let mut table = TABLE.lock();
    let Some(slot) = table.get_mut(handle) else {
        STALE_REJECTIONS.fetch_add(1, Ordering::Relaxed);
        return Err(Error::InvalidHandle);
    };
    if slot.owner != owner {
        return Err(Error::NotOwner);
    }
    let wake = if slot.state == State::Pending {
        slot.port.map(|port| crate::completion_port::publish(port, handle.to_raw(), slot.class as u32,
            crate::completion_queue::CANCELLED, -2, 0))
    } else { None };
    slot.release();
    drop(table);
    if let Some(wake) = wake { crate::completion_port::notify(wake); }
    CANCELLED.fetch_add(1, Ordering::Relaxed);
    RELEASED.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Deterministically reclaim every operation owned by a task that is exiting.
/// This is safe against concurrent producers because completion and teardown
/// serialize on `TABLE`; late producers see InvalidHandle after release.
pub fn release_owner(owner: TaskId) -> usize {
    let mut released = 0usize;
    let mut table = TABLE.lock();
    for (index, slot) in table.slots.iter_mut().enumerate() {
        if slot.state != State::Free && slot.owner == owner {
            if let Some(port) = slot.port {
                crate::completion_port::abandon(port, Handle { slot: index as u16, generation: slot.generation }.to_raw());
            }
            slot.release();
            released += 1;
        }
    }
    if released != 0 {
        OWNER_REAPED.fetch_add(released as u64, Ordering::Relaxed);
        RELEASED.fetch_add(released as u64, Ordering::Relaxed);
    }
    released
}

pub fn stats() -> Stats {
    Stats {
        allocated: ALLOCATED.load(Ordering::Acquire),
        completed: COMPLETED.load(Ordering::Acquire),
        waits: WAITS.load(Ordering::Acquire),
        signals: SIGNALS.load(Ordering::Acquire),
        released: RELEASED.load(Ordering::Acquire),
        cancelled: CANCELLED.load(Ordering::Acquire),
        owner_reaped: OWNER_REAPED.load(Ordering::Acquire),
        stale_rejections: STALE_REJECTIONS.load(Ordering::Acquire),
        active: TABLE.lock().active_count(),
    }
}

/// Allocation-free structural probe for the generic API.
///
/// All operation classes are exercised. Completion-before-wait, stable raw
/// handles, non-consuming peek, cancellation, and stale reuse are verified.
pub fn structural_self_test() -> bool {
    let before = stats();
    let classes = [
        AsyncClass::Block,
        AsyncClass::Network,
        AsyncClass::Device,
        AsyncClass::Service,
        AsyncClass::File,
    ];

    let mut first_stale = None;
    for (index, class) in classes.into_iter().enumerate() {
        let Ok(handle) = allocate_current(class) else {
            return false;
        };
        if Handle::from_raw(handle.to_raw()) != Some(handle) {
            return false;
        }
        if first_stale.is_none() {
            first_stale = Some(handle);
        }
        if class_current(handle) != Ok(class) || peek_current(handle) != Ok(None) {
            return false;
        }
        let completion = Completion::new(index as i32, 0x1020 + index as u64);
        if complete(handle, completion).is_err()
            || peek_current(handle) != Ok(Some(completion))
            || wait(handle) != Ok(completion)
        {
            return false;
        }
    }

    let Some(stale) = first_stale else {
        return false;
    };
    if wait(stale) != Err(Error::InvalidHandle) {
        return false;
    }

    let Ok(cancelled) = allocate_current(AsyncClass::Service) else {
        return false;
    };
    if cancel_current(cancelled).is_err() || peek_current(cancelled) != Err(Error::InvalidHandle) {
        return false;
    }

    let after = stats();
    after.allocated >= before.allocated + 6
        && after.completed >= before.completed + 5
        && after.released >= before.released + 5
        && after.cancelled > before.cancelled
        && after.stale_rejections >= before.stale_rejections + 2
        && after.active == before.active
}

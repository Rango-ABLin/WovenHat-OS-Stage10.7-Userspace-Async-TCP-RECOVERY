//! Per-process completion ports. Producers reserve capacity at association.
//! Lock order: async operation table -> one port. Never call the scheduler
//! under a port lock; return waiter IDs to the caller for notification instead.
use crate::{completion_queue::{Queue, Event, BATCH}, irq_lock::IrqMutex, task::{self, TaskId}};

const PORTS: usize = 32;
const PER_PROCESS: usize = 4;
const TAG: u64 = 0xc001 << 16;
type Waiters = [Option<TaskId>; crate::config::MAX_TASKS];

struct Port {
    generation: u32,
    owner: Option<u64>,
    queue: Queue,
    waiters: Waiters,
    consumer: Option<TaskId>,
}
impl Port {
    const fn new() -> Self {
        Self { generation: 0, owner: None, queue: Queue::new(), waiters: [None; crate::config::MAX_TASKS], consumer: None }
    }
    fn matches(&self, handle: u64) -> bool { self.owner.is_some() && self.generation == (handle >> 32) as u32 }
    fn clear(&mut self) {
        self.owner = None;
        self.queue = Queue::new();
        self.waiters.fill(None);
        self.consumer = None;
    }
}
static ALLOCATION: IrqMutex<()> = IrqMutex::with_rank((), 10);
static TABLE: [IrqMutex<Port>; PORTS] =
    [const { IrqMutex::with_rank(Port::new(), 20) }; PORTS];

fn index(handle: u64) -> Result<usize, ()> {
    let index = (handle & 0xffff) as usize;
    if handle & 0xffff_0000 != TAG || handle >> 32 == 0 || index >= PORTS { return Err(()); }
    Ok(index)
}

pub fn create(owner: u64) -> Result<u64, ()> {
    let _allocation = ALLOCATION.lock();
    if TABLE.iter().filter(|p| p.lock().owner == Some(owner)).count() >= PER_PROCESS { return Err(()); }
    for (index, cell) in TABLE.iter().enumerate() {
        let mut port = cell.lock();
        if port.owner.is_none() && port.generation < u32::MAX {
            port.generation += 1; // exhausted slots retire permanently
            port.owner = Some(owner);
            return Ok(((port.generation as u64) << 32) | TAG | index as u64);
        }
    }
    Err(())
}

pub fn close(owner: u64, handle: u64) -> Result<(), ()> {
    let wake = {
        let _allocation = ALLOCATION.lock();
        let mut port = TABLE[index(handle)?].lock();
        if !port.matches(handle) || port.owner != Some(owner) { return Err(()); }
        let wake = port.waiters;
        port.clear();
        wake
    };
    notify(wake);
    Ok(())
}

/// Process teardown runs under the scheduler lock. All its tasks are being
/// retired; never recursively signal the scheduler here.
pub fn release_owner(owner: u64) {
    let _allocation = ALLOCATION.lock();
    for cell in &TABLE {
        let mut port = cell.lock();
        if port.owner == Some(owner) { port.clear(); }
    }
}

pub fn reserve(owner: u64, port: u64, operation: u64, cookie: u64) -> Result<(), ()> {
    let mut slot = TABLE[index(port)?].lock();
    if !slot.matches(port) || slot.owner != Some(owner) || !slot.queue.reserve(operation, cookie) { return Err(()); }
    Ok(())
}

pub fn publish(port: u64, operation: u64, class: u32, flags: u32, status: i32, value: u64) -> Waiters {
    let Ok(index) = index(port) else { return [None; crate::config::MAX_TASKS]; };
    let mut slot = TABLE[index].lock();
    if !slot.matches(port) || !slot.queue.publish(operation, class, flags, status, value) { return [None; crate::config::MAX_TASKS]; }
    core::mem::replace(&mut slot.waiters, [None; crate::config::MAX_TASKS])
}

pub fn abandon(port: u64, operation: u64) {
    let Ok(index) = index(port) else { return; };
    let mut slot = TABLE[index].lock();
    if slot.matches(port) { slot.queue.remove(operation); }
}

pub fn notify(waiters: Waiters) {
    for waiter in waiters.into_iter().flatten() { task::signal_event(waiter); }
}

/// Claim a copyout batch or atomically register a waiter. Only the consumer
/// claim persists over user copying; no lock is held during paging operations.
pub fn begin(owner: u64, handle: u64, task: TaskId, capacity: usize, wait: bool) -> Result<([Event; BATCH], usize), ()> {
    if capacity == 0 || capacity > BATCH { return Err(()); }
    let mut port = TABLE[index(handle)?].lock();
    if !port.matches(handle) || port.owner != Some(owner) || port.consumer.is_some() { return Err(()); }
    let mut events = [Event::default(); BATCH];
    let count = port.queue.snapshot(&mut events[..capacity]);
    if count != 0 {
        port.consumer = Some(task);
    } else if wait && !port.waiters.contains(&Some(task)) {
        let Some(slot) = port.waiters.iter_mut().find(|s| s.is_none()) else { return Err(()); };
        *slot = Some(task);
    }
    Ok((events, count))
}

pub fn finish(owner: u64, handle: u64, task: TaskId, events: &[Event], copied: bool) -> Result<(), ()> {
    let mut port = TABLE[index(handle)?].lock();
    if !port.matches(handle) || port.owner != Some(owner) || port.consumer != Some(task) { return Err(()); }
    if copied { for event in events { port.queue.remove(event.operation); } }
    port.consumer = None;
    Ok(())
}

pub fn unwatch(handle: u64, task: TaskId) {
    let Ok(index) = index(handle) else { return; };
    let mut port = TABLE[index].lock();
    if port.matches(handle) {
        for waiter in &mut port.waiters { if *waiter == Some(task) { *waiter = None; } }
    }
}

#[cfg(feature = "stage10-8-test")]
pub fn active_count() -> usize {
    TABLE.iter().filter(|p| p.lock().owner.is_some()).count()
}

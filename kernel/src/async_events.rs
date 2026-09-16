//! Bounded process-owned timers and manual-reset events. Object locks are never
//! retained across allocation, completion, scheduler notification or user copy.
use crate::{async_op::{self, AsyncClass, Completion, Handle}, deadline::Timer,
    irq_lock::IrqMutex, task::{self, TaskId, TaskPriority}, timer};
const COUNT: usize = 32;
const QUOTA: usize = 8;
const TAG: u64 = 0xc002 << 16;
#[derive(Clone, Copy)]
enum Kind { Timer(Timer), Event(bool) }
#[derive(Clone, Copy)]
struct Object { generation: u32, owner: Option<u64>, kind: Kind, waiter: Option<Handle> }
impl Object {
    const EMPTY: Self = Self { generation: 0, owner: None, kind: Kind::Event(false), waiter: None };
    fn clear(&mut self) { self.owner = None; self.waiter = None; }
    fn ready(&mut self, now: u64) -> u64 {
        match &mut self.kind {
            Kind::Timer(timer) => { timer.advance(now); timer.take() },
            Kind::Event(set) => u64::from(*set),
        }
    }
}
static OBJECTS: IrqMutex<[Object; COUNT]> = IrqMutex::new([Object::EMPTY; COUNT]);
static WORKER: IrqMutex<Option<TaskId>> = IrqMutex::new(None);

fn lookup(objects: &mut [Object; COUNT], owner: u64, raw: u64) -> Result<&mut Object, ()> {
    if raw & 0xffff_0000 != TAG || raw >> 32 == 0 { return Err(()); }
    let slot = objects.get_mut((raw & 0xffff) as usize).ok_or(())?;
    if slot.owner != Some(owner) || slot.generation != (raw >> 32) as u32 { return Err(()); }
    Ok(slot)
}
fn notify_worker() {
    let worker = *WORKER.lock();
    if let Some(worker) = worker { task::signal_event(worker); }
}
pub fn start_worker() -> bool {
    if WORKER.lock().is_some() { return true; }
    match task::spawn_with_priority("async-events", worker, TaskPriority::NORMAL) {
        Ok(id) => { *WORKER.lock() = Some(id); true }, Err(_) => false,
    }
}
fn create(owner: u64, kind: Kind) -> Result<u64, ()> {
    let raw = {
        let mut objects = OBJECTS.lock();
        if objects.iter().filter(|o| o.owner == Some(owner)).count() >= QUOTA { return Err(()); }
        let (index, object) = objects.iter_mut().enumerate().find(|(_, o)| o.owner.is_none() && o.generation < u32::MAX).ok_or(())?;
        object.generation += 1;
        object.owner = Some(owner); object.kind = kind; object.waiter = None;
        ((object.generation as u64) << 32) | TAG | index as u64
    };
    notify_worker();
    Ok(raw)
}
pub fn create_timer(owner: u64, deadline: u64, period: u64) -> Result<u64, ()> {
    if WORKER.lock().is_none() { return Err(()); }
    create(owner, Kind::Timer(Timer::new(deadline, period).ok_or(())?))
}
pub fn create_event(owner: u64, set: bool) -> Result<u64, ()> { create(owner, Kind::Event(set)) }

pub fn wait_current(raw: u64, is_timer: bool) -> Result<Handle, ()> {
    let owner = task::current_process_id();
    let class = if is_timer { AsyncClass::Timer } else { AsyncClass::Event };
    let handle = async_op::allocate_current(class).map_err(|_| ())?;
    let result = (|| {
        let mut objects = OBJECTS.lock();
        let object = lookup(&mut objects, owner, raw)?;
        if matches!(object.kind, Kind::Timer(_)) != is_timer || object.waiter.is_some() { return Err(()); }
        let count = object.ready(timer::ticks());
        if count != 0 { return Ok(Some(count)); }
        if matches!(object.kind, Kind::Timer(timer) if timer.exhausted()) { return Err(()); }
        object.waiter = Some(handle);
        Ok(None)
    })();
    match result {
        Ok(Some(count)) => { let _ = async_op::complete(handle, Completion::new(0, count)); },
        Ok(None) => notify_worker(),
        Err(()) => { let _ = async_op::release_current(handle); return Err(()); },
    }
    Ok(handle)
}
pub fn set_event(owner: u64, raw: u64, set: bool) -> Result<(), ()> {
    let waiter = {
        let mut objects = OBJECTS.lock();
        let object = lookup(&mut objects, owner, raw)?;
        let Kind::Event(state) = &mut object.kind else { return Err(()); };
        *state = set;
        if set { object.waiter.take() } else { None }
    };
    if let Some(waiter) = waiter { let _ = async_op::complete(waiter, Completion::new(0, 1)); }
    Ok(())
}
pub fn close(owner: u64, raw: u64, is_timer: bool) -> Result<(), ()> {
    let waiter = {
        let mut objects = OBJECTS.lock();
        let object = lookup(&mut objects, owner, raw)?;
        if matches!(object.kind, Kind::Timer(_)) != is_timer { return Err(()); }
        let waiter = object.waiter;
        object.clear();
        waiter
    };
    // Current processes have one thread. Stage 11 must extend close to cancel
    // sibling-thread waits without weakening generic operation ownership.
    if let Some(waiter) = waiter { let _ = async_op::cancel_current(waiter); }
    notify_worker();
    Ok(())
}
pub fn cancel(handle: Handle) {
    for object in OBJECTS.lock().iter_mut() {
        if object.waiter == Some(handle) { object.waiter = None; }
    }
}
/// Called under the scheduler lock, before generic operation owner cleanup.
pub fn release_owner(owner: u64) {
    for object in OBJECTS.lock().iter_mut() { if object.owner == Some(owner) { object.clear(); } }
}
fn pump(now: u64) -> u64 {
    let mut completions = [None; COUNT];
    let mut deadline = u64::MAX;
    {
        let mut objects = OBJECTS.lock();
        for (index, object) in objects.iter_mut().enumerate().filter(|(_, o)| o.owner.is_some()) {
            if let Kind::Timer(timer) = &mut object.kind {
                timer.advance(now);
                if let Some(next) = timer.next { deadline = deadline.min(next); }
            }
            if object.waiter.is_some() {
                let count = object.ready(now);
                if count != 0 { completions[index] = Some((object.waiter.take().unwrap(), count)); }
            }
        }
    }
    for (handle, count) in completions.into_iter().flatten() {
        let _ = async_op::complete(handle, Completion::new(0, count));
    }
    deadline
}
fn worker() -> ! {
    loop { let deadline = pump(timer::ticks()); task::wait_for_event_until(deadline); }
}
#[cfg(feature = "stage10-9-test")]
pub fn active_count() -> usize { OBJECTS.lock().iter().filter(|o| o.owner.is_some()).count() }

#[cfg(feature = "stage10-9-test")]
pub fn structural_self_test() -> bool {
    let owner = task::current_process_id();
    let event = match create_event(owner, false) { Ok(raw) => raw, Err(()) => return false };
    if set_event(owner, event, true).is_err() { return false; }
    let event_handle = match wait_current(event, false) { Ok(handle) => handle, Err(()) => return false };
    let event_result = async_op::wait(event_handle).map(|completion| completion.value == 1).unwrap_or(false);
    if close(owner, event, false).is_err() || !event_result { return false; }
    let deadline = timer::ticks().saturating_add(2);
    let timer_raw = match create_timer(owner, deadline, 0) { Ok(raw) => raw, Err(()) => return false };
    let timer_handle = match wait_current(timer_raw, true) { Ok(handle) => handle, Err(()) => return false };
    let _ = pump(deadline);
    let timer_result = async_op::wait(timer_handle).map(|completion| completion.value == 1).unwrap_or(false);
    close(owner, timer_raw, true).is_ok() && timer_result
}

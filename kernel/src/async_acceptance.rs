//! QEMU-only probes use ordinary operation/port/scheduler code.
use core::sync::atomic::{AtomicU64, Ordering};
use crate::{async_op, completion_port, completion_queue, smp, task, timer};
static HANDLES: [AtomicU64; smp::MAX_CPUS] = [const { AtomicU64::new(0) }; smp::MAX_CPUS];
static DONE: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);
static WAIT_HANDLE: AtomicU64 = AtomicU64::new(0);
static WAIT_RESULT: AtomicU64 = AtomicU64::new(0);

fn waiting_consumer() -> ! {
    let ok = (|| {
        let owner = task::current_process_id();
        let me = task::current_task_id();
        let port = completion_port::create(owner).ok()?;
        let operation = async_op::allocate_current(async_op::AsyncClass::Device).ok()?;
        async_op::associate_current(operation, port, 123).ok()?;
        let (_, count) = completion_port::begin(owner, port, me, 1, true).ok()?;
        if count != 0 { return None; }
        // Publish only after registering. The producer may win the subsequent
        // sleep race; either a latched wake or a blocked-task wake must work.
        WAIT_HANDLE.store(operation.to_raw(), Ordering::Release);
        let deadline = timer::ticks() + 100;
        loop {
            task::wait_for_event_until(deadline);
            completion_port::unwatch(port, me);
            let (events, count) = completion_port::begin(owner, port, me, 1, true).ok()?;
            if count == 1 {
                if events[0].cookie != 123 || events[0].value != 456 { return None; }
                completion_port::finish(owner, port, me, &events[..count], true).ok()?;
                break;
            }
            if timer::ticks() >= deadline { return None; }
        }
        async_op::release_current(operation).ok()?;
        completion_port::close(owner, port).ok()?;
        Some(())
    })().is_some();
    WAIT_RESULT.store(if ok { 1 } else { 2 }, Ordering::Release);
    task::exit_current_task()
}

fn waking_producer() -> ! {
    let handle = async_op::Handle::from_raw(WAIT_HANDLE.load(Ordering::Acquire)).unwrap();
    if async_op::complete(handle, async_op::Completion::new(0, 456)).is_err() {
        FAILED.fetch_add(1, Ordering::Relaxed);
    }
    task::exit_current_task()
}

fn wake_probe() -> bool {
    // SAFETY: The consumer stays on CPU0; the producer uses only the IRQ-safe
    // operation/port tables and scheduler notification after releasing them.
    if unsafe { task::spawn_on(0, "port-waiter", waiting_consumer) }.is_err() { return false; }
    let deadline = timer::ticks() + 500;
    while WAIT_HANDLE.load(Ordering::Acquire) == 0 {
        if timer::ticks() >= deadline || WAIT_RESULT.load(Ordering::Acquire) != 0 { return false; }
        x86_64::instructions::hlt();
    }
    // SAFETY: Producer follows the restricted cross-CPU contract above.
    if unsafe { task::spawn_on(smp::online_count() - 1, "port-waker", waking_producer) }.is_err() { return false; }
    while WAIT_RESULT.load(Ordering::Acquire) == 0 {
        if timer::ticks() >= deadline { return false; }
        x86_64::instructions::hlt();
    }
    WAIT_RESULT.load(Ordering::Acquire) == 1 && FAILED.load(Ordering::Relaxed) == 0
}

fn producer() -> ! {
    let cpu = smp::cpu_index();
    let raw = HANDLES[cpu].load(Ordering::Acquire);
    let ok = async_op::Handle::from_raw(raw).is_some_and(|h|
        async_op::complete(h, async_op::Completion::new(0, cpu as u64)).is_ok());
    if !ok { FAILED.fetch_add(1, Ordering::Relaxed); }
    DONE.fetch_add(1, Ordering::Release);
    task::exit_current_task()
}

pub fn ports_smp() -> bool {
    let owner = task::current_process_id();
    let task = task::current_task_id();
    let Ok(port) = completion_port::create(owner) else { return false; };
    for (cpu, target) in HANDLES.iter().enumerate().take(smp::online_count()) {
        let Ok(handle) = async_op::allocate_current(async_op::AsyncClass::Device) else { return false; };
        if async_op::associate_current(handle, port, cpu as u64).is_err() { return false; }
        target.store(handle.to_raw(), Ordering::Release);
        // SAFETY: Complete a preallocated operation through IRQ-safe tables,
        // signal after unlocking, then exit. No allocator/paging/legacy I/O.
        if unsafe { task::spawn_on(cpu, "port-producer", producer) }.is_err() { return false; }
    }
    let start = timer::ticks();
    while DONE.load(Ordering::Acquire) < smp::online_count() as u64 {
        if timer::ticks().saturating_sub(start) > 500 { return false; }
        x86_64::instructions::hlt();
    }
    let Ok((events, count)) = completion_port::begin(owner, port, task, completion_queue::BATCH, false) else { return false; };
    if count != smp::online_count() || FAILED.load(Ordering::Relaxed) != 0 { return false; }
    let mut mask = 0u64;
    for event in &events[..count] {
        if event.value >= smp::online_count() as u64 || event.cookie != event.value { return false; }
        mask |= 1 << event.value;
        let Some(handle) = async_op::Handle::from_raw(event.operation) else { return false; };
        if async_op::release_current(handle).is_err() { return false; }
    }
    completion_port::finish(owner, port, task, &events[..count], true).is_ok()
        && completion_port::close(owner, port).is_ok()
        && mask == smp::online_mask() as u64
        && wake_probe()
}

//! Execute the production worker with deterministic host socket/scheduler doubles.
//! The wait boundary unwinds back to the test instead of parking a host thread.
#![allow(dead_code)]
extern crate self as spin;
extern crate self as smoltcp;

pub struct Mutex<T>(std::sync::Mutex<T>);
mod irq_lock { pub use crate::Mutex as IrqMutex; }
impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
    pub const fn with_rank(value: T, _rank: u8) -> Self { Self::new(value) }
    pub fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
}
pub mod wire {
    #[derive(Clone, Copy)]
    pub struct IpEndpoint;
}
mod config {
    pub const MAX_ASYNC_NETWORK_REQUESTS: usize = 4;
    pub const MAX_IO_SIZE: usize = 16;
}
mod timer {
    pub fn ticks() -> u64 { 0 }
}
mod task {
    use std::sync::atomic::{AtomicU64, Ordering};
    pub type TaskId = u64;
    pub struct TaskPriority;
    impl TaskPriority { pub const NORMAL: Self = Self; }
    pub static ENTRY: crate::Mutex<Option<fn() -> !>> = crate::Mutex::new(None);
    pub fn spawn_with_priority(_: &str, entry: fn() -> !, _: TaskPriority) -> Result<TaskId, ()> {
        *ENTRY.lock() = Some(entry);
        Ok(99)
    }
    pub fn current_process_id() -> u64 { 1 }
    pub fn current_task_id_if_running() -> Option<TaskId> { Some(1) }
    pub static SIGNALS: AtomicU64 = AtomicU64::new(0);
    pub fn signal_event(_: TaskId) -> Result<(), ()> {
        SIGNALS.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    #[derive(Debug)]
    pub struct Parked;
    pub fn wait_for_event() { std::panic::panic_any(Parked); }
    pub fn wait_for_event_until(_: u64) { std::panic::panic_any(Parked); }
}
mod async_op {
    use std::sync::atomic::{AtomicU64, Ordering};
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Handle(u64);
    pub enum AsyncClass { Network }
    pub struct Completion;
    impl Completion { pub fn new(_: i32, _: u64) -> Self { Self } }
    static NEXT: AtomicU64 = AtomicU64::new(1);
    pub static DONE: crate::Mutex<Vec<Handle>> = crate::Mutex::new(Vec::new());
    pub fn allocate_current(_: AsyncClass) -> Result<Handle, ()> {
        Ok(Handle(NEXT.fetch_add(1, Ordering::Relaxed)))
    }
    pub fn release_current(_: Handle) -> Result<(), ()> { Ok(()) }
    pub fn complete(handle: Handle, _: Completion) -> Result<(), ()> {
        DONE.lock().push(handle);
        Ok(())
    }
}
mod network {
    use crate::wire::IpEndpoint;
    #[derive(Clone, Copy)]
    pub struct SocketToken(u64);
    #[derive(Clone, Copy, Debug)]
    pub enum SocketError { Address, WouldBlock, BufferFull }
    pub static ATTEMPTS: crate::Mutex<Vec<u64>> = crate::Mutex::new(Vec::new());
    pub static UNPINNED: crate::Mutex<Vec<u64>> = crate::Mutex::new(Vec::new());
    pub fn pin_socket(_: u64, descriptor: u64) -> Result<SocketToken, SocketError> {
        Ok(SocketToken(descriptor))
    }
    pub fn unpin_socket(token: SocketToken) { UNPINNED.lock().push(token.0); }
    pub fn socket_connect_pinned(_: SocketToken, _: IpEndpoint) -> Result<(), SocketError> {
        Err(SocketError::WouldBlock)
    }
    pub fn socket_send_pinned(token: SocketToken, data: &[u8]) -> Result<usize, SocketError> {
        let mut attempts = ATTEMPTS.lock();
        assert!(attempts.len() < 16, "worker busy loops instead of parking");
        attempts.push(token.0);
        if token.0 == 4 { crate::async_network::network_progress(); }
        if token.0 == 5 { assert_eq!(crate::async_network::release_owner(1), 1); }
        if token.0 == 2 { Ok(data.len()) } else { Err(SocketError::WouldBlock) }
    }
    pub fn socket_recv_pinned(_: SocketToken, _: &mut [u8]) -> Result<(usize, Option<IpEndpoint>), SocketError> {
        Err(SocketError::WouldBlock)
    }
}
#[path = "../kernel/src/async_network.rs"]
mod async_network;

fn run_until_parked() {
    let worker = task::ENTRY.lock().unwrap();
    let stopped = std::panic::catch_unwind(worker).unwrap_err();
    assert!(stopped.is::<task::Parked>());
}

#[test]
fn blocked_socket_does_not_starve_ready_work_and_worker_parks() {
    assert!(async_network::start_worker());
    let blocked = async_network::submit_send(1, b"blocked").unwrap();
    let ready = async_network::submit_send(2, b"ready").unwrap();
    let blocked_later = async_network::submit_send(3, b"blocked").unwrap();
    run_until_parked();
    assert_eq!(*async_op::DONE.lock(), vec![ready], "ready socket was starved");
    assert_eq!(*network::ATTEMPTS.lock(), vec![1, 2, 3]);
    assert!(async_network::peek_result(blocked).is_none());
    assert_eq!(async_network::peek_result(ready).unwrap().result.unwrap(), 5);
    assert!(async_network::consume(ready));
    assert!(async_network::cancel(blocked));
    assert!(async_network::cancel(blocked_later));
    let mut unpinned = network::UNPINNED.lock().clone();
    unpinned.sort_unstable();
    assert_eq!(unpinned, vec![1, 2, 3]);
    network::ATTEMPTS.lock().clear();
    run_until_parked();
    assert!(network::ATTEMPTS.lock().is_empty());

    // Readiness delivered while the only request is InProgress must wake it.
    use std::sync::atomic::Ordering;
    let in_progress = async_network::submit_send(4, b"pending").unwrap();
    task::SIGNALS.store(0, Ordering::Relaxed);
    run_until_parked();
    assert_eq!(task::SIGNALS.load(Ordering::Relaxed), 1);
    assert!(async_network::cancel(in_progress));

    // Owner teardown races a worker that has already copied its request.
    // Retry must drop that abandoned slot and release its pin exactly once.
    let abandoned = async_network::submit_send(5, b"abandoned").unwrap();
    run_until_parked();
    assert!(async_network::peek_result(abandoned).is_none());
    assert_eq!(network::UNPINNED.lock().iter().filter(|&&id| id == 5).count(), 1);
    assert_eq!(async_network::release_owner(1), 0);
}

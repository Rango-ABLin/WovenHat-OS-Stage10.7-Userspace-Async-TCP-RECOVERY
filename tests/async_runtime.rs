//! Real operation/port modules; only scheduler and IRQ locking are host doubles.
#![allow(dead_code)]
mod config {
    pub const MAX_ASYNC_OPERATIONS: usize = 32;
    pub const MAX_TASKS: usize = 32;
}
mod irq_lock {
    pub struct IrqMutex<T>(std::sync::Mutex<T>);
    impl<T> IrqMutex<T> {
        pub const fn new(value: T) -> Self {
            Self(std::sync::Mutex::new(value))
        }
        pub const fn with_rank(value: T, _rank: u8) -> Self {
            Self::new(value)
        }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
            self.0.lock().unwrap()
        }
    }
}
mod task {
    use std::cell::Cell;
    pub static SIGNALS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct TaskId(u64);
    impl TaskId {
        pub const fn from_u64(id: u64) -> Self {
            Self(id)
        }
        pub const fn as_u64(self) -> u64 {
            self.0
        }
    }
    thread_local! { static ID: Cell<u64> = const { Cell::new(1) }; static PID: Cell<u64> = const { Cell::new(1) }; }
    pub fn set(task: u64, process: u64) {
        ID.with(|id| id.set(task));
        PID.with(|id| id.set(process));
    }
    pub fn current_task_id_if_running() -> Option<TaskId> {
        Some(ID.with(|id| TaskId(id.get())))
    }
    pub fn current_process_id() -> u64 {
        PID.with(Cell::get)
    }
    pub fn signal_event(_: TaskId) -> bool {
        SIGNALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        true
    }
    pub fn wait_for_event() {
        panic!("unexpected blocking wait in host test");
    }
}
#[path = "../kernel/src/async_op.rs"]
mod async_op;
#[path = "../kernel/src/completion_port.rs"]
mod completion_port;
#[path = "../kernel/src/completion_queue.rs"]
mod completion_queue;

#[test]
fn ownership_retry_cancel_overflow_teardown_and_parallel_producers() {
    use async_op::{AsyncClass, Completion};
    let task = task::TaskId::from_u64(1);
    let port = completion_port::create(1).unwrap();
    assert_eq!(completion_port::begin(1, port, task, 1, true).unwrap().1, 0);
    let op = async_op::allocate_current(AsyncClass::Service).unwrap();
    async_op::complete(op, Completion::new(0, 42)).unwrap();
    async_op::associate_current(op, port, 99).unwrap(); // completion before association
    assert_eq!(task::SIGNALS.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!(async_op::associate_current(op, port, 0).is_err());
    assert!(completion_port::begin(2, port, task, 8, false).is_err());
    let (events, n) = completion_port::begin(1, port, task, 8, false).unwrap();
    assert_eq!(n, 1);
    assert_eq!((events[0].value, events[0].cookie), (42, 99));
    assert!(completion_port::begin(1, port, task, 8, false).is_err());
    completion_port::finish(1, port, task, &events[..n], false).unwrap();
    let (retry, n) = completion_port::begin(1, port, task, 8, false).unwrap();
    assert_eq!(retry, events);
    completion_port::finish(1, port, task, &retry[..n], true).unwrap();
    assert_eq!(async_op::peek_current(op), Ok(Some(Completion::new(0, 42))));
    async_op::release_current(op).unwrap();
    let cancelled = async_op::allocate_current(AsyncClass::Service).unwrap();
    async_op::associate_current(cancelled, port, 77).unwrap();
    async_op::cancel_current(cancelled).unwrap();
    assert!(async_op::complete(cancelled, Completion::OK).is_err());
    let (events, n) = completion_port::begin(1, port, task, 8, false).unwrap();
    assert_eq!(n, 1);
    assert_eq!((events[0].flags, events[0].status), (1, -2));
    completion_port::finish(1, port, task, &events[..n], true).unwrap();

    let workers: Vec<_> = (0..4)
        .map(|cpu| {
            std::thread::spawn(move || {
                task::set(10 + cpu, 1);
                for n in 0..8 {
                    let op = async_op::allocate_current(AsyncClass::Device).unwrap();
                    async_op::associate_current(op, port, cpu * 8 + n).unwrap();
                    async_op::complete(op, Completion::new(0, cpu * 8 + n)).unwrap();
                    async_op::release_current(op).unwrap();
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let extra = async_op::allocate_current(AsyncClass::Service).unwrap();
    assert!(async_op::associate_current(extra, port, 0).is_err());
    async_op::cancel_current(extra).unwrap();
    let mut seen = 0u32;
    for _ in 0..4 {
        let (events, n) = completion_port::begin(1, port, task, 8, false).unwrap();
        assert_eq!(n, 8);
        for event in &events[..n] {
            assert_eq!(seen & (1 << event.value), 0);
            seen |= 1 << event.value;
        }
        completion_port::finish(1, port, task, &events[..n], true).unwrap();
    }
    assert_eq!(seen, u32::MAX);
    let abandoned = async_op::allocate_current(AsyncClass::Service).unwrap();
    async_op::associate_current(abandoned, port, 0).unwrap();
    assert_eq!(async_op::release_owner(task), 1);
    completion_port::release_owner(1);
    assert!(completion_port::begin(1, port, task, 1, false).is_err());
    let fresh = completion_port::create(1).unwrap();
    assert_ne!(port, fresh);
    assert!(completion_port::close(1, port).is_err());
    completion_port::close(1, fresh).unwrap();
    let ports: Vec<_> = (0..4)
        .map(|_| completion_port::create(1).unwrap())
        .collect();
    assert!(completion_port::create(1).is_err());
    assert!(completion_port::close(2, ports[0]).is_err());
    for port in ports {
        completion_port::close(1, port).unwrap();
    }
    assert_eq!(async_op::stats().active, 0);
}

//! Bounded structured lifecycle notifications.
use spin::Mutex;

#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Kind { Terminate = 1, Suspend = 2, Resume = 3, ChildExit = 4, Exception = 5, User = 6 }
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Notification { pub kind: Kind, pub source: u64, pub payload: u64 }
const EMPTY: Option<Notification> = None;
const CAPACITY: usize = 64;
struct Queue { entries: [Option<Notification>; CAPACITY], head: usize, tail: usize, len: usize }
impl Queue { const fn new() -> Self { Self { entries: [EMPTY; CAPACITY], head: 0, tail: 0, len: 0 } } }
static QUEUE: Mutex<Queue> = Mutex::new(Queue::new());

pub fn publish(note: Notification) -> bool {
    let mut q = QUEUE.lock();
    if q.len == CAPACITY { return false }
    let tail = q.tail;
    q.entries[tail] = Some(note); q.tail = (tail + 1) % CAPACITY; q.len += 1; true
}
#[allow(dead_code)]
pub fn receive() -> Option<Notification> {
    let mut q = QUEUE.lock();
    let head = q.head;
    let note = q.entries[head].take()?; q.head = (head + 1) % CAPACITY; q.len -= 1; Some(note)
}
#[cfg(feature = "stage11-3-test")]
pub fn structural_self_test() -> bool {
    while receive().is_some() {}
    let note = Notification { kind: Kind::ChildExit, source: 7, payload: 23 };
    publish(note) && receive() == Some(note) && receive().is_none()
}

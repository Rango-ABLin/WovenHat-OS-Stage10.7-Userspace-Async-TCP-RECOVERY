//! Bounded structured lifecycle notifications.
use spin::Mutex;

#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Kind { Terminate = 1, Suspend = 2, Resume = 3, ChildExit = 4, Exception = 5, User = 6 }
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Notification { pub recipient: u64, pub kind: Kind, pub source: u64, pub payload: u64 }
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
pub fn receive_for(recipient: u64) -> Option<Notification> {
    let mut q = QUEUE.lock();
    let mut offset = 0;
    while offset < q.len {
        let index = (q.head + offset) % CAPACITY;
        if q.entries[index].is_some_and(|n| n.recipient == recipient) {
            let note = q.entries[index].take();
            let mut i = offset;
            while i + 1 < q.len {
                let from = (q.head + i + 1) % CAPACITY;
                let to = (q.head + i) % CAPACITY;
                q.entries[to] = q.entries[from].take(); i += 1;
            }
            q.tail = (q.tail + CAPACITY - 1) % CAPACITY; let tail = q.tail; q.entries[tail] = None; q.len -= 1;
            return note;
        }
        offset += 1;
    }
    None
}
#[cfg(feature = "stage11-3-test")]
pub fn structural_self_test() -> bool {
    while receive().is_some() {}
    let note = Notification { recipient: 9, kind: Kind::ChildExit, source: 7, payload: 23 };
    publish(note) && receive_for(8).is_none() && receive_for(9) == Some(note) && receive().is_none()
}

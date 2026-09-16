//! Stage 10.6 asynchronous socket I/O for ordinary Ring-3 applications.
//!
//! Requests pin a generation-tagged process socket, copy send payloads into
//! kernel memory, and retain no Ring-3 pointers. A close during an in-flight
//! operation becomes a deferred close until the async reference is released.
//! The worker sleeps on scheduler events when smoltcp reports WouldBlock;
//! `network::poll()` signals progress so readiness retries are event-driven.

use core::sync::atomic::{AtomicU64, Ordering};
use crate::irq_lock::IrqMutex as Mutex;
use smoltcp::wire::IpEndpoint;

use crate::{
    async_op::{self, AsyncClass, Completion},
    config::{MAX_ASYNC_NETWORK_REQUESTS, MAX_IO_SIZE},
    network::{self, SocketError, SocketToken},
    task::{self, TaskId},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Operation { Connect, Send, Recv }
#[derive(Clone, Copy, PartialEq, Eq)]
enum State { Pending, InProgress, Complete }

#[derive(Clone, Copy)]
struct Request {
    id: u64,
    owner: TaskId,
    operation: Operation,
    socket: SocketToken,
    length: usize,
    endpoint: Option<IpEndpoint>,
    data: [u8; MAX_IO_SIZE],
    result: Result<usize, SocketError>,
    state: State,
    completion: Option<async_op::Handle>,
}

#[derive(Clone, Copy)]
struct Work { slot: usize, request: Request }

#[derive(Clone, Copy)]
struct Queue {
    entries: [Option<Request>; MAX_ASYNC_NETWORK_REQUESTS],
    next_id: u64,
}

impl Queue {
    const fn new() -> Self {
        Self { entries: [const { None }; MAX_ASYNC_NETWORK_REQUESTS], next_id: 1 }
    }
    fn push(&mut self, mut request: Request) -> bool {
        let Some(slot) = self.entries.iter_mut().find(|entry| entry.is_none()) else { return false; };
        request.id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        *slot = Some(request);
        true
    }
    fn take_pending(&mut self, next_slot: &mut usize) -> Option<Work> {
        for (slot, entry) in self.entries.iter_mut().enumerate().skip(*next_slot) {
            let Some(request) = entry.as_mut() else { continue; };
            if request.state == State::Pending {
                *next_slot = slot + 1;
                request.state = State::InProgress;
                return Some(Work { slot, request: *request });
            }
        }
        None
    }
    fn retry(&mut self, slot: usize, id: u64) {
        let Some(Some(request)) = self.entries.get_mut(slot) else { return; };
        if request.id == id && request.state == State::InProgress && request.completion.is_some() {
            request.state = State::Pending;
        } else if request.id == id && request.state == State::InProgress && request.completion.is_none() {
            let socket = request.socket;
            self.entries[slot] = None;
            network::unpin_socket(socket);
        }
    }
    fn finish(&mut self, slot: usize, id: u64, result: Result<usize, SocketError>, data: [u8; MAX_IO_SIZE]) -> Option<async_op::Handle> {
        let (completion, socket) = {
            let Some(Some(request)) = self.entries.get_mut(slot) else { return None; };
            if request.id != id || request.state != State::InProgress { return None; }
            request.result = result;
            request.data = data;
            if request.completion.is_some() { request.state = State::Complete; }
            (request.completion, request.socket)
        };
        if completion.is_none() {
            self.entries[slot] = None;
            network::unpin_socket(socket);
        }
        completion
    }
    fn find_complete(&self, handle: async_op::Handle) -> Option<Request> {
        self.entries.iter().flatten().copied().find(|r| r.completion == Some(handle) && r.state == State::Complete)
    }
    fn consume(&mut self, handle: async_op::Handle) -> bool {
        for entry in &mut self.entries {
            let Some(request) = entry.as_ref() else { continue; };
            if request.completion == Some(handle) && request.state == State::Complete {
                let socket = request.socket;
                *entry = None;
                network::unpin_socket(socket);
                return true;
            }
        }
        false
    }
    fn cancel(&mut self, handle: async_op::Handle) -> bool {
        for entry in &mut self.entries {
            let Some(request) = entry.as_mut() else { continue; };
            if request.completion != Some(handle) { continue; }
            if request.state == State::InProgress {
                request.completion = None;
            } else {
                let socket = request.socket;
                *entry = None;
                network::unpin_socket(socket);
            }
            return true;
        }
        false
    }
    fn release_owner(&mut self, owner: TaskId) -> usize {
        let mut count = 0;
        for entry in &mut self.entries {
            let Some(request) = entry.as_mut() else { continue; };
            if request.owner != owner { continue; }
            count += 1;
            if request.state == State::InProgress {
                request.completion = None;
            } else {
                let socket = request.socket;
                *entry = None;
                network::unpin_socket(socket);
            }
        }
        count
    }
    fn has_work(&self) -> bool {
        self.entries.iter().flatten().any(|r| r.state != State::Complete)
    }
    #[cfg(any(feature = "stage10-6-test", feature = "stage10-7-test"))]
    fn active(&self) -> usize { self.entries.iter().filter(|e| e.is_some()).count() }
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue::new());
static WORKER: Mutex<Option<TaskId>> = Mutex::new(None);
static SUBMITTED: AtomicU64 = AtomicU64::new(0);
static COMPLETED: AtomicU64 = AtomicU64::new(0);
static CANCELLED: AtomicU64 = AtomicU64::new(0);
static OWNER_REAPED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub struct UserResult {
    pub operation: Operation,
    pub result: Result<usize, SocketError>,
    pub length: usize,
    pub data: [u8; MAX_IO_SIZE],
}

#[cfg(any(feature = "stage10-6-test", feature = "stage10-7-test"))]
#[derive(Clone, Copy)]
pub struct Stats { pub submitted: u64, pub completed: u64, pub cancelled: u64, pub owner_reaped: u64, pub active: usize }

pub fn start_worker() -> bool {
    if WORKER.lock().is_some() { return true; }
    match task::spawn_io_service("async-net", worker_task) {
        Ok(id) => { *WORKER.lock() = Some(id); true }
        Err(_) => false,
    }
}

fn submit(operation: Operation, descriptor: u64, endpoint: Option<IpEndpoint>, data: [u8; MAX_IO_SIZE], length: usize) -> Result<async_op::Handle, ()> {
    if length > MAX_IO_SIZE || WORKER.lock().is_none() { return Err(()); }
    let process = task::current_process_id();
    let socket = network::pin_socket(process, descriptor).map_err(|_| ())?;
    let owner = match task::current_task_id_if_running() {
        Some(owner) => owner,
        None => { network::unpin_socket(socket); return Err(()); }
    };
    let completion = match async_op::allocate_current(AsyncClass::Network) {
        Ok(handle) => handle,
        Err(_) => { network::unpin_socket(socket); return Err(()); }
    };
    let request = Request { id: 0, owner, operation, socket, length, endpoint, data, result: Ok(0), state: State::Pending, completion: Some(completion) };
    if !QUEUE.lock().push(request) {
        network::unpin_socket(socket);
        let _ = async_op::release_current(completion);
        return Err(());
    }
    SUBMITTED.fetch_add(1, Ordering::Relaxed);
    signal_worker();
    Ok(completion)
}

pub fn submit_send(descriptor: u64, input: &[u8]) -> Result<async_op::Handle, ()> {
    if input.len() > MAX_IO_SIZE { return Err(()); }
    let mut data = [0u8; MAX_IO_SIZE];
    data[..input.len()].copy_from_slice(input);
    submit(Operation::Send, descriptor, None, data, input.len())
}

pub fn submit_recv(descriptor: u64, capacity: usize) -> Result<async_op::Handle, ()> {
    if capacity > MAX_IO_SIZE { return Err(()); }
    submit(Operation::Recv, descriptor, None, [0u8; MAX_IO_SIZE], capacity)
}

pub fn submit_connect(descriptor: u64, endpoint: IpEndpoint) -> Result<async_op::Handle, ()> {
    submit(Operation::Connect, descriptor, Some(endpoint), [0u8; MAX_IO_SIZE], 0)
}

pub fn peek_result(handle: async_op::Handle) -> Option<UserResult> {
    QUEUE.lock().find_complete(handle).map(|r| UserResult { operation: r.operation, result: r.result, length: r.length, data: r.data })
}
pub fn consume(handle: async_op::Handle) -> bool { QUEUE.lock().consume(handle) }
pub fn cancel(handle: async_op::Handle) -> bool {
    let ok = QUEUE.lock().cancel(handle);
    if ok { CANCELLED.fetch_add(1, Ordering::Relaxed); }
    ok
}
pub fn release_owner(owner: TaskId) -> usize {
    let count = QUEUE.lock().release_owner(owner);
    OWNER_REAPED.fetch_add(count as u64, Ordering::Relaxed);
    count
}

#[cfg(any(feature = "stage10-6-test", feature = "stage10-7-test"))]
pub fn stats() -> Stats {
    let queue = QUEUE.lock();
    Stats {
        submitted: SUBMITTED.load(Ordering::Acquire), completed: COMPLETED.load(Ordering::Acquire),
        cancelled: CANCELLED.load(Ordering::Acquire), owner_reaped: OWNER_REAPED.load(Ordering::Acquire), active: queue.active(),
    }
}

fn signal_worker() {
    if let Some(worker) = *WORKER.lock() { let _ = task::signal_event(worker); }
}

pub fn network_progress() {
    let worker = *WORKER.lock();
    // Observe Pending and InProgress in ONE critical section. Sampling a queue
    // predicate and then a separate atomic bridge can miss a transition: first
    // read Pending=false, then the worker retries and clears the bridge, then
    // read bridge=false. The request never leaves this queue while being tried.
    let active = QUEUE.lock().has_work();
    if let Some(worker) = worker.filter(|_| active) { let _ = task::signal_event(worker); }
}

fn worker_task() -> ! {
    loop {
        // Attempt each slot at most once per wake. A blocked connection must
        // not prevent unrelated sockets from progressing, and retrying blocked
        // slots within this pass would turn event-driven waiting into a spin.
        // Submissions to already visited slots latch a scheduler event.
        let mut next_slot = 0;
        while process_one(&mut next_slot) {}
        task::wait_for_event();
    }
}

fn finish_invalid(work: Work, data: [u8; MAX_IO_SIZE]) -> bool {
    let plain: Result<usize, SocketError> = Err(SocketError::Address);
    let handle = QUEUE.lock().finish(work.slot, work.request.id, plain, data);
    COMPLETED.fetch_add(1, Ordering::Relaxed);
    if let Some(handle) = handle {
        let _ = async_op::complete(handle, Completion::new(-1, 0));
    }
    true
}

fn process_one(next_slot: &mut usize) -> bool {
    let Some(work) = QUEUE.lock().take_pending(next_slot) else {
        return false;
    };
    let mut data = work.request.data;
    let result = match work.request.operation {
        Operation::Connect => {
            let Some(endpoint) = work.request.endpoint else {
                return finish_invalid(work, data);
            };
            network::socket_connect_pinned(work.request.socket, endpoint).map(|()| (0, None))
        }
        Operation::Send => network::socket_send_pinned(work.request.socket, &data[..work.request.length]).map(|n| (n, None)),
        Operation::Recv => network::socket_recv_pinned(work.request.socket, &mut data[..work.request.length]),
    };
    match result {
        Err(SocketError::WouldBlock) | Err(SocketError::BufferFull) => {
            QUEUE.lock().retry(work.slot, work.request.id);
            // Both sides of InProgress -> Pending are visible to the single
            // queue-state snapshot in network_progress(). The scheduler event
            // latch covers a signal arriving before wait_for_event().
            true
        }
        other => {
            let plain = other.map(|(n, _)| n);
            let handle = QUEUE.lock().finish(work.slot, work.request.id, plain, data);
            COMPLETED.fetch_add(1, Ordering::Relaxed);
            if let Some(handle) = handle {
                let status = if plain.is_ok() { 0 } else { -1 };
                let value = plain.unwrap_or(0) as u64;
                let _ = async_op::complete(handle, Completion::new(status, value));
            }
            true
        }
    }
}

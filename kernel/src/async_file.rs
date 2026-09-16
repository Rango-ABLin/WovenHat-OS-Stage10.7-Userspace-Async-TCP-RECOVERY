//! Stage 10.5 asynchronous VFS/file I/O for ordinary Ring-3 applications.
//!
//! Submission pins the exact refcounted VFS open-file description, copies all
//! write payloads into kernel memory, and stores no Ring-3 pointer. Reads and
//! writes are positional, so concurrent operations never mutate/share the fd
//! seek offset. Cancellation and owner teardown safely handle a worker that has
//! already copied an in-progress request: that request becomes detached from
//! its async handle and the worker releases the final pinned VFS reference.

use core::sync::atomic::{AtomicU64, Ordering};
use crate::irq_lock::IrqMutex as Mutex;

use crate::{
    async_op::{self, AsyncClass, Completion},
    config::{MAX_ASYNC_FILE_REQUESTS, MAX_IO_SIZE},
    task::{self, TaskId},
    vfs,
    wovenguard::FileScope,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Operation { Read, Write }
#[derive(Clone, Copy, PartialEq, Eq)]
enum State { Pending, InProgress, Complete }

#[derive(Clone, Copy)]
struct Request {
    id: u64,
    owner: TaskId,
    operation: Operation,
    file: vfs::OpenFileId,
    scope: FileScope,
    under_mnt: bool,
    offset: usize,
    length: usize,
    data: [u8; MAX_IO_SIZE],
    result: Result<usize, vfs::Error>,
    state: State,
    completion: Option<async_op::Handle>,
}

#[derive(Clone, Copy)]
struct Work { slot: usize, request: Request }

#[derive(Clone, Copy)]
struct Queue {
    entries: [Option<Request>; MAX_ASYNC_FILE_REQUESTS],
    next_id: u64,
}
impl Queue {
    const fn new() -> Self { Self { entries: [const { None }; MAX_ASYNC_FILE_REQUESTS], next_id: 1 } }
    fn push(&mut self, mut request: Request) -> bool {
        let Some(slot) = self.entries.iter_mut().find(|e| e.is_none()) else { return false; };
        request.id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        *slot = Some(request);
        true
    }
    fn take_pending(&mut self) -> Option<Work> {
        for (slot, entry) in self.entries.iter_mut().enumerate() {
            let Some(request) = entry.as_mut() else { continue; };
            if request.state == State::Pending {
                request.state = State::InProgress;
                return Some(Work { slot, request: *request });
            }
        }
        None
    }
    fn finish(&mut self, slot: usize, id: u64, result: Result<usize, vfs::Error>, data: [u8; MAX_IO_SIZE]) -> Option<async_op::Handle> {
        let (completion, file) = {
            let Some(Some(request)) = self.entries.get_mut(slot) else { return None; };
            if request.id != id || request.state != State::InProgress { return None; }
            request.result = result;
            request.data = data;
            if request.completion.is_some() {
                request.state = State::Complete;
            }
            (request.completion, request.file)
        };
        if completion.is_none() {
            self.entries[slot] = None;
            let _ = vfs::close_open_file(file);
        }
        completion
    }
    fn find_complete(&self, handle: async_op::Handle) -> Option<Request> {
        self.entries.iter().flatten().copied().find(|r| r.completion == Some(handle) && r.state == State::Complete)
    }
    fn consume(&mut self, handle: async_op::Handle) -> bool {
        for entry in &mut self.entries {
            let Some(request) = entry.as_ref() else {
                continue;
            };
            if request.completion == Some(handle) && request.state == State::Complete {
                let file = request.file;
                *entry = None;
                let _ = vfs::close_open_file(file);
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
                let file = request.file;
                *entry = None;
                let _ = vfs::close_open_file(file);
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
                let file = request.file;
                *entry = None;
                let _ = vfs::close_open_file(file);
            }
        }
        count
    }
    #[cfg(feature = "stage10-5-test")]
    fn active(&self) -> usize { self.entries.iter().filter(|e| e.is_some()).count() }
}

static QUEUE: Mutex<Queue> = Mutex::with_rank(Queue::new(), 10);
static WORKER: Mutex<Option<TaskId>> = Mutex::with_rank(None, 10);
static SUBMITTED: AtomicU64 = AtomicU64::new(0);
static COMPLETED: AtomicU64 = AtomicU64::new(0);
static CANCELLED: AtomicU64 = AtomicU64::new(0);
static OWNER_REAPED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub struct UserResult {
    pub operation: Operation,
    pub scope: FileScope,
    pub result: Result<usize, vfs::Error>,
    pub length: usize,
    pub data: [u8; MAX_IO_SIZE],
}
#[cfg(feature = "stage10-5-test")]
#[derive(Clone, Copy)]
pub struct Stats { pub submitted: u64, pub completed: u64, pub cancelled: u64, pub owner_reaped: u64, pub active: usize }

pub fn start_worker() -> bool {
    if WORKER.lock().is_some() { return true; }
    match task::spawn_io_service("async-file", worker_task) {
        Ok(id) => { *WORKER.lock() = Some(id); true }, Err(_) => false,
    }
}

fn submit(operation: Operation, descriptor: u64, offset: usize, data: [u8; MAX_IO_SIZE], length: usize) -> Result<async_op::Handle, ()> {
    if length > MAX_IO_SIZE || WORKER.lock().is_none() { return Err(()); }
    let write = operation == Operation::Write;
    let (file, scope, under_mnt) = task::pin_current_file_for_async(descriptor, write).map_err(|_| ())?;
    let owner = match task::current_task_id_if_running() {
        Some(owner) => owner,
        None => { let _ = vfs::close_open_file(file); return Err(()); }
    };
    let completion = match async_op::allocate_current(AsyncClass::File) {
        Ok(h) => h,
        Err(_) => { let _ = vfs::close_open_file(file); return Err(()); }
    };
    let request = Request { id: 0, owner, operation, file, scope, under_mnt, offset, length, data, result: Ok(0), state: State::Pending, completion: Some(completion) };
    if !QUEUE.lock().push(request) {
        let _ = vfs::close_open_file(file);
        let _ = async_op::release_current(completion);
        return Err(());
    }
    SUBMITTED.fetch_add(1, Ordering::Relaxed);
    if let Some(worker) = *WORKER.lock() { let _ = task::signal_event(worker); }
    Ok(completion)
}

pub fn submit_read(descriptor: u64, offset: usize, length: usize) -> Result<async_op::Handle, ()> {
    submit(Operation::Read, descriptor, offset, [0; MAX_IO_SIZE], length)
}
pub fn submit_write(descriptor: u64, offset: usize, input: &[u8]) -> Result<async_op::Handle, ()> {
    if input.len() > MAX_IO_SIZE { return Err(()); }
    let mut data = [0; MAX_IO_SIZE]; data[..input.len()].copy_from_slice(input);
    submit(Operation::Write, descriptor, offset, data, input.len())
}
pub fn peek_result(handle: async_op::Handle) -> Option<UserResult> {
    QUEUE.lock().find_complete(handle).map(|r| UserResult { operation: r.operation, scope: r.scope, result: r.result, length: r.length, data: r.data })
}
pub fn consume(handle: async_op::Handle) -> bool { QUEUE.lock().consume(handle) }
pub fn cancel(handle: async_op::Handle) -> bool { let ok=QUEUE.lock().cancel(handle); if ok { CANCELLED.fetch_add(1, Ordering::Relaxed); } ok }
pub fn release_owner(owner: TaskId) -> usize { let n=QUEUE.lock().release_owner(owner); OWNER_REAPED.fetch_add(n as u64, Ordering::Relaxed); n }
#[cfg(feature = "stage10-5-test")]
pub fn stats() -> Stats { let q=QUEUE.lock(); Stats { submitted: SUBMITTED.load(Ordering::Acquire), completed: COMPLETED.load(Ordering::Acquire), cancelled: CANCELLED.load(Ordering::Acquire), owner_reaped: OWNER_REAPED.load(Ordering::Acquire), active: q.active() } }

fn worker_task() -> ! { loop { while process_one() {} task::wait_for_event(); } }
fn process_one() -> bool {
    let Some(work)=QUEUE.lock().take_pending() else { return false; };
    let mut data=work.request.data;
    let result=match work.request.operation {
        Operation::Read => vfs::read_at(work.request.file, work.request.offset, &mut data[..work.request.length]),
        Operation::Write => vfs::write_at(work.request.file, work.request.offset, &data[..work.request.length]),
    };
    if result.is_ok() && work.request.operation == Operation::Write && work.request.under_mnt { crate::storage::mark_mnt_dirty(); }
    let handle=QUEUE.lock().finish(work.slot, work.request.id, result, data);
    COMPLETED.fetch_add(1, Ordering::Relaxed);
    if let Some(handle)=handle {
        let status=if result.is_ok(){0}else{-1};
        let value=result.unwrap_or(0) as u64;
        let _=async_op::complete(handle, Completion::new(status, value));
    }
    true
}

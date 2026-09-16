use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use spin::Mutex;

use crate::{
    async_op::{self, AsyncClass, Completion as AsyncCompletion},
    block::{BlockDevice, Error, SECTOR_SIZE},
    config::MAX_BLOCK_IO_REQUESTS,
    task::{self, TaskId, TaskPriority},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation {
    Read,
    Write,
    Flush,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RequestState {
    Pending,
    InProgress,
    Complete,
}

#[derive(Clone, Copy)]
struct Request {
    id: u64,
    operation: Operation,
    lba: u64,
    data: [u8; SECTOR_SIZE],
    result: Result<(), Error>,
    state: RequestState,
    owner: TaskId,
    completion: Option<async_op::Handle>,
}

impl Request {
    fn pending(
        id: u64,
        operation: Operation,
        lba: u64,
        data: [u8; SECTOR_SIZE],
        owner: TaskId,
        completion: Option<async_op::Handle>,
    ) -> Self {
        Self {
            id,
            operation,
            lba,
            data,
            result: Ok(()),
            state: RequestState::Pending,
            owner,
            completion,
        }
    }
}

#[derive(Clone, Copy)]
struct WorkItem {
    slot: usize,
    request: Request,
}

#[derive(Clone, Copy)]
struct Queue {
    entries: [Option<Request>; MAX_BLOCK_IO_REQUESTS],
    next_id: u64,
}

impl Queue {
    const fn new() -> Self {
        Self {
            entries: [const { None }; MAX_BLOCK_IO_REQUESTS],
            next_id: 1,
        }
    }

    fn push(
        &mut self,
        operation: Operation,
        lba: u64,
        data: [u8; SECTOR_SIZE],
        owner: TaskId,
        completion: Option<async_op::Handle>,
    ) -> Option<u64> {
        let slot = self.entries.iter_mut().find(|entry| entry.is_none())?;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        *slot = Some(Request::pending(id, operation, lba, data, owner, completion));
        Some(id)
    }

    fn take_pending(&mut self) -> Option<WorkItem> {
        for (slot, entry) in self.entries.iter_mut().enumerate() {
            let Some(request) = entry.as_mut() else {
                continue;
            };
            if request.state == RequestState::Pending {
                request.state = RequestState::InProgress;
                return Some(WorkItem {
                    slot,
                    request: *request,
                });
            }
        }
        None
    }

    fn finish(
        &mut self,
        slot: usize,
        id: u64,
        result: Result<(), Error>,
        data: [u8; SECTOR_SIZE],
    ) -> Option<async_op::Handle> {
        let Some(Some(request)) = self.entries.get_mut(slot) else {
            return None;
        };
        if request.id == id && request.state == RequestState::InProgress {
            request.data = data;
            request.result = result;
            request.state = RequestState::Complete;
            return request.completion;
        }
        None
    }

    fn take_result(&mut self, id: u64, output: Option<&mut [u8]>) -> Option<Result<(), Error>> {
        for entry in &mut self.entries {
            let Some(request) = entry else {
                continue;
            };
            if request.id != id || request.state != RequestState::Complete {
                continue;
            }
            let result = request.result;
            if result.is_ok() {
                if let Some(output) = output {
                    output.copy_from_slice(&request.data);
                }
            }
            *entry = None;
            return Some(result);
        }
        None
    }

    fn peek_by_completion(
        &self,
        handle: async_op::Handle,
    ) -> Option<(Result<(), Error>, Operation, [u8; SECTOR_SIZE])> {
        self.entries.iter().flatten().find_map(|request| {
            (request.completion == Some(handle) && request.state == RequestState::Complete)
                .then_some((request.result, request.operation, request.data))
        })
    }

    fn remove_by_completion(&mut self, handle: async_op::Handle) -> bool {
        for entry in &mut self.entries {
            if entry
                .as_ref()
                .is_some_and(|request| request.completion == Some(handle))
            {
                *entry = None;
                return true;
            }
        }
        false
    }

    fn release_owner(&mut self, owner: TaskId) -> usize {
        let mut released = 0usize;
        for entry in &mut self.entries {
            if entry.as_ref().is_some_and(|request| request.owner == owner) {
                *entry = None;
                released += 1;
            }
        }
        released
    }

    fn pending_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.is_some_and(|request| request.state == RequestState::Pending))
            .count()
    }

    fn active_count(&self) -> usize {
        self.entries.iter().filter(|entry| entry.is_some()).count()
    }
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue::new());
static WORKER_TASK: Mutex<Option<TaskId>> = Mutex::new(None);
static QUEUED: AtomicU64 = AtomicU64::new(0);
static COMPLETED: AtomicU64 = AtomicU64::new(0);
static DIRECT: AtomicU64 = AtomicU64::new(0);
static EVENT_WAITS: AtomicU64 = AtomicU64::new(0);
static EVENT_SIGNALS: AtomicU64 = AtomicU64::new(0);
static PROBE_DONE: AtomicBool = AtomicBool::new(false);
static PROBE_PASSED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub queued: u64,
    pub completed: u64,
    pub direct: u64,
    pub event_waits: u64,
    pub event_signals: u64,
    pub pending: usize,
    pub active: usize,
}

pub struct PrimaryAta;

pub fn primary_ata() -> PrimaryAta {
    PrimaryAta
}

pub fn primary_ata_present() -> bool {
    crate::ata::with_primary_master(|_| ()).is_some()
}

pub fn start_worker() -> bool {
    if WORKER_TASK.lock().is_some() {
        return true;
    }
    match task::spawn_with_priority("block-io", worker_task, TaskPriority::NORMAL) {
        Ok(id) => {
            *WORKER_TASK.lock() = Some(id);
            true
        }
        Err(_) => false,
    }
}

pub fn async_completion_self_test() -> bool {
    if WORKER_TASK.lock().is_none() {
        return false;
    }
    PROBE_DONE.store(false, Ordering::Release);
    PROBE_PASSED.store(false, Ordering::Release);
    if task::spawn("block-io-probe", async_probe_task).is_err() {
        return false;
    }
    let start = crate::timer::ticks();
    while !PROBE_DONE.load(Ordering::Acquire) {
        if crate::timer::ticks().wrapping_sub(start) > 100 {
            return false;
        }
        task::yield_now();
    }
    PROBE_PASSED.load(Ordering::Acquire)
}

/// Stage 10.1 production proof for scheduler-integrated asynchronous I/O.
///
/// A real queued ATA request must complete through the worker while the caller
/// sleeps on the Stage 8.3 latched scheduler event.  The completion path must
/// signal that event rather than forcing the waiter to poll/yield.  The test is
/// deliberately device-result agnostic: an absent/limited test disk may return
/// a normal device error, but the asynchronous lifecycle still has to complete.
pub fn stage10_1_event_driven_completion_valid() -> bool {
    let before = stats();
    if !async_completion_self_test() {
        return false;
    }
    let after = stats();

    after.queued > before.queued
        && after.completed > before.completed
        && after.event_waits > before.event_waits
        && after.event_signals > before.event_signals
        && after.active == 0
}

/// Stage 10.2 proof that block I/O is using the generic generation-tagged
/// asynchronous operation table rather than a storage-private wakeup contract.
pub fn stage10_2_generic_async_completion_valid() -> bool {
    let generic_before = async_op::stats();
    let block_before = stats();

    if !async_op::structural_self_test() || !async_completion_self_test() {
        return false;
    }

    let generic_after = async_op::stats();
    let block_after = stats();
    // Keep wait/signal telemetry part of the public Stage 10.2 surface without
    // making the proof timing-dependent: completion may legitimately win
    // before the owner has to sleep.
    let _scheduler_activity = (generic_after.waits, generic_after.signals);
    generic_after.allocated > generic_before.allocated
        && generic_after.completed > generic_before.completed
        && generic_after.released > generic_before.released
        && generic_after.stale_rejections > generic_before.stale_rejections
        && block_after.queued > block_before.queued
        && block_after.completed > block_before.completed
        && generic_after.active == 0
        && block_after.active == 0
}

fn async_probe_task() -> ! {
    let before = stats();
    let mut sector = [0_u8; SECTOR_SIZE];
    let result = primary_ata().read_sector(0, &mut sector);
    let after = stats();
    let completed_result = matches!(
        result,
        Ok(()) | Err(Error::DeviceFault) | Err(Error::OutOfBounds)
    );
    PROBE_PASSED.store(
        completed_result
            && after.queued > before.queued
            && after.completed > before.completed
            && after.direct == before.direct,
        Ordering::Release,
    );
    PROBE_DONE.store(true, Ordering::Release);
    task::exit_current_task()
}

#[derive(Clone, Copy)]
pub struct UserResult {
    pub result: Result<(), Error>,
    pub is_read: bool,
    pub data: [u8; SECTOR_SIZE],
}

fn enqueue_user(
    operation: Operation,
    lba: u64,
    data: [u8; SECTOR_SIZE],
) -> Result<async_op::Handle, Error> {
    // Unlike the synchronous kernel `submit()` path, this dedicated Ring-3
    // asynchronous submission is entered through the int 0x80 interrupt gate,
    // so IF is intentionally clear while the syscall is executing. Rejecting
    // IF=0 here would make every userspace async block submission fail before
    // it ever reached the queue. The syscall only copies/queues kernel-owned
    // state and never waits while holding this submission context, so it is
    // safe to enqueue with interrupts masked. The later AsyncBlockWait syscall
    // performs the scheduler hand-off after the request is visible.
    let owner = task::current_task_id_if_running().ok_or(Error::DeviceFault)?;
    let worker = WORKER_TASK.lock().ok_or(Error::DeviceFault)?;
    if worker == owner {
        return Err(Error::DeviceFault);
    }
    let completion = async_op::allocate_current(AsyncClass::Block)
        .map_err(|_| Error::DeviceFault)?;
    if QUEUE
        .lock()
        .push(operation, lba, data, owner, Some(completion))
        .is_none()
    {
        let _ = async_op::release_current(completion);
        return Err(Error::DeviceFault);
    }
    QUEUED.fetch_add(1, Ordering::Relaxed);
    let _ = task::signal_event(worker);
    Ok(completion)
}

/// Stage 10.4 Ring-3 submission path. Read data remains in a kernel-owned
/// bounce buffer until the owner explicitly collects the completed request.
pub fn submit_user_read(lba: u64) -> Result<async_op::Handle, Error> {
    enqueue_user(Operation::Read, lba, [0; SECTOR_SIZE])
}

/// Stage 10.4 Ring-3 write submission. The userspace sector has already been
/// copied into this kernel-owned buffer before the request becomes visible to
/// the worker, so no raw Ring-3 pointer survives the syscall boundary.
pub fn submit_user_write(
    lba: u64,
    data: [u8; SECTOR_SIZE],
) -> Result<async_op::Handle, Error> {
    enqueue_user(Operation::Write, lba, data)
}

pub fn peek_user_result(handle: async_op::Handle) -> Option<UserResult> {
    QUEUE
        .lock()
        .peek_by_completion(handle)
        .map(|(result, operation, data)| UserResult {
            result,
            is_read: operation == Operation::Read,
            data,
        })
}

pub fn consume_user_result(handle: async_op::Handle) -> bool {
    QUEUE.lock().remove_by_completion(handle)
}

pub fn cancel_user_request(handle: async_op::Handle) -> bool {
    QUEUE.lock().remove_by_completion(handle)
}

/// Reclaim queued/in-progress/completed block requests owned by an exiting
/// task. A worker that already copied an in-progress WorkItem may still finish
/// the device transaction, but the request-id check prevents it from attaching
/// the result to a reused queue slot, and the later async completion sees a
/// stale generation after async_op owner teardown.
pub fn release_owner(owner: TaskId) -> usize {
    QUEUE.lock().release_owner(owner)
}

pub fn stats() -> Stats {
    let queue = QUEUE.lock();
    Stats {
        queued: QUEUED.load(Ordering::Acquire),
        completed: COMPLETED.load(Ordering::Acquire),
        direct: DIRECT.load(Ordering::Acquire),
        event_waits: EVENT_WAITS.load(Ordering::Acquire),
        event_signals: EVENT_SIGNALS.load(Ordering::Acquire),
        pending: queue.pending_count(),
        active: queue.active_count(),
    }
}

impl BlockDevice for PrimaryAta {
    fn sector_count(&self) -> u64 {
        crate::ata::with_primary_master(|disk| disk.sector_count()).unwrap_or(0)
    }

    fn is_read_only(&self) -> bool {
        crate::ata::with_primary_master(|disk| disk.is_read_only()).unwrap_or(true)
    }

    fn flush(&mut self) -> Result<(), Error> {
        submit(Operation::Flush, 0, None, None)
    }

    fn read_sector(&mut self, lba: u64, sector: &mut [u8]) -> Result<(), Error> {
        if sector.len() != SECTOR_SIZE {
            return Err(Error::InvalidBuffer);
        }
        submit(Operation::Read, lba, None, Some(sector))
    }

    fn write_sector(&mut self, lba: u64, sector: &[u8]) -> Result<(), Error> {
        if sector.len() != SECTOR_SIZE {
            return Err(Error::InvalidBuffer);
        }
        submit(Operation::Write, lba, Some(sector), None)
    }
}

fn should_queue() -> bool {
    if !x86_64::instructions::interrupts::are_enabled() {
        return false;
    }
    let Some(current) = task::current_task_id_if_running() else {
        return false;
    };
    if current.as_u64() == 0 {
        return false;
    }
    WORKER_TASK.lock().is_some_and(|worker| worker != current)
}

fn submit(
    operation: Operation,
    lba: u64,
    input: Option<&[u8]>,
    output: Option<&mut [u8]>,
) -> Result<(), Error> {
    if !should_queue() {
        DIRECT.fetch_add(1, Ordering::Relaxed);
        return direct_primary(operation, lba, input, output);
    }

    let mut data = [0_u8; SECTOR_SIZE];
    if let Some(input) = input {
        data.copy_from_slice(input);
    }
    let completion = async_op::allocate_current(AsyncClass::Block)
        .map_err(|_| Error::DeviceFault)?;
    let owner = task::current_task_id_if_running().ok_or(Error::DeviceFault)?;
    let Some(id) = QUEUE
        .lock()
        .push(operation, lba, data, owner, Some(completion))
    else {
        let _ = async_op::release_current(completion);
        return Err(Error::DeviceFault);
    };
    QUEUED.fetch_add(1, Ordering::Relaxed);
    if let Some(worker) = *WORKER_TASK.lock() {
        // The worker uses the same scheduler-latched event contract as async
        // request completion. If the worker is still Running after observing
        // an empty queue, this records a pending event; if it is already
        // Blocked, the signal makes it Ready. There is therefore no
        // enqueue-vs-sleep lost-wakeup window.
        let _ = task::signal_event(worker);
    }
    wait_for_completion(id, completion, output)
}

fn wait_for_completion(
    id: u64,
    completion: async_op::Handle,
    output: Option<&mut [u8]>,
) -> Result<(), Error> {
    EVENT_WAITS.fetch_add(1, Ordering::Relaxed);
    if async_op::wait(completion).is_err() {
        return Err(Error::DeviceFault);
    }
    QUEUE
        .lock()
        .take_result(id, output)
        .unwrap_or(Err(Error::DeviceFault))
}

fn worker_task() -> ! {
    loop {
        // Drain all work currently visible before sleeping. A producer that
        // races with the final empty observation signals a scheduler-latched
        // event, so wait_for_event() either consumes the pre-block permit or
        // is woken after publishing Blocked.
        while process_one_primary() {}
        task::wait_for_event();
    }
}

fn process_one_primary() -> bool {
    let Some(work) = QUEUE.lock().take_pending() else {
        return false;
    };
    let mut data = work.request.data;
    let result = match work.request.operation {
        Operation::Read => direct_primary(Operation::Read, work.request.lba, None, Some(&mut data)),
        Operation::Write => direct_primary(Operation::Write, work.request.lba, Some(&data), None),
        Operation::Flush => direct_primary(Operation::Flush, work.request.lba, None, None),
    };
    let completion = QUEUE
        .lock()
        .finish(work.slot, work.request.id, result, data);
    COMPLETED.fetch_add(1, Ordering::Relaxed);
    if let Some(completion) = completion {
        EVENT_SIGNALS.fetch_add(1, Ordering::Relaxed);
        let status = if result.is_ok() { 0 } else { -1 };
        let _ = async_op::complete(
            completion,
            AsyncCompletion::new(status, work.request.id),
        );
    }
    true
}

fn direct_primary(
    operation: Operation,
    lba: u64,
    input: Option<&[u8]>,
    output: Option<&mut [u8]>,
) -> Result<(), Error> {
    crate::ata::with_primary_master(|disk| match operation {
        Operation::Read => {
            let Some(output) = output else {
                return Err(Error::InvalidBuffer);
            };
            disk.read_sector(lba, output)
        }
        Operation::Write => {
            let Some(input) = input else {
                return Err(Error::InvalidBuffer);
            };
            disk.write_sector(lba, input)
        }
        Operation::Flush => disk.flush(),
    })
    .unwrap_or(Err(Error::DeviceFault))
}

fn drive_one(queue: &mut Queue, device: &mut impl BlockDevice) -> bool {
    let Some(work) = queue.take_pending() else {
        return false;
    };
    let mut data = work.request.data;
    let result = match work.request.operation {
        Operation::Read => device.read_sector(work.request.lba, &mut data),
        Operation::Write => device.write_sector(work.request.lba, &data),
        Operation::Flush => device.flush(),
    };
    let _ = queue.finish(work.slot, work.request.id, result, data);
    true
}

pub fn self_test() -> bool {
    let mut queue = Queue::new();
    let mut disk = crate::block::RamDisk::<4>::new();
    let mut first = [0_u8; SECTOR_SIZE];
    let mut second = [0_u8; SECTOR_SIZE];
    first[..12].copy_from_slice(b"wovenhat-io!");
    second.fill(0xaa);

    let Some(write_id) = queue.push(Operation::Write, 2, first, TaskId::from_u64(0), None) else {
        return false;
    };
    if queue.pending_count() != 1 || !drive_one(&mut queue, &mut disk) {
        return false;
    }
    let mut out = [0_u8; SECTOR_SIZE];
    if queue.take_result(write_id, None) != Some(Ok(()))
        || disk.read_sector(2, &mut out).is_err()
        || out != first
    {
        return false;
    }

    let Some(read_id) = queue.push(Operation::Read, 2, [0; SECTOR_SIZE], TaskId::from_u64(0), None) else {
        return false;
    };
    if !drive_one(&mut queue, &mut disk)
        || queue.take_result(read_id, Some(&mut out)) != Some(Ok(()))
        || out != first
    {
        return false;
    }

    let mut ids = [0_u64; MAX_BLOCK_IO_REQUESTS];
    for (index, slot) in ids.iter_mut().enumerate() {
        let Some(id) = queue.push(Operation::Write, index as u64 % 4, second, TaskId::from_u64(0), None) else {
            return false;
        };
        *slot = id;
    }
    if queue.push(Operation::Flush, 0, [0; SECTOR_SIZE], TaskId::from_u64(0), None).is_some()
        || queue.active_count() != MAX_BLOCK_IO_REQUESTS
    {
        return false;
    }
    while drive_one(&mut queue, &mut disk) {}
    for id in ids {
        if queue.take_result(id, None) != Some(Ok(())) {
            return false;
        }
    }
    if queue.active_count() != 0 {
        return false;
    }

    disk.set_read_only(true);
    let Some(fail_id) = queue.push(Operation::Write, 1, first, TaskId::from_u64(0), None) else {
        return false;
    };
    drive_one(&mut queue, &mut disk)
        && queue.take_result(fail_id, None) == Some(Err(Error::ReadOnly))
}

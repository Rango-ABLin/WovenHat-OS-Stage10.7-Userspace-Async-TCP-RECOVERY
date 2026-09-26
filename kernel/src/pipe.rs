//! In-kernel anonymous pipes (POSIX subset) with blocking reads/writes.
//!
//! Empty reads block while writers exist; full writes block while readers
//! exist. Closing an end wakes the opposite waiters. No signal interruption.

use crate::irq_lock::IrqMutex as Mutex;

use crate::task::{self, TaskId};

const PIPE_BUFFER: usize = 2048;
const MAX_PIPES: usize = 32;
const MAX_WAITERS: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PipeId {
    slot: usize,
    epoch: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Full,
    Invalid,
    Closed,
}

struct Pipe {
    epoch: u64,
    data: [u8; PIPE_BUFFER],
    head: usize,
    tail: usize,
    len: usize,
    readers: u32,
    writers: u32,
    occupied: bool,
    read_waiters: [Option<TaskId>; MAX_WAITERS],
    write_waiters: [Option<TaskId>; MAX_WAITERS],
}

impl Pipe {
    const fn empty() -> Self {
        Self {
            epoch: 0,
            data: [0; PIPE_BUFFER],
            head: 0,
            tail: 0,
            len: 0,
            readers: 0,
            writers: 0,
            occupied: false,
            read_waiters: [None; MAX_WAITERS],
            write_waiters: [None; MAX_WAITERS],
        }
    }
}

struct Table {
    pipes: [Pipe; MAX_PIPES],
    generations: [u64; MAX_PIPES],
}

impl Table {
    const fn new() -> Self {
        Self {
            pipes: [const { Pipe::empty() }; MAX_PIPES],
            generations: [0; MAX_PIPES],
        }
    }

    fn alloc(&mut self) -> Result<PipeId, Error> {
        let slot = self
            .pipes
            .iter()
            .enumerate()
            .position(|(slot, p)| !p.occupied && self.generations[slot] < u64::MAX)
            .ok_or(Error::Full)?;
        let epoch = self.generations[slot].checked_add(1).ok_or(Error::Full)?;
        self.generations[slot] = epoch;
        self.pipes[slot] = Pipe {
            epoch,
            data: [0; PIPE_BUFFER],
            head: 0,
            tail: 0,
            len: 0,
            readers: 1,
            writers: 1,
            occupied: true,
            read_waiters: [None; MAX_WAITERS],
            write_waiters: [None; MAX_WAITERS],
        };
        Ok(PipeId { slot, epoch })
    }

    fn get(&self, id: PipeId) -> Result<&Pipe, Error> {
        self.pipes
            .get(id.slot)
            .filter(|pipe| pipe.occupied && pipe.epoch == id.epoch)
            .ok_or(Error::Invalid)
    }

    fn get_mut(&mut self, id: PipeId) -> Result<&mut Pipe, Error> {
        self.pipes
            .get_mut(id.slot)
            .filter(|pipe| pipe.occupied && pipe.epoch == id.epoch)
            .ok_or(Error::Invalid)
    }

    fn push_waiter(slots: &mut [Option<TaskId>; MAX_WAITERS], id: TaskId) -> bool {
        if slots.contains(&Some(id)) {
            return true;
        }
        for slot in slots.iter_mut() {
            if slot.is_none() {
                *slot = Some(id);
                return true;
            }
        }
        false
    }

    fn take_waiters(slots: &mut [Option<TaskId>; MAX_WAITERS]) -> [Option<TaskId>; MAX_WAITERS] {
        let out = *slots;
        *slots = [None; MAX_WAITERS];
        out
    }

    fn try_write(&mut self, id: PipeId, buf: &[u8]) -> Result<(usize, bool), Error> {
        let pipe = self.get_mut(id)?;
        if pipe.readers == 0 {
            return Err(Error::Closed);
        }
        let mut written = 0usize;
        while written < buf.len() && pipe.len < PIPE_BUFFER {
            pipe.data[pipe.tail] = buf[written];
            pipe.tail = (pipe.tail + 1) % PIPE_BUFFER;
            pipe.len += 1;
            written += 1;
        }
        let need_block = written < buf.len() && pipe.readers > 0;
        Ok((written, need_block && written == 0))
    }

    fn try_read(&mut self, id: PipeId, buf: &mut [u8]) -> Result<(usize, bool, bool), Error> {
        // returns (n, should_block, eof)
        let pipe = self.get_mut(id)?;
        if pipe.len == 0 {
            if pipe.writers == 0 {
                return Ok((0, false, true));
            }
            return Ok((0, true, false));
        }
        let mut read = 0usize;
        while read < buf.len() && pipe.len > 0 {
            buf[read] = pipe.data[pipe.head];
            pipe.head = (pipe.head + 1) % PIPE_BUFFER;
            pipe.len -= 1;
            read += 1;
        }
        Ok((read, false, false))
    }
}

static TABLE: Mutex<Table> = Mutex::with_rank(Table::new(), 10);

fn wake_list(waiters: [Option<TaskId>; MAX_WAITERS]) {
    for id in waiters.into_iter().flatten() {
        let _ = task::signal_event(id);
    }
}

pub fn create() -> Result<PipeId, Error> {
    TABLE.lock().alloc()
}

/// Blocking write: waits until at least one byte is written or the pipe is closed.
pub fn write(id: PipeId, buf: &[u8]) -> Result<usize, Error> {
    if buf.is_empty() {
        return Ok(0);
    }
    let mut total = 0usize;
    while total < buf.len() {
        let task_id = task::current_task_id();
        let (written, block) = {
            let mut table = TABLE.lock();
            let result = table.try_write(id, &buf[total..])?;
            if result.1 && !Table::push_waiter(&mut table.get_mut(id)?.write_waiters, task_id) {
                return Err(Error::Full);
            }
            result
        };
        if written > 0 {
            total += written;
            let waiters = {
                let mut table = TABLE.lock();
                table
                    .get_mut(id)
                    .map(|p| Table::take_waiters(&mut p.read_waiters))
                    .unwrap_or([None; MAX_WAITERS])
            };
            wake_list(waiters);
        }
        if total == buf.len() {
            break;
        }
        if block {
            let still_block = {
                let table = TABLE.lock();
                table
                    .get(id)
                    .is_ok_and(|p| p.readers > 0 && p.len >= PIPE_BUFFER)
            };
            if !still_block {
                continue;
            }
            // A peer may signal between the condition check and this call.
            // The scheduler latch consumes that signal instead of sleeping.
            task::wait_for_event();
            continue;
        }
        if written == 0 {
            // readers gone
            return if total > 0 {
                Ok(total)
            } else {
                Err(Error::Closed)
            };
        }
    }
    Ok(total)
}

/// Blocking read: waits for data or EOF (all writers closed).
pub fn read(id: PipeId, buf: &mut [u8]) -> Result<usize, Error> {
    if buf.is_empty() {
        return Ok(0);
    }
    loop {
        let task_id = task::current_task_id();
        let (n, block, eof) = {
            let mut table = TABLE.lock();
            let result = table.try_read(id, buf)?;
            if result.1 && !Table::push_waiter(&mut table.get_mut(id)?.read_waiters, task_id) {
                return Err(Error::Full);
            }
            result
        };
        if n > 0 {
            let waiters = {
                let mut table = TABLE.lock();
                table
                    .get_mut(id)
                    .map(|p| Table::take_waiters(&mut p.write_waiters))
                    .unwrap_or([None; MAX_WAITERS])
            };
            wake_list(waiters);
            return Ok(n);
        }
        if eof {
            return Ok(0);
        }
        if block {
            let still_block = {
                let table = TABLE.lock();
                table.get(id).is_ok_and(|p| p.len == 0 && p.writers > 0)
            };
            if !still_block {
                continue;
            }
            task::wait_for_event();
            continue;
        }
        return Ok(0);
    }
}

pub fn clone_reader(id: PipeId) -> Result<(), Error> {
    let mut table = TABLE.lock();
    let pipe = table.get_mut(id)?;
    pipe.readers = pipe.readers.checked_add(1).ok_or(Error::Full)?;
    Ok(())
}

pub fn clone_writer(id: PipeId) -> Result<(), Error> {
    let mut table = TABLE.lock();
    let pipe = table.get_mut(id)?;
    pipe.writers = pipe.writers.checked_add(1).ok_or(Error::Full)?;
    Ok(())
}

pub fn close_reader(id: PipeId) {
    let waiters = {
        let mut table = TABLE.lock();
        let Ok(pipe) = table.get_mut(id) else {
            return;
        };
        pipe.readers = pipe.readers.saturating_sub(1);
        let w = Table::take_waiters(&mut pipe.write_waiters);
        if pipe.readers == 0 && pipe.writers == 0 {
            *pipe = Pipe::empty();
        }
        w
    };
    wake_list(waiters);
}

pub fn close_writer(id: PipeId) {
    let waiters = {
        let mut table = TABLE.lock();
        let Ok(pipe) = table.get_mut(id) else {
            return;
        };
        pipe.writers = pipe.writers.saturating_sub(1);
        let w = Table::take_waiters(&mut pipe.read_waiters);
        if pipe.readers == 0 && pipe.writers == 0 {
            *pipe = Pipe::empty();
        }
        w
    };
    wake_list(waiters);
}

pub fn self_test() -> bool {
    let mut waiters = [None; MAX_WAITERS];
    let waiter_bound = (0..MAX_WAITERS)
        .all(|index| Table::push_waiter(&mut waiters, TaskId::from_u64(index as u64)))
        && Table::push_waiter(&mut waiters, TaskId::from_u64(0))
        && !Table::push_waiter(&mut waiters, TaskId::from_u64(MAX_WAITERS as u64));
    let Ok(id) = create() else {
        return false;
    };
    let payload = b"blocking-pipe-test";
    if write(id, payload) != Ok(payload.len()) {
        close_writer(id);
        close_reader(id);
        return false;
    }
    let mut buf = [0u8; 32];
    let ok = read(id, &mut buf) == Ok(payload.len()) && &buf[..payload.len()] == payload;
    close_writer(id);
    // EOF after writers closed
    let eof = read(id, &mut buf) == Ok(0);
    close_reader(id);
    let reuse_safe = match create() {
        Ok(reused) => {
            let stale_rejected = reused != id
                && clone_reader(id) == Err(Error::Invalid)
                && clone_writer(id) == Err(Error::Invalid)
                && read(id, &mut buf) == Err(Error::Invalid)
                && write(id, b"x") == Err(Error::Invalid);
            // A delayed close from the retired epoch must not touch the new pipe.
            close_reader(id);
            close_writer(id);
            let fresh_intact = write(reused, b"y") == Ok(1)
                && read(reused, &mut buf[..1]) == Ok(1)
                && buf[0] == b'y';
            close_writer(reused);
            close_reader(reused);
            stale_rejected && fresh_intact
        }
        Err(_) => false,
    };
    waiter_bound && ok && eof && reuse_safe
}

#[allow(dead_code)]
pub const fn buffer_size() -> usize {
    PIPE_BUFFER
}

use core::{
    arch::{asm, global_asm},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use crate::config::{MAX_IO_SIZE, MAX_PATH_SIZE};

global_asm!(
    ".global wovenhat_syscall_entry",
    "wovenhat_syscall_entry:",
    "push rax",
    "push rbx",
    "push rcx",
    "push rdx",
    "push rsi",
    "push rdi",
    "push rbp",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov rdi, [rsp + 112]",
    "mov rsi, [rsp + 72]",
    "mov rdx, [rsp + 80]",
    "mov rcx, [rsp + 88]",
    "mov r8, rsp",
    "mov r12, rsp",
    "and rsp, -16",
    "call wovenhat_syscall_dispatch",
    "mov rsp, r12",
    "mov [rsp + 112], rax",
    ".global wovenhat_syscall_resume",
    "wovenhat_syscall_resume:",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rbp",
    "pop rdi",
    "pop rsi",
    "pop rdx",
    "pop rcx",
    "pop rbx",
    "pop rax",
    "iretq",
);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct UserForkFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

impl UserForkFrame {
    pub fn child_return(mut self) -> Self {
        self.rax = 0;
        self
    }
}

static LAST_COMPLETED: AtomicU64 = AtomicU64::new(u64::MAX);
static COMPLETIONS: AtomicU64 = AtomicU64::new(0);
static IO_COMPLETIONS: AtomicU64 = AtomicU64::new(0);
static YIELD_PENDING: AtomicBool = AtomicBool::new(false);
static ASYNC_ABI_COMPLETIONS: AtomicU64 = AtomicU64::new(0);

const IO_WRITE: u64 = 1 << 0;
const IO_OPEN: u64 = 1 << 1;
const IO_READ: u64 = 1 << 2;
const IO_CLOSE: u64 = 1 << 3;
const IO_MMAP: u64 = 1 << 4;
const IO_MUNMAP: u64 = 1 << 5;
const IO_FILE_WRITE: u64 = 1 << 6;
const IO_GETUID: u64 = 1 << 7;
const IO_GETGID: u64 = 1 << 8;
const IO_EXEC: u64 = 1 << 9;
const IO_FORK: u64 = 1 << 10;
const IO_STDIN: u64 = 1 << 11;
const IO_STDERR: u64 = 1 << 12;

const ASYNC_CREATE: u64 = 1 << 0;
const ASYNC_POLL: u64 = 1 << 1;
const ASYNC_WAIT: u64 = 1 << 2;
const ASYNC_CANCEL: u64 = 1 << 3;
const ASYNC_COMPLETE: u64 = 1 << 4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Number {
    Read = 0,
    Write = 1,
    Open = 2,
    Exit = 3,
    Getpid = 4,
    Waitpid = 5,
    Close = 6,
    Yield = 7,
    Mmap = 8,
    Munmap = 9,
    FileWrite = 10,
    MessageSend = 11,
    MessageReceive = 12,
    Getuid = 13,
    Getgid = 14,
    Exec = 15,
    Fork = 16,
    /// path ptr, path len → packs kind|size|writable into return value
    Stat = 17,
    /// path ptr, path len, entry index → writes DirEntry to user buffer in RSI...
    /// Actually: arg0=path, arg1=path_len, arg2=index; entry copied via a side channel is awkward.
    /// Convention: arg0=path ptr, arg1=path len | (index<<32), arg2=user buffer for name (MAX_DIR_NAME).
    /// Return: name length | (kind<<16), or error.
    Readdir = 18,
    /// path ptr, path len
    Mkdir = 19,
    /// path ptr, path len
    Chdir = 20,
    /// user buffer, capacity → length written
    Getcwd = 21,
    Dup = 22,
    /// returns read_fd in low 32 bits, write_fd in high 32 bits
    Pipe = 23,
    /// oldfd, newfd
    Dup2 = 24,
    Getppid = 25,
    /// pid, sig — minimal: record pending signal on target
    Kill = 26,
    Lseek = 27,
    Unlink = 28,
    Sleep = 29,
    Rename = 30,
    Getticks = 31,
    Sync = 32,
    Ioctl = 33,
    /// sig, handler (0=default, 1=ignore, otherwise userspace address)
    Sigaction = 34,
    Getpgrp = 35,
    /// pid, pgid (0 follows POSIX current/default conventions)
    Setpgid = 36,
    /// kind: 1=UDP, 2=TCP -> process-owned network descriptor
    Socket = 37,
    /// socket, local port
    Bind = 38,
    /// socket, packed IPv4 endpoint
    Connect = 39,
    /// socket, user buffer, length
    NetSend = 40,
    /// socket, user buffer, capacity
    NetRecv = 41,
    NetClose = 42,
    /// user pointer to network::NetInfo
    NetInfo = 43,
    /// hostname pointer, length -> query id
    DnsStart = 44,
    /// query id, user IPv4[4] buffer -> 0 pending, 1 complete
    DnsPoll = 45,
    /// socket -> packed peer endpoint
    NetPeer = 46,
    /// 0=static fallback, nonzero=enable DHCP
    Dhcp = 47,
    /// packed IPv4 in low 32 bits
    PingStart = 48,
    /// 0 pending, otherwise RTT ticks + 1
    PingPoll = 49,
    /// command-line pointer + byte length; parses argv and resolves bare names via /bin
    ExecCommand = 50,
    /// key ptr, key len, value buffer ptr (MAX_ENV_ENTRY bytes) -> value length
    EnvGet = 51,
    /// key ptr, packed key/value lengths, value ptr -> 0
    EnvSet = 52,
    /// number of environment entries
    EnvCount = 53,
    /// index, output ptr, capacity -> entry length (KEY=VALUE)
    EnvEntry = 54,
    /// number of live/zombie process-table entries
    ProcessCount = 55,
    /// index, user ProcessInfo pointer -> 0
    ProcessInfo = 56,
    /// command-line pointer + byte length -> spawn child, return pid
    SpawnCommand = 57,
    /// fd, byte length, page-aligned file offset: read-only private snapshot.
    MmapFile = 58,
    /// fd, byte length, page-aligned file offset: writable private snapshot.
    MmapFileWritable = 59,
    MmapFileLazy = 60,
    MmapFileLazyWritable = 61,
    MmapFileShared = 62,
    Msync = 63,
    /// class -> generation-tagged owner-bound async handle
    AsyncCreate = 64,
    /// handle, user Completion* -> 0 pending, 1 completed/consumed
    AsyncPoll = 65,
    /// handle, user Completion* -> blocks event-driven, returns 1 on copyout
    AsyncWait = 66,
    /// handle -> release pending/completed operation without waiting
    AsyncCancel = 67,
    /// service handle, status, value -> complete an owner-managed service event
    AsyncComplete = 68,
    /// LBA -> generation-tagged Block handle backed by the real block worker
    AsyncBlockRead = 69,
    /// LBA, userspace 512-byte sector -> generation-tagged Block handle
    AsyncBlockWrite = 70,
    /// Block handle, user BlockCompletion* -> 0 pending, 1 copied/consumed
    AsyncBlockPoll = 71,
    /// Block handle, user BlockCompletion* -> blocks event-driven, 1 copied/consumed
    AsyncBlockWait = 72,
    /// fd, user AsyncFileRequest* -> positional read handle
    AsyncFileRead = 73,
    /// fd, user AsyncFileRequest* -> positional write handle; payload copied at submit
    AsyncFileWrite = 74,
    /// File handle, user Completion*, user data* -> 0 pending, 1 copied/consumed
    AsyncFilePoll = 75,
    /// File handle, user Completion*, user data* -> event wait, 1 copied/consumed
    AsyncFileWait = 76,
    /// socket, user buffer, length -> generation-tagged Network send handle
    AsyncNetSend = 77,
    /// socket, capacity -> generation-tagged Network receive handle
    AsyncNetRecv = 78,
    /// Network handle, user Completion*, user data* -> 0 pending, 1 copied/consumed
    AsyncNetPoll = 79,
    /// Network handle, user Completion*, user data* -> event wait, 1 copied/consumed
    AsyncNetWait = 80,
    /// TCP socket, packed IPv4 endpoint -> generation-tagged Network connect handle
    AsyncTcpConnect = 81,
    CompletionPortCreate = 82,
    CompletionPortClose = 83,
    CompletionPortAssociate = 84,
    CompletionPortPoll = 85,
    CompletionPortWait = 86,
    TimerCreate = 87,
    TimerWait = 88,
    EventCreate = 89,
    EventSet = 90,
    EventWait = 91,
    TimerClose = 92,
    EventClose = 93,
    SleepUntil = 94,
    /// user notification buffer -> 0 empty, 1 record copied
    NotificationPoll = 95,
}

pub fn entry_address() -> u64 {
    unsafe extern "C" {
        fn wovenhat_syscall_entry();
    }

    wovenhat_syscall_entry as *const () as u64
}

pub fn emit(number: Number) -> u64 {
    invoke(number, 0, 0, 0)
}

pub fn invoke(number: Number, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    let result: u64;

    // SAFETY: Vector 0x80 is a DPL3 interrupt gate whose assembly entry saves
    // all general registers and returns the dispatch result in RAX.
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") number as u64 => result,
            in("rdi") arg0,
            in("rsi") arg1,
            in("rdx") arg2,
            options(preserves_flags),
        );
    }

    result
}
fn fork_frame_layout_valid() -> bool {
    let frame = UserForkFrame {
        rax: 7,
        rip: 0x4000,
        rsp: 0x8000,
        ..UserForkFrame::default()
    };
    let child = frame.child_return();
    core::mem::size_of::<UserForkFrame>() == 20 * core::mem::size_of::<u64>()
        && child.rax == 0
        && child.rip == frame.rip
        && child.rsp == frame.rsp
}
pub fn test() -> bool {
    let before = COMPLETIONS.load(Ordering::Acquire);
    let result = emit(Number::Getpid);
    fork_frame_layout_valid()
        && COMPLETIONS.load(Ordering::Acquire) == before + 1
        && LAST_COMPLETED.load(Ordering::Acquire) == Number::Getpid as u64
        && result == crate::task::current_process_id()
}

pub fn user_io_verified() -> bool {
    IO_COMPLETIONS.load(Ordering::Acquire) & (IO_WRITE | IO_OPEN | IO_READ | IO_CLOSE)
        == IO_WRITE | IO_OPEN | IO_READ | IO_CLOSE
}

pub fn user_standard_streams_verified() -> bool {
    IO_COMPLETIONS.load(Ordering::Acquire) & (IO_STDIN | IO_STDERR) == IO_STDIN | IO_STDERR
}
pub fn user_memory_verified() -> bool {
    IO_COMPLETIONS.load(Ordering::Acquire) & (IO_MMAP | IO_MUNMAP) == IO_MMAP | IO_MUNMAP
}

pub fn user_identity_verified() -> bool {
    IO_COMPLETIONS.load(Ordering::Acquire) & (IO_GETUID | IO_GETGID) == IO_GETUID | IO_GETGID
}

pub fn user_exec_verified() -> bool {
    IO_COMPLETIONS.load(Ordering::Acquire) & IO_EXEC == IO_EXEC
}
pub fn user_fork_verified() -> bool {
    IO_COMPLETIONS.load(Ordering::Acquire) & IO_FORK == IO_FORK
}

pub fn user_async_verified() -> bool {
    let required = ASYNC_CREATE | ASYNC_POLL | ASYNC_WAIT | ASYNC_CANCEL | ASYNC_COMPLETE;
    ASYNC_ABI_COMPLETIONS.load(Ordering::Acquire) & required == required
}
pub fn last_completed(number: Number) -> bool {
    LAST_COMPLETED.load(Ordering::Acquire) == number as u64
}

pub fn service_pending() {
    if YIELD_PENDING.swap(false, Ordering::AcqRel) {
        crate::task::yield_now();
    }
}

const SYSCALL_ERROR: u64 = u64::MAX;

fn sys_mmap(length: u64, flags: u64) -> u64 {
    if flags & !1 != 0 {
        return SYSCALL_ERROR;
    }
    match crate::task::mmap_current(length, flags & 1 != 0) {
        Ok(address) => {
            IO_COMPLETIONS.fetch_or(IO_MMAP, Ordering::Release);
            address
        }
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_munmap(address: u64, length: u64) -> u64 {
    match crate::task::munmap_current(address, length) {
        Ok(()) => {
            IO_COMPLETIONS.fetch_or(IO_MUNMAP, Ordering::Release);
            0
        }
        Err(_) => SYSCALL_ERROR,
    }
}
fn sys_write(descriptor: u64, user_buffer: u64, length: u64) -> u64 {
    if !matches!(descriptor, 1 | 2) {
        return SYSCALL_ERROR;
    }
    let Ok(length) = usize::try_from(length) else {
        return SYSCALL_ERROR;
    };
    if length > MAX_IO_SIZE {
        return SYSCALL_ERROR;
    }
    let mut buffer = [0_u8; MAX_IO_SIZE];
    if crate::paging::copy_from_current_user(user_buffer, &mut buffer[..length]).is_err() {
        return SYSCALL_ERROR;
    }

    // If stdout/stderr has been explicitly redirected with dup2(), honor the
    // process file table. Otherwise these descriptors remain the console.
    if crate::task::descriptor_is_open(descriptor) {
        let Ok(count) = crate::task::write_current(descriptor, &buffer[..length]) else {
            return SYSCALL_ERROR;
        };
        IO_COMPLETIONS.fetch_or(IO_WRITE, Ordering::Release);
        if descriptor == 2 {
            IO_COMPLETIONS.fetch_or(IO_STDERR, Ordering::Release);
        }
        return count as u64;
    }

    if !crate::task::current_has(crate::capability::Capability::Console) {
        return SYSCALL_ERROR;
    }
    crate::serial::write_bytes(&buffer[..length]);
    crate::terminal::write_bytes(&buffer[..length]);
    IO_COMPLETIONS.fetch_or(IO_WRITE, Ordering::Release);
    if descriptor == 2 {
        IO_COMPLETIONS.fetch_or(IO_STDERR, Ordering::Release);
    }
    length as u64
}

fn sys_open(user_path: u64, length: u64) -> u64 {
    let Ok(length) = usize::try_from(length) else {
        return SYSCALL_ERROR;
    };
    if length == 0 || length > MAX_PATH_SIZE {
        return SYSCALL_ERROR;
    }
    let mut path = [0_u8; MAX_PATH_SIZE];
    if crate::paging::copy_from_current_user(user_path, &mut path[..length]).is_err() {
        return SYSCALL_ERROR;
    }
    let Ok(path) = core::str::from_utf8(&path[..length]) else {
        return SYSCALL_ERROR;
    };
    match crate::task::open_current(path) {
        Ok(descriptor) => {
            IO_COMPLETIONS.fetch_or(IO_OPEN, Ordering::Release);
            descriptor
        }
        Err(_) => SYSCALL_ERROR,
    }
}

/// `stat(path)` → packed result:
/// bits[7:0]   = kind (0=file, 1=directory)
/// bits[39:8]  = size (24 bits)
/// bit 40      = writable
fn sys_stat(user_path: u64, length: u64) -> u64 {
    let Ok(length) = usize::try_from(length) else {
        return SYSCALL_ERROR;
    };
    if length == 0 || length > MAX_PATH_SIZE {
        return SYSCALL_ERROR;
    }
    let mut path_buf = [0_u8; MAX_PATH_SIZE];
    if crate::paging::copy_from_current_user(user_path, &mut path_buf[..length]).is_err() {
        return SYSCALL_ERROR;
    }
    let Ok(path) = core::str::from_utf8(&path_buf[..length]) else {
        return SYSCALL_ERROR;
    };
    match crate::task::stat_path(path) {
        Ok(stat) => {
            let kind = match stat.kind {
                crate::vfs::NodeKind::File => 0u64,
                crate::vfs::NodeKind::Directory => 1u64,
            };
            let size = (stat.size as u64) & 0x00FF_FFFF;
            let writable = u64::from(stat.writable);
            kind | (size << 8) | (writable << 40)
        }
        Err(_) => SYSCALL_ERROR,
    }
}

/// `readdir(path, index, name_buf)`
/// arg0 = path pointer, arg1 = path length, arg2 = (index << 32) | name_buf_ptr low...
/// Simpler: arg0=path, arg1=path_len, arg2=index.
/// Name is written to a fixed side-buffer? No — use:
///   arg0 = path ptr
///   arg1 = path_len | (index << 16)
///   arg2 = user buffer for name (at least MAX_DIR_NAME bytes)
/// Return: name_length | (kind << 8) on success.
fn sys_readdir(user_path: u64, path_len_and_index: u64, user_name_buf: u64) -> u64 {
    let path_len = (path_len_and_index & 0xFFFF) as usize;
    let index = (path_len_and_index >> 16) as usize;
    if path_len == 0 || path_len > MAX_PATH_SIZE {
        return SYSCALL_ERROR;
    }
    let mut path_buf = [0_u8; MAX_PATH_SIZE];
    if crate::paging::copy_from_current_user(user_path, &mut path_buf[..path_len]).is_err() {
        return SYSCALL_ERROR;
    }
    let Ok(path) = core::str::from_utf8(&path_buf[..path_len]) else {
        return SYSCALL_ERROR;
    };
    match crate::task::readdir_path(path, index) {
        Ok(entry) => {
            if crate::paging::copy_to_current_user(user_name_buf, &entry.name[..entry.name_length])
                .is_err()
            {
                return SYSCALL_ERROR;
            }
            let kind = match entry.kind {
                crate::vfs::NodeKind::File => 0u64,
                crate::vfs::NodeKind::Directory => 1u64,
            };
            (entry.name_length as u64) | (kind << 8)
        }
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_chdir(user_path: u64, length: u64) -> u64 {
    let Ok(length) = usize::try_from(length) else {
        return SYSCALL_ERROR;
    };
    if length == 0 || length > MAX_PATH_SIZE {
        return SYSCALL_ERROR;
    }
    let mut path_buf = [0_u8; MAX_PATH_SIZE];
    if crate::paging::copy_from_current_user(user_path, &mut path_buf[..length]).is_err() {
        return SYSCALL_ERROR;
    }
    let Ok(path) = core::str::from_utf8(&path_buf[..length]) else {
        return SYSCALL_ERROR;
    };
    match crate::task::chdir_current(path) {
        Ok(()) => 0,
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_getcwd(user_buffer: u64, capacity: u64) -> u64 {
    let Ok(capacity) = usize::try_from(capacity) else {
        return SYSCALL_ERROR;
    };
    if capacity == 0 || capacity > MAX_PATH_SIZE {
        return SYSCALL_ERROR;
    }
    let mut path_buf = [0_u8; MAX_PATH_SIZE];
    let Ok(len) = crate::task::current_cwd_str(&mut path_buf) else {
        return SYSCALL_ERROR;
    };
    if len > capacity {
        return SYSCALL_ERROR;
    }
    if crate::paging::copy_to_current_user(user_buffer, &path_buf[..len]).is_err() {
        return SYSCALL_ERROR;
    }
    len as u64
}

fn sys_mkdir(user_path: u64, length: u64) -> u64 {
    let Ok(length) = usize::try_from(length) else {
        return SYSCALL_ERROR;
    };
    if length == 0 || length > MAX_PATH_SIZE {
        return SYSCALL_ERROR;
    }
    let mut path_buf = [0_u8; MAX_PATH_SIZE];
    if crate::paging::copy_from_current_user(user_path, &mut path_buf[..length]).is_err() {
        return SYSCALL_ERROR;
    }
    let Ok(path) = core::str::from_utf8(&path_buf[..length]) else {
        return SYSCALL_ERROR;
    };
    match crate::task::mkdir_path(path) {
        Ok(()) => 0,
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_read(descriptor: u64, user_buffer: u64, length: u64) -> u64 {
    let Ok(length) = usize::try_from(length) else {
        return SYSCALL_ERROR;
    };
    if length > MAX_IO_SIZE {
        return SYSCALL_ERROR;
    }
    let mut buffer = [0_u8; MAX_IO_SIZE];
    if descriptor == 0 && !crate::task::descriptor_is_open(0) {
        if !crate::task::current_has(crate::capability::Capability::Console) {
            return SYSCALL_ERROR;
        }
        let count = crate::keyboard::read_bytes(&mut buffer[..length]);
        if crate::paging::copy_to_current_user(user_buffer, &buffer[..count]).is_err() {
            return SYSCALL_ERROR;
        }
        IO_COMPLETIONS.fetch_or(IO_STDIN, Ordering::Release);
        return count as u64;
    }
    let Ok(count) = crate::task::read_current(descriptor, &mut buffer[..length]) else {
        return SYSCALL_ERROR;
    };
    if crate::paging::copy_to_current_user(user_buffer, &buffer[..count]).is_err() {
        return SYSCALL_ERROR;
    }
    IO_COMPLETIONS.fetch_or(IO_READ, Ordering::Release);
    count as u64
}

fn sys_file_write(descriptor: u64, user_buffer: u64, length: u64) -> u64 {
    let actor = crate::task::current_process_id();
    let Ok(length) = usize::try_from(length) else {
        return SYSCALL_ERROR;
    };
    if length > MAX_IO_SIZE {
        return SYSCALL_ERROR;
    }
    let mut buffer = [0_u8; MAX_IO_SIZE];
    if crate::paging::copy_from_current_user(user_buffer, &mut buffer[..length]).is_err() {
        return SYSCALL_ERROR;
    }
    let result = crate::task::write_current(descriptor, &buffer[..length]);
    crate::audit::record(
        actor,
        crate::audit::Action::FileWrite,
        descriptor,
        result.is_ok(),
    );
    match result {
        Ok(count) => {
            IO_COMPLETIONS.fetch_or(IO_FILE_WRITE, Ordering::Release);
            count as u64
        }
        Err(_) => SYSCALL_ERROR,
    }
}
fn sys_message_send(receiver: u64, user_buffer: u64, length: u64) -> u64 {
    let actor = crate::task::current_process_id();
    if !crate::task::current_has(crate::capability::Capability::Ipc)
        || !crate::task::may_ipc_with(receiver)
    {
        crate::audit::record(actor, crate::audit::Action::IpcSend, receiver, false);
        return SYSCALL_ERROR;
    }
    let Ok(length) = usize::try_from(length) else {
        return SYSCALL_ERROR;
    };
    if length > crate::ipc::MAX_MESSAGE_SIZE {
        return SYSCALL_ERROR;
    }
    let mut buffer = [0_u8; crate::ipc::MAX_MESSAGE_SIZE];
    if crate::paging::copy_from_current_user(user_buffer, &mut buffer[..length]).is_err() {
        return SYSCALL_ERROR;
    }
    let sender = crate::task::current_process_id();
    let result = crate::ipc::send(sender, receiver, &buffer[..length]);
    crate::audit::record(
        actor,
        crate::audit::Action::IpcSend,
        receiver,
        result.is_ok(),
    );
    result.map_or(SYSCALL_ERROR, |()| 0)
}
fn sys_message_receive(user_buffer: u64, capacity: u64, user_sender: u64) -> u64 {
    if !crate::task::current_has(crate::capability::Capability::Ipc) {
        return SYSCALL_ERROR;
    }
    let Ok(capacity) = usize::try_from(capacity) else {
        return SYSCALL_ERROR;
    };
    let receiver = crate::task::current_process_id();
    let Ok(message) = crate::ipc::peek(receiver) else {
        return SYSCALL_ERROR;
    };
    if message.payload().len() > capacity
        || crate::paging::copy_to_current_user(user_buffer, message.payload()).is_err()
        || crate::paging::copy_to_current_user(user_sender, &message.sender.to_le_bytes()).is_err()
    {
        return SYSCALL_ERROR;
    }
    if crate::ipc::receive(receiver).is_err() {
        return SYSCALL_ERROR;
    }
    message.payload().len() as u64
}

fn sys_pipe() -> u64 {
    match crate::task::pipe_current() {
        Ok((r, w)) => r | (w << 32),
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_dup2(old: u64, new: u64) -> u64 {
    match crate::task::dup2_current(old, new) {
        Ok(fd) => fd,
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_getppid() -> u64 {
    crate::task::getppid_current()
}

fn sys_lseek(fd: u64, offset: u64) -> u64 {
    match crate::task::seek_current(fd, offset) {
        Ok(pos) => pos,
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_unlink(user_path: u64, length: u64) -> u64 {
    let Ok(length) = usize::try_from(length) else {
        return SYSCALL_ERROR;
    };
    if length == 0 || length > MAX_PATH_SIZE {
        return SYSCALL_ERROR;
    }
    let mut path = [0_u8; MAX_PATH_SIZE];
    if crate::paging::copy_from_current_user(user_path, &mut path[..length]).is_err() {
        return SYSCALL_ERROR;
    }
    let Ok(path) = core::str::from_utf8(&path[..length]) else {
        return SYSCALL_ERROR;
    };
    match crate::task::unlink_current(path) {
        Ok(()) => 0,
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_rename(user_old: u64, old_len_and_new_len: u64, user_new: u64) -> u64 {
    let old_len = (old_len_and_new_len & 0xffff) as usize;
    let new_len = ((old_len_and_new_len >> 16) & 0xffff) as usize;
    if old_len == 0 || new_len == 0 || old_len > MAX_PATH_SIZE || new_len > MAX_PATH_SIZE {
        return SYSCALL_ERROR;
    }
    let mut old = [0_u8; MAX_PATH_SIZE];
    let mut new = [0_u8; MAX_PATH_SIZE];
    if crate::paging::copy_from_current_user(user_old, &mut old[..old_len]).is_err() {
        return SYSCALL_ERROR;
    }
    if crate::paging::copy_from_current_user(user_new, &mut new[..new_len]).is_err() {
        return SYSCALL_ERROR;
    }
    let Ok(old) = core::str::from_utf8(&old[..old_len]) else {
        return SYSCALL_ERROR;
    };
    let Ok(new) = core::str::from_utf8(&new[..new_len]) else {
        return SYSCALL_ERROR;
    };
    match crate::task::rename_current(old, new) {
        Ok(()) => 0,
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_getticks() -> u64 {
    crate::timer::ticks()
}

fn sys_sync() -> u64 {
    crate::storage::sync_all_mounted()
        .map(|count| count as u64)
        .unwrap_or(SYSCALL_ERROR)
}

fn sys_ioctl(_fd: u64, _req: u64, _arg: u64) -> u64 {
    0 // no-op placeholder for future TTY control
}

fn sys_sleep(ticks: u64) -> u64 {
    crate::task::sleep_current(ticks);
    0
}

fn sys_kill(pid: u64, sig: u64) -> u64 {
    match crate::task::kill_process(pid, sig) {
        Ok(()) => 0,
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_dup(descriptor: u64) -> u64 {
    match crate::task::dup_current(descriptor) {
        Ok(fd) => fd,
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_close(descriptor: u64) -> u64 {
    match crate::task::close_current(descriptor) {
        Ok(()) => {
            IO_COMPLETIONS.fetch_or(IO_CLOSE, Ordering::Release);
            0
        }
        Err(_) => SYSCALL_ERROR,
    }
}
fn sys_fork(frame: *const UserForkFrame) -> u64 {
    let actor = crate::task::current_process_id();
    if !crate::task::current_has(crate::capability::Capability::ProcessCreate) {
        crate::audit::record(actor, crate::audit::Action::ProcessFork, 0, false);
        return SYSCALL_ERROR;
    }
    let Some(frame) = (unsafe { frame.as_ref() }) else {
        crate::audit::record(actor, crate::audit::Action::ProcessFork, 0, false);
        return SYSCALL_ERROR;
    };
    match crate::task::fork_current(*frame) {
        Ok(child) => {
            IO_COMPLETIONS.fetch_or(IO_FORK, Ordering::Release);
            crate::audit::record(
                actor,
                crate::audit::Action::ProcessFork,
                child.as_u64(),
                true,
            );
            child.as_u64()
        }
        Err(_) => {
            crate::audit::record(actor, crate::audit::Action::ProcessFork, 0, false);
            SYSCALL_ERROR
        }
    }
}
fn sys_sigaction(sig: u64, handler: u64) -> u64 {
    crate::task::sigaction_current(sig, handler).unwrap_or(SYSCALL_ERROR)
}

fn sys_setpgid(pid: u64, pgid: u64) -> u64 {
    match crate::task::set_process_group(pid, pgid) {
        Ok(()) => 0,
        Err(_) => SYSCALL_ERROR,
    }
}

/// Arrange a caught signal to enter its userspace handler after this syscall.
/// The handler receives `sig` in RDI and a normal `ret` resumes the interrupted RIP.
fn prepare_signal_delivery(frame: *const UserForkFrame) {
    let Some((sig, handler)) = crate::task::take_pending_signal_current() else {
        return;
    };
    if frame.is_null() {
        return;
    }
    let frame = unsafe { &mut *(frame as *mut UserForkFrame) };
    let Some(new_rsp) = frame.rsp.checked_sub(8) else {
        return;
    };
    if crate::paging::copy_to_current_user(new_rsp, &frame.rip.to_le_bytes()).is_err() {
        return;
    }
    frame.rsp = new_rsp;
    frame.rip = handler;
    frame.rdi = sig;
}

fn sys_exec(user_path: u64, length: u64) -> u64 {
    let actor = crate::task::current_process_id();
    if !crate::task::current_has(crate::capability::Capability::FileRead)
        || !crate::task::current_has(crate::capability::Capability::ProcessCreate)
    {
        crate::audit::record(actor, crate::audit::Action::ProcessExec, length, false);
        return SYSCALL_ERROR;
    }
    let Ok(length) = usize::try_from(length) else {
        return SYSCALL_ERROR;
    };
    if length == 0 || length > MAX_PATH_SIZE {
        return SYSCALL_ERROR;
    }
    let mut path = [0_u8; MAX_PATH_SIZE];
    if crate::paging::copy_from_current_user(user_path, &mut path[..length]).is_err() {
        return SYSCALL_ERROR;
    }
    let Ok(path) = core::str::from_utf8(&path[..length]) else {
        return SYSCALL_ERROR;
    };
    let mut image = alloc::vec![0_u8; crate::vfs::NODE_CAPACITY];
    let Ok(image_length) = crate::vfs::read_all(path, &mut image) else {
        crate::audit::record(
            actor,
            crate::audit::Action::ProcessExec,
            length as u64,
            false,
        );
        return SYSCALL_ERROR;
    };
    let Some(program) = crate::userspace::load_elf_with_argv(&image[..image_length], &[path])
    else {
        crate::audit::record(
            actor,
            crate::audit::Action::ProcessExec,
            length as u64,
            false,
        );
        return SYSCALL_ERROR;
    };
    drop(image);
    IO_COMPLETIONS.fetch_or(IO_EXEC, Ordering::Release);
    crate::audit::record(
        actor,
        crate::audit::Action::ProcessExec,
        length as u64,
        true,
    );
    crate::task::exec_current(program)
}

/// Execute a whitespace-delimited command line from userspace.
///
/// Stage 6 keeps the existing `exec(path,len)` ABI intact and adds this
/// argument-bearing form for the interactive shell. Bare command names are
/// resolved under `/bin`; paths containing `/` are used verbatim. The parser
/// intentionally supports simple ASCII whitespace only for now (no quoting or
/// environment expansion), bounded by userspace::MAX_ARGV/MAX_ARGV_BYTES.
fn sys_exec_command(user_line: u64, length: u64) -> u64 {
    let actor = crate::task::current_process_id();
    if !crate::task::current_has(crate::capability::Capability::FileRead)
        || !crate::task::current_has(crate::capability::Capability::ProcessCreate)
    {
        crate::audit::record(actor, crate::audit::Action::ProcessExec, length, false);
        return SYSCALL_ERROR;
    }

    let Ok(length) = usize::try_from(length) else {
        return SYSCALL_ERROR;
    };
    if length == 0 || length > crate::userspace::MAX_ARGV_BYTES {
        return SYSCALL_ERROR;
    }

    let mut command = [0_u8; crate::userspace::MAX_ARGV_BYTES];
    if crate::paging::copy_from_current_user(user_line, &mut command[..length]).is_err() {
        return SYSCALL_ERROR;
    }
    if !command[..length].is_ascii() {
        return SYSCALL_ERROR;
    }

    let mut ranges = [(0_usize, 0_usize); crate::userspace::MAX_ARGV];
    let mut argc = 0_usize;
    let mut cursor = 0_usize;
    while cursor < length {
        while cursor < length && command[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor == length {
            break;
        }
        if argc == crate::userspace::MAX_ARGV {
            return SYSCALL_ERROR;
        }
        let start = cursor;
        while cursor < length && !command[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        ranges[argc] = (start, cursor);
        argc += 1;
    }
    if argc == 0 {
        return SYSCALL_ERROR;
    }

    let Ok(command_name) = core::str::from_utf8(&command[ranges[0].0..ranges[0].1]) else {
        return SYSCALL_ERROR;
    };
    let resolved = if command_name.as_bytes().contains(&b'/') {
        alloc::string::String::from(command_name)
    } else {
        alloc::format!("/bin/{command_name}")
    };
    if resolved.len() > MAX_PATH_SIZE {
        return SYSCALL_ERROR;
    }

    let mut argv = [""; crate::userspace::MAX_ARGV];
    argv[0] = resolved.as_str();
    for index in 1..argc {
        let (start, end) = ranges[index];
        let Ok(argument) = core::str::from_utf8(&command[start..end]) else {
            return SYSCALL_ERROR;
        };
        argv[index] = argument;
    }

    let mut image = alloc::vec![0_u8; crate::vfs::NODE_CAPACITY];
    let Ok(image_length) = crate::vfs::read_all(resolved.as_str(), &mut image) else {
        crate::audit::record(
            actor,
            crate::audit::Action::ProcessExec,
            length as u64,
            false,
        );
        return SYSCALL_ERROR;
    };
    let Some(program) = crate::userspace::load_elf_with_argv(&image[..image_length], &argv[..argc])
    else {
        crate::audit::record(
            actor,
            crate::audit::Action::ProcessExec,
            length as u64,
            false,
        );
        return SYSCALL_ERROR;
    };
    drop(image);
    IO_COMPLETIONS.fetch_or(IO_EXEC, Ordering::Release);
    crate::audit::record(
        actor,
        crate::audit::Action::ProcessExec,
        length as u64,
        true,
    );
    crate::task::exec_current(program)
}

/// Spawn a simple command as a new userspace process without requiring the
/// userspace shell to fork first. This is the reliable fast path for ordinary
/// foreground/background commands; fork remains available for pipelines and
/// redirections that need inherited descriptor wiring.
fn sys_spawn_command(user_line: u64, length: u64) -> u64 {
    let actor = crate::task::current_process_id();
    if !crate::task::current_has(crate::capability::Capability::FileRead)
        || !crate::task::current_has(crate::capability::Capability::ProcessCreate)
    {
        return SYSCALL_ERROR;
    }
    let Ok(length) = usize::try_from(length) else {
        return SYSCALL_ERROR;
    };
    if length == 0 || length > crate::userspace::MAX_ARGV_BYTES {
        return SYSCALL_ERROR;
    }
    let mut command = [0_u8; crate::userspace::MAX_ARGV_BYTES];
    if crate::paging::copy_from_current_user(user_line, &mut command[..length]).is_err()
        || !command[..length].is_ascii()
    {
        return SYSCALL_ERROR;
    }

    let mut ranges = [(0_usize, 0_usize); crate::userspace::MAX_ARGV];
    let mut argc = 0_usize;
    let mut cursor = 0_usize;
    while cursor < length {
        while cursor < length && command[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor == length {
            break;
        }
        if argc == crate::userspace::MAX_ARGV {
            return SYSCALL_ERROR;
        }
        let start = cursor;
        while cursor < length && !command[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        ranges[argc] = (start, cursor);
        argc += 1;
    }
    if argc == 0 {
        return SYSCALL_ERROR;
    }
    let Ok(command_name) = core::str::from_utf8(&command[ranges[0].0..ranges[0].1]) else {
        return SYSCALL_ERROR;
    };
    let resolved = if command_name.as_bytes().contains(&b'/') {
        alloc::string::String::from(command_name)
    } else {
        alloc::format!("/bin/{command_name}")
    };
    if resolved.len() > MAX_PATH_SIZE {
        return SYSCALL_ERROR;
    }
    let mut argv = [""; crate::userspace::MAX_ARGV];
    argv[0] = resolved.as_str();
    for index in 1..argc {
        let (start, end) = ranges[index];
        let Ok(argument) = core::str::from_utf8(&command[start..end]) else {
            return SYSCALL_ERROR;
        };
        argv[index] = argument;
    }
    let mut image = alloc::vec![0_u8; crate::vfs::NODE_CAPACITY];
    let Ok(image_length) = crate::vfs::read_all(resolved.as_str(), &mut image) else {
        crate::audit::record(
            actor,
            crate::audit::Action::ProcessExec,
            length as u64,
            false,
        );
        return SYSCALL_ERROR;
    };
    let Some(program) = crate::userspace::load_elf_with_argv(&image[..image_length], &argv[..argc])
    else {
        crate::audit::record(
            actor,
            crate::audit::Action::ProcessExec,
            length as u64,
            false,
        );
        return SYSCALL_ERROR;
    };
    drop(image);
    match crate::task::spawn_user_process("cmd", program) {
        Ok((pid, _)) => {
            crate::audit::record(
                actor,
                crate::audit::Action::ProcessExec,
                length as u64,
                true,
            );
            pid.as_u64()
        }
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_env_get(user_key: u64, key_len: u64, user_out: u64) -> u64 {
    let Ok(key_len) = usize::try_from(key_len) else {
        return SYSCALL_ERROR;
    };
    if key_len == 0 || key_len >= crate::task::MAX_ENV_ENTRY {
        return SYSCALL_ERROR;
    }
    let mut key = [0u8; crate::task::MAX_ENV_ENTRY];
    if crate::paging::copy_from_current_user(user_key, &mut key[..key_len]).is_err() {
        return SYSCALL_ERROR;
    }
    let mut value = [0u8; crate::task::MAX_ENV_ENTRY];
    let Ok(len) = crate::task::env_get(&key[..key_len], &mut value) else {
        return SYSCALL_ERROR;
    };
    if crate::paging::copy_to_current_user(user_out, &value[..len]).is_err() {
        return SYSCALL_ERROR;
    }
    len as u64
}

fn sys_env_set(user_key: u64, packed_lengths: u64, user_value: u64) -> u64 {
    let key_len = (packed_lengths & 0xffff_ffff) as usize;
    let value_len = (packed_lengths >> 32) as usize;
    if key_len == 0
        || key_len >= crate::task::MAX_ENV_ENTRY
        || value_len >= crate::task::MAX_ENV_ENTRY
    {
        return SYSCALL_ERROR;
    }
    let mut key = [0u8; crate::task::MAX_ENV_ENTRY];
    let mut value = [0u8; crate::task::MAX_ENV_ENTRY];
    if crate::paging::copy_from_current_user(user_key, &mut key[..key_len]).is_err()
        || crate::paging::copy_from_current_user(user_value, &mut value[..value_len]).is_err()
    {
        return SYSCALL_ERROR;
    }
    crate::task::env_set(&key[..key_len], &value[..value_len]).map_or(SYSCALL_ERROR, |()| 0)
}

fn sys_env_entry(index: u64, user_out: u64, capacity: u64) -> u64 {
    let Ok(index) = usize::try_from(index) else {
        return SYSCALL_ERROR;
    };
    let Ok(capacity) = usize::try_from(capacity) else {
        return SYSCALL_ERROR;
    };
    let capacity = core::cmp::min(capacity, crate::task::MAX_ENV_ENTRY);
    if capacity == 0 {
        return SYSCALL_ERROR;
    }
    let mut entry = [0u8; crate::task::MAX_ENV_ENTRY];
    let Ok(len) = crate::task::env_entry(index, &mut entry[..capacity]) else {
        return SYSCALL_ERROR;
    };
    if crate::paging::copy_to_current_user(user_out, &entry[..len]).is_err() {
        return SYSCALL_ERROR;
    }
    len as u64
}

fn sys_process_info(index: u64, user_out: u64) -> u64 {
    let Ok(index) = usize::try_from(index) else {
        return SYSCALL_ERROR;
    };
    let Some(info) = crate::task::process_info(index) else {
        return SYSCALL_ERROR;
    };
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&info as *const crate::task::ProcessInfo).cast::<u8>(),
            core::mem::size_of::<crate::task::ProcessInfo>(),
        )
    };
    if crate::paging::copy_to_current_user(user_out, bytes).is_err() {
        return SYSCALL_ERROR;
    }
    0
}

fn sys_socket(kind: u64) -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network) {
        return SYSCALL_ERROR;
    }
    let kind = match kind {
        1 => crate::network::SocketKind::Udp,
        2 => crate::network::SocketKind::Tcp,
        _ => return SYSCALL_ERROR,
    };
    crate::network::socket_open(crate::task::current_process_id(), kind).unwrap_or(SYSCALL_ERROR)
}

fn sys_net_bind(socket: u64, port: u64) -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network) {
        return SYSCALL_ERROR;
    }
    let Ok(port) = u16::try_from(port) else {
        return SYSCALL_ERROR;
    };
    crate::network::socket_bind(crate::task::current_process_id(), socket, port)
        .map_or(SYSCALL_ERROR, |()| 0)
}

fn sys_net_connect(socket: u64, packed: u64) -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network) {
        return SYSCALL_ERROR;
    }
    let Ok(endpoint) = crate::network::endpoint_from_packed(packed) else {
        return SYSCALL_ERROR;
    };
    crate::network::socket_connect(crate::task::current_process_id(), socket, endpoint)
        .map_or(SYSCALL_ERROR, |()| 0)
}

fn sys_net_send(socket: u64, user_buffer: u64, length: u64) -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network) {
        return SYSCALL_ERROR;
    }
    let Ok(length) = usize::try_from(length) else {
        return SYSCALL_ERROR;
    };
    if length > MAX_IO_SIZE {
        return SYSCALL_ERROR;
    }
    let mut buffer = [0u8; MAX_IO_SIZE];
    if crate::paging::copy_from_current_user(user_buffer, &mut buffer[..length]).is_err() {
        return SYSCALL_ERROR;
    }
    crate::network::socket_send(crate::task::current_process_id(), socket, &buffer[..length])
        .map_or(SYSCALL_ERROR, |n| n as u64)
}

fn sys_net_recv(socket: u64, user_buffer: u64, capacity: u64) -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network) {
        return SYSCALL_ERROR;
    }
    let Ok(capacity) = usize::try_from(capacity) else {
        return SYSCALL_ERROR;
    };
    if capacity > MAX_IO_SIZE {
        return SYSCALL_ERROR;
    }
    let mut buffer = [0u8; MAX_IO_SIZE];
    match crate::network::socket_recv(
        crate::task::current_process_id(),
        socket,
        &mut buffer[..capacity],
    ) {
        Ok((n, _)) => {
            if crate::paging::copy_to_current_user(user_buffer, &buffer[..n]).is_err() {
                SYSCALL_ERROR
            } else {
                n as u64
            }
        }
        Err(crate::network::SocketError::WouldBlock) => u64::MAX - 1,
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_net_close(socket: u64) -> u64 {
    crate::network::socket_close(crate::task::current_process_id(), socket)
        .map_or(SYSCALL_ERROR, |()| 0)
}

fn sys_net_info(user_buffer: u64) -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network) {
        return SYSCALL_ERROR;
    }
    let info = crate::network::net_info();
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&info as *const crate::network::NetInfo).cast::<u8>(),
            core::mem::size_of::<crate::network::NetInfo>(),
        )
    };
    if crate::paging::copy_to_current_user(user_buffer, bytes).is_err() {
        SYSCALL_ERROR
    } else {
        bytes.len() as u64
    }
}

fn sys_dns_start(user_name: u64, length: u64) -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network) {
        return SYSCALL_ERROR;
    }
    let Ok(length) = usize::try_from(length) else {
        return SYSCALL_ERROR;
    };
    if length == 0 || length > 253 {
        return SYSCALL_ERROR;
    }
    let mut name = [0u8; 253];
    if crate::paging::copy_from_current_user(user_name, &mut name[..length]).is_err() {
        return SYSCALL_ERROR;
    }
    let Ok(name) = core::str::from_utf8(&name[..length]) else {
        return SYSCALL_ERROR;
    };
    crate::network::dns_start(name).unwrap_or(SYSCALL_ERROR)
}

fn sys_dns_poll(query: u64, user_ipv4: u64) -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network) {
        return SYSCALL_ERROR;
    }
    match crate::network::dns_poll(query) {
        Ok(None) => 0,
        Ok(Some(ip)) => {
            let octets = ip.octets();
            if crate::paging::copy_to_current_user(user_ipv4, &octets).is_err() {
                SYSCALL_ERROR
            } else {
                1
            }
        }
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_net_peer(socket: u64) -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network) {
        return SYSCALL_ERROR;
    }
    match crate::network::socket_peer(crate::task::current_process_id(), socket) {
        Ok(Some(endpoint)) => crate::network::endpoint_to_packed(endpoint),
        Ok(None) => 0,
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_dhcp(enabled: u64) -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network) {
        return SYSCALL_ERROR;
    }
    crate::network::set_dhcp(enabled != 0).map_or(SYSCALL_ERROR, |()| 0)
}

fn sys_ping_start(ip_packed: u64) -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network) {
        return SYSCALL_ERROR;
    }
    let raw = (ip_packed & 0xffff_ffff) as u32;
    let ip = smoltcp::wire::Ipv4Address::from_octets(raw.to_be_bytes());
    crate::network::ping_start(ip).map_or(SYSCALL_ERROR, |()| 0)
}

fn sys_ping_poll() -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network) {
        return SYSCALL_ERROR;
    }
    match crate::network::ping_poll() {
        Ok(None) => 0,
        Ok(Some(ticks)) => ticks.saturating_add(1),
        Err(_) => SYSCALL_ERROR,
    }
}


fn authorize_async_class(class: crate::async_op::AsyncClass) -> bool {
    match class {
        crate::async_op::AsyncClass::Block => {
            crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Storage)
        }
        crate::async_op::AsyncClass::Network => {
            crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network)
        }
        crate::async_op::AsyncClass::Device => {
            crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Generic)
        }
        crate::async_op::AsyncClass::Service => {
            crate::task::current_has(crate::capability::Capability::Ipc)
        }
        // File operations are created only through the dedicated Stage 10.5
        // submission syscalls, which authorize the exact descriptor/scope.
        crate::async_op::AsyncClass::File => false,
        crate::async_op::AsyncClass::Timer => crate::task::current_has(crate::capability::Capability::TimerRead),
        crate::async_op::AsyncClass::Event => crate::task::current_has(crate::capability::Capability::Ipc),
    }
}

fn async_handle(raw: u64) -> Result<crate::async_op::Handle, ()> {
    crate::async_op::Handle::from_raw(raw).ok_or(())
}

fn copy_async_completion_to_user(
    user_completion: u64,
    completion: crate::async_op::Completion,
) -> Result<(), ()> {
    let mut bytes = [0u8; 16];
    bytes[0..4].copy_from_slice(&completion.status.to_ne_bytes());
    bytes[4..8].copy_from_slice(&completion.reserved.to_ne_bytes());
    bytes[8..16].copy_from_slice(&completion.value.to_ne_bytes());
    crate::paging::copy_to_current_user(user_completion, &bytes).map_err(|_| ())
}

fn sys_async_create(class: u64) -> u64 {
    let Some(class) = crate::async_op::AsyncClass::from_u64(class) else {
        return SYSCALL_ERROR;
    };
    // Ring 3 may directly manufacture only Service operations. Hardware-backed
    // classes receive handles from their dedicated submission syscalls.
    if !matches!(class, crate::async_op::AsyncClass::Service | crate::async_op::AsyncClass::Timer | crate::async_op::AsyncClass::Event)
        || !authorize_async_class(class) {
        return SYSCALL_ERROR;
    }
    match crate::async_op::allocate_current(class) {
        Ok(handle) => {
            ASYNC_ABI_COMPLETIONS.fetch_or(ASYNC_CREATE, Ordering::Release);
            handle.to_raw()
        }
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_async_poll(raw_handle: u64, user_completion: u64) -> u64 {
    let Ok(handle) = async_handle(raw_handle) else {
        return SYSCALL_ERROR;
    };
    let Ok(class) = crate::async_op::class_current(handle) else {
        return SYSCALL_ERROR;
    };
    if !matches!(class, crate::async_op::AsyncClass::Service | crate::async_op::AsyncClass::Timer | crate::async_op::AsyncClass::Event)
        || !authorize_async_class(class) {
        return SYSCALL_ERROR;
    }
    match crate::async_op::peek_current(handle) {
        Ok(None) => {
            ASYNC_ABI_COMPLETIONS.fetch_or(ASYNC_POLL, Ordering::Release);
            0
        }
        Ok(Some(completion)) => {
            if copy_async_completion_to_user(user_completion, completion).is_err() {
                return SYSCALL_ERROR;
            }
            if crate::async_op::release_current(handle).is_err() {
                return SYSCALL_ERROR;
            }
            ASYNC_ABI_COMPLETIONS.fetch_or(ASYNC_POLL, Ordering::Release);
            1
        }
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_async_wait(raw_handle: u64, user_completion: u64) -> u64 {
    let Ok(handle) = async_handle(raw_handle) else {
        return SYSCALL_ERROR;
    };
    let Ok(class) = crate::async_op::class_current(handle) else {
        return SYSCALL_ERROR;
    };
    if !matches!(
        class,
        crate::async_op::AsyncClass::Service
            | crate::async_op::AsyncClass::Timer
            | crate::async_op::AsyncClass::Event
    ) || !authorize_async_class(class) {
        return SYSCALL_ERROR;
    }
    let Ok(completion) = crate::async_op::wait_ready(handle) else {
        return SYSCALL_ERROR;
    };
    if copy_async_completion_to_user(user_completion, completion).is_err() {
        return SYSCALL_ERROR;
    }
    if crate::async_op::release_current(handle).is_err() {
        return SYSCALL_ERROR;
    }
    ASYNC_ABI_COMPLETIONS.fetch_or(ASYNC_WAIT, Ordering::Release);
    1
}

fn sys_timer_create(relative: u64, period: u64) -> u64 {
    if !crate::task::current_has(crate::capability::Capability::TimerRead) { return SYSCALL_ERROR; }
    let deadline = crate::timer::ticks().checked_add(relative).filter(|&value| value != u64::MAX);
    deadline.and_then(|value| crate::async_events::create_timer(crate::task::current_process_id(), value, period).ok()).unwrap_or(SYSCALL_ERROR)
}

fn sys_timer_wait(raw: u64) -> u64 {
    if !crate::task::current_has(crate::capability::Capability::TimerRead) { return SYSCALL_ERROR; }
    crate::async_events::wait_current(raw, true).map_or(SYSCALL_ERROR, |handle| handle.to_raw())
}

fn sys_event_create(initial: u64) -> u64 {
    if !crate::task::current_has(crate::capability::Capability::Ipc) { return SYSCALL_ERROR; }
    crate::async_events::create_event(crate::task::current_process_id(), initial != 0).unwrap_or(SYSCALL_ERROR)
}

fn sys_event_set(raw: u64, state: u64) -> u64 {
    if !crate::task::current_has(crate::capability::Capability::Ipc) { return SYSCALL_ERROR; }
    crate::async_events::set_event(crate::task::current_process_id(), raw, state != 0).map_or(SYSCALL_ERROR, |()| 0)
}

fn sys_event_wait(raw: u64) -> u64 {
    if !crate::task::current_has(crate::capability::Capability::Ipc) { return SYSCALL_ERROR; }
    crate::async_events::wait_current(raw, false).map_or(SYSCALL_ERROR, |handle| handle.to_raw())
}

fn sys_timer_close(raw: u64) -> u64 {
    crate::async_events::close(crate::task::current_process_id(), raw, true).map_or(SYSCALL_ERROR, |()| 0)
}

fn sys_event_close(raw: u64) -> u64 {
    crate::async_events::close(crate::task::current_process_id(), raw, false).map_or(SYSCALL_ERROR, |()| 0)
}

fn sys_sleep_until(deadline: u64) -> u64 {
    if !crate::task::current_has(crate::capability::Capability::TimerRead) { return SYSCALL_ERROR; }
    crate::task::wait_for_event_until(deadline);
    0
}

fn sys_notification_poll(user_buffer: u64) -> u64 {
    if !crate::task::current_has(crate::capability::Capability::Ipc) { return SYSCALL_ERROR; }
    let Some(note) = crate::notifications::receive() else { return 0; };
    let mut bytes = [0u8; 24];
    bytes[0..8].copy_from_slice(&(note.kind as u64).to_le_bytes());
    bytes[8..16].copy_from_slice(&note.source.to_le_bytes());
    bytes[16..24].copy_from_slice(&note.payload.to_le_bytes());
    if crate::paging::copy_to_current_user(user_buffer, &bytes).is_err() { return SYSCALL_ERROR; }
    1
}

fn sys_async_cancel(raw_handle: u64) -> u64 {
    let Ok(handle) = async_handle(raw_handle) else {
        return SYSCALL_ERROR;
    };
    let Ok(class) = crate::async_op::class_current(handle) else {
        return SYSCALL_ERROR;
    };
    // Cancellation is always permitted to the owner. Policy tightening must
    // never prevent safe teardown of authority that was valid when created.
    // Block requests additionally detach their kernel bounce-buffer queue slot.
    if class == crate::async_op::AsyncClass::Block {
        let _ = crate::block_io::cancel_user_request(handle);
    } else if class == crate::async_op::AsyncClass::File {
        let _ = crate::async_file::cancel(handle);
    } else if class == crate::async_op::AsyncClass::Network {
        let _ = crate::async_network::cancel(handle);
    } else if matches!(class, crate::async_op::AsyncClass::Timer | crate::async_op::AsyncClass::Event) {
        crate::async_events::cancel(handle);
    }
    match crate::async_op::cancel_current(handle) {
        Ok(()) => {
            ASYNC_ABI_COMPLETIONS.fetch_or(ASYNC_CANCEL, Ordering::Release);
            0
        }
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_async_complete(raw_handle: u64, status: u64, value: u64) -> u64 {
    let Ok(handle) = async_handle(raw_handle) else {
        return SYSCALL_ERROR;
    };
    if crate::async_op::class_current(handle) != Ok(crate::async_op::AsyncClass::Service)
        || !crate::task::current_has(crate::capability::Capability::Ipc)
    {
        return SYSCALL_ERROR;
    }
    match crate::async_op::complete_current_service(
        handle,
        crate::async_op::Completion::new(status as i32, value),
    ) {
        Ok(()) => {
            ASYNC_ABI_COMPLETIONS.fetch_or(ASYNC_COMPLETE, Ordering::Release);
            0
        }
        Err(_) => SYSCALL_ERROR,
    }
}

fn copy_block_completion_to_user(
    user_result: u64,
    completion: crate::async_op::Completion,
    result: crate::block_io::UserResult,
) -> Result<(), ()> {
    // ABI: 16-byte generic Completion header followed by one 512-byte sector.
    // For writes and failed reads the payload is zeroed so kernel bounce-buffer
    // contents are never exposed accidentally.
    let mut bytes = [0u8; 16 + crate::block::SECTOR_SIZE];
    bytes[0..4].copy_from_slice(&completion.status.to_ne_bytes());
    bytes[4..8].copy_from_slice(&completion.reserved.to_ne_bytes());
    bytes[8..16].copy_from_slice(&completion.value.to_ne_bytes());
    if result.is_read && result.result.is_ok() {
        bytes[16..].copy_from_slice(&result.data);
    }
    crate::paging::copy_to_current_user(user_result, &bytes).map_err(|_| ())
}

fn sys_async_block_read(lba: u64) -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Storage) {
        return SYSCALL_ERROR;
    }
    match crate::block_io::submit_user_read(lba) {
        Ok(handle) => handle.to_raw(),
        Err(_) => SYSCALL_ERROR,
    }
}

fn sys_async_block_write(lba: u64, user_sector: u64) -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Storage) {
        return SYSCALL_ERROR;
    }
    let mut data = [0u8; crate::block::SECTOR_SIZE];
    if crate::paging::copy_from_current_user(user_sector, &mut data).is_err() {
        return SYSCALL_ERROR;
    }
    match crate::block_io::submit_user_write(lba, data) {
        Ok(handle) => handle.to_raw(),
        Err(_) => SYSCALL_ERROR,
    }
}

fn finish_async_block(handle: crate::async_op::Handle, user_result: u64, wait: bool) -> u64 {
    if crate::async_op::class_current(handle) != Ok(crate::async_op::AsyncClass::Block)
        || !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Storage)
    {
        return SYSCALL_ERROR;
    }
    let completion = if wait {
        match crate::async_op::wait_ready(handle) {
            Ok(completion) => completion,
            Err(_) => return SYSCALL_ERROR,
        }
    } else {
        match crate::async_op::peek_current(handle) {
            Ok(Some(completion)) => completion,
            Ok(None) => return 0,
            Err(_) => return SYSCALL_ERROR,
        }
    };
    let Some(result) = crate::block_io::peek_user_result(handle) else {
        return SYSCALL_ERROR;
    };
    if copy_block_completion_to_user(user_result, completion, result).is_err() {
        return SYSCALL_ERROR;
    }
    if !crate::block_io::consume_user_result(handle)
        || crate::async_op::release_current(handle).is_err()
    {
        return SYSCALL_ERROR;
    }
    1
}

fn sys_async_block_poll(raw_handle: u64, user_result: u64) -> u64 {
    let Ok(handle) = async_handle(raw_handle) else {
        return SYSCALL_ERROR;
    };
    finish_async_block(handle, user_result, false)
}

fn sys_async_block_wait(raw_handle: u64, user_result: u64) -> u64 {
    let Ok(handle) = async_handle(raw_handle) else {
        return SYSCALL_ERROR;
    };
    finish_async_block(handle, user_result, true)
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AsyncFileRequest { offset: u64, buffer: u64, length: u64 }

fn copy_async_file_request(user_request: u64) -> Result<AsyncFileRequest, ()> {
    let mut bytes=[0u8; 24];
    crate::paging::copy_from_current_user(user_request, &mut bytes).map_err(|_| ())?;
    Ok(AsyncFileRequest {
        offset: u64::from_ne_bytes(bytes[0..8].try_into().map_err(|_| ())?),
        buffer: u64::from_ne_bytes(bytes[8..16].try_into().map_err(|_| ())?),
        length: u64::from_ne_bytes(bytes[16..24].try_into().map_err(|_| ())?),
    })
}
fn sys_async_file_read(descriptor: u64, user_request: u64) -> u64 {
    let Ok(request)=copy_async_file_request(user_request) else { return SYSCALL_ERROR; };
    let Ok(offset)=usize::try_from(request.offset) else { return SYSCALL_ERROR; };
    let Ok(length)=usize::try_from(request.length) else { return SYSCALL_ERROR; };
    if length > MAX_IO_SIZE { return SYSCALL_ERROR; }
    crate::async_file::submit_read(descriptor, offset, length).map_or(SYSCALL_ERROR, |h| h.to_raw())
}
fn sys_async_file_write(descriptor: u64, user_request: u64) -> u64 {
    let Ok(request)=copy_async_file_request(user_request) else { return SYSCALL_ERROR; };
    let Ok(offset)=usize::try_from(request.offset) else { return SYSCALL_ERROR; };
    let Ok(length)=usize::try_from(request.length) else { return SYSCALL_ERROR; };
    if length > MAX_IO_SIZE { return SYSCALL_ERROR; }
    let mut data=[0u8; MAX_IO_SIZE];
    if crate::paging::copy_from_current_user(request.buffer, &mut data[..length]).is_err() { return SYSCALL_ERROR; }
    crate::async_file::submit_write(descriptor, offset, &data[..length]).map_or(SYSCALL_ERROR, |h| h.to_raw())
}
fn copy_file_completion(user_completion: u64, completion: crate::async_op::Completion) -> Result<(), ()> {
    let mut bytes=[0u8;16];
    bytes[0..4].copy_from_slice(&completion.status.to_ne_bytes());
    bytes[4..8].copy_from_slice(&completion.reserved.to_ne_bytes());
    bytes[8..16].copy_from_slice(&completion.value.to_ne_bytes());
    crate::paging::copy_to_current_user(user_completion, &bytes).map_err(|_| ())
}
fn finish_async_file(handle: crate::async_op::Handle, user_completion: u64, user_data: u64, wait: bool) -> u64 {
    if crate::async_op::class_current(handle) != Ok(crate::async_op::AsyncClass::File) { return SYSCALL_ERROR; }
    let completion=if wait {
        match crate::async_op::wait_ready(handle) { Ok(c)=>c, Err(_)=>return SYSCALL_ERROR }
    } else {
        match crate::async_op::peek_current(handle) { Ok(Some(c))=>c, Ok(None)=>return 0, Err(_)=>return SYSCALL_ERROR }
    };
    let Some(result)=crate::async_file::peek_result(handle) else { return SYSCALL_ERROR; };
    let write=result.operation == crate::async_file::Operation::Write;
    if !crate::task::authorize_current_file_scope(result.scope, write) { return SYSCALL_ERROR; }
    if copy_file_completion(user_completion, completion).is_err() { return SYSCALL_ERROR; }
    if !write && result.result.is_ok() {
        let count=result.result.unwrap_or(0).min(result.length);
        if crate::paging::copy_to_current_user(user_data, &result.data[..count]).is_err() { return SYSCALL_ERROR; }
    }
    if !crate::async_file::consume(handle) || crate::async_op::release_current(handle).is_err() { return SYSCALL_ERROR; }
    1
}
fn sys_async_file_poll(raw_handle: u64, user_completion: u64, user_data: u64) -> u64 {
    let Ok(handle)=async_handle(raw_handle) else { return SYSCALL_ERROR; };
    finish_async_file(handle, user_completion, user_data, false)
}
fn sys_async_file_wait(raw_handle: u64, user_completion: u64, user_data: u64) -> u64 {
    let Ok(handle)=async_handle(raw_handle) else { return SYSCALL_ERROR; };
    finish_async_file(handle, user_completion, user_data, true)
}


fn sys_port_read(port: u64, destination: u64, argument: u64, wait: bool) -> u64 {
    let (capacity, deadline) = if wait {
        let mut options = [0u8; 16];
        if crate::paging::copy_from_current_user(argument, &mut options).is_err() { return SYSCALL_ERROR; }
        let timeout = u64::from_le_bytes(options[8..].try_into().unwrap());
        let deadline = if timeout == u64::MAX { u64::MAX } else {
            let Some(deadline) = crate::timer::ticks().checked_add(timeout).filter(|&d| d != u64::MAX) else { return SYSCALL_ERROR; };
            deadline
        };
        (u64::from_le_bytes(options[..8].try_into().unwrap()), deadline)
    } else { (argument, 0) };
    if capacity == 0 || capacity > crate::completion_queue::BATCH as u64 { return SYSCALL_ERROR; }
    let owner = crate::task::current_process_id();
    let Some(task) = crate::task::current_task_id_if_running() else { return SYSCALL_ERROR; };
    loop {
        let result = crate::completion_port::begin(owner, port, task, capacity as usize, wait);
        let Ok((events, count)) = result else { return SYSCALL_ERROR; };
        if count != 0 {
            let mut bytes = [0u8; crate::completion_queue::BATCH * 40];
            for (event, out) in events[..count].iter().zip(bytes.as_chunks_mut::<40>().0.iter_mut()) { event.encode(out); }
            let copied = crate::paging::copy_to_current_user(destination, &bytes[..count * 40]).is_ok();
            let acknowledged = crate::completion_port::finish(owner, port, task, &events[..count], copied).is_ok();
            return if copied && acknowledged { count as u64 } else { SYSCALL_ERROR };
        }
        if !wait || (deadline != u64::MAX && crate::timer::ticks() >= deadline) {
            crate::completion_port::unwatch(port, task);
            return 0;
        }
        crate::task::wait_for_event_until(deadline);
        crate::completion_port::unwatch(port, task);
    }
}

fn sys_async_tcp_connect(socket: u64, packed: u64) -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network) { return SYSCALL_ERROR; }
    let Ok(endpoint) = crate::network::endpoint_from_packed(packed) else { return SYSCALL_ERROR; };
    crate::async_network::submit_connect(socket, endpoint).map_or(SYSCALL_ERROR, |h| h.to_raw())
}

fn sys_async_net_send(socket: u64, user_buffer: u64, length: u64) -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network) { return SYSCALL_ERROR; }
    let Ok(length)=usize::try_from(length) else { return SYSCALL_ERROR; };
    if length > MAX_IO_SIZE { return SYSCALL_ERROR; }
    let mut data=[0u8; MAX_IO_SIZE];
    if crate::paging::copy_from_current_user(user_buffer, &mut data[..length]).is_err() { return SYSCALL_ERROR; }
    crate::async_network::submit_send(socket, &data[..length]).map_or(SYSCALL_ERROR, |h| h.to_raw())
}

fn sys_async_net_recv(socket: u64, capacity: u64) -> u64 {
    if !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network) { return SYSCALL_ERROR; }
    let Ok(capacity)=usize::try_from(capacity) else { return SYSCALL_ERROR; };
    if capacity > MAX_IO_SIZE { return SYSCALL_ERROR; }
    crate::async_network::submit_recv(socket, capacity).map_or(SYSCALL_ERROR, |h| h.to_raw())
}

fn finish_async_net(handle: crate::async_op::Handle, user_completion: u64, user_data: u64, wait: bool) -> u64 {
    if crate::async_op::class_current(handle) != Ok(crate::async_op::AsyncClass::Network)
        || !crate::task::authorize_current_device(crate::wovenguard::DeviceClass::Network)
    { return SYSCALL_ERROR; }
    let completion=if wait {
        match crate::async_op::wait_ready(handle) { Ok(c)=>c, Err(_)=>return SYSCALL_ERROR }
    } else {
        match crate::async_op::peek_current(handle) { Ok(Some(c))=>c, Ok(None)=>return 0, Err(_)=>return SYSCALL_ERROR }
    };
    let Some(result)=crate::async_network::peek_result(handle) else { return SYSCALL_ERROR; };
    if copy_file_completion(user_completion, completion).is_err() { return SYSCALL_ERROR; }
    if result.operation == crate::async_network::Operation::Recv && result.result.is_ok() {
        let count=result.result.unwrap_or(0).min(result.length);
        if crate::paging::copy_to_current_user(user_data, &result.data[..count]).is_err() { return SYSCALL_ERROR; }
    }
    if !crate::async_network::consume(handle) || crate::async_op::release_current(handle).is_err() { return SYSCALL_ERROR; }
    1
}

fn sys_async_net_poll(raw_handle: u64, user_completion: u64, user_data: u64) -> u64 {
    let Ok(handle)=async_handle(raw_handle) else { return SYSCALL_ERROR; };
    finish_async_net(handle, user_completion, user_data, false)
}
fn sys_async_net_wait(raw_handle: u64, user_completion: u64, user_data: u64) -> u64 {
    let Ok(handle)=async_handle(raw_handle) else { return SYSCALL_ERROR; };
    finish_async_net(handle, user_completion, user_data, true)
}

#[unsafe(no_mangle)]
pub extern "C" fn wovenhat_syscall_dispatch(
    number: u64,
    arg0: u64,
    arg1: u64,
    arg2: u64,
    frame: *const UserForkFrame,
) -> u64 {
    let result = match number {
        value if value == Number::Mmap as u64 => sys_mmap(arg0, arg1),
        value if value == Number::Munmap as u64 => sys_munmap(arg0, arg1),
        value if value == Number::Read as u64 => sys_read(arg0, arg1, arg2),
        value if value == Number::FileWrite as u64 => sys_file_write(arg0, arg1, arg2),
        value if value == Number::Write as u64 => sys_write(arg0, arg1, arg2),
        value if value == Number::Open as u64 => sys_open(arg0, arg1),
        value if value == Number::Close as u64 => sys_close(arg0),
        value if value == Number::Exec as u64 => sys_exec(arg0, arg1),
        value if value == Number::ExecCommand as u64 => sys_exec_command(arg0, arg1),
        value if value == Number::EnvGet as u64 => sys_env_get(arg0, arg1, arg2),
        value if value == Number::EnvSet as u64 => sys_env_set(arg0, arg1, arg2),
        value if value == Number::EnvCount as u64 => crate::task::env_count() as u64,
        value if value == Number::EnvEntry as u64 => sys_env_entry(arg0, arg1, arg2),
        value if value == Number::ProcessCount as u64 => crate::task::process_count() as u64,
        value if value == Number::ProcessInfo as u64 => sys_process_info(arg0, arg1),
        value if value == Number::SpawnCommand as u64 => sys_spawn_command(arg0, arg1),
        value if value == Number::MmapFileShared as u64 => {
            crate::task::mmap_file_current(arg0, arg1, arg2, true, true, true)
                .unwrap_or(SYSCALL_ERROR)
        }
        value if value == Number::Msync as u64 => crate::task::msync_current(arg0, arg1)
            .map(|_| 0)
            .unwrap_or(SYSCALL_ERROR),
        value if value == Number::MmapFileLazy as u64 => {
            crate::task::mmap_file_current(arg0, arg1, arg2, false, true, false)
                .unwrap_or(SYSCALL_ERROR)
        }
        value if value == Number::MmapFileLazyWritable as u64 => {
            crate::task::mmap_file_current(arg0, arg1, arg2, true, true, false)
                .unwrap_or(SYSCALL_ERROR)
        }
        value if value == Number::MmapFile as u64 => {
            crate::task::mmap_file_current(arg0, arg1, arg2, false, false, false)
                .unwrap_or(SYSCALL_ERROR)
        }
        value if value == Number::MmapFileWritable as u64 => {
            crate::task::mmap_file_current(arg0, arg1, arg2, true, false, false)
                .unwrap_or(SYSCALL_ERROR)
        }
        value if value == Number::Fork as u64 => sys_fork(frame),
        value if value == Number::MessageSend as u64 => sys_message_send(arg0, arg1, arg2),
        value if value == Number::MessageReceive as u64 => sys_message_receive(arg0, arg1, arg2),
        value if value == Number::Getuid as u64 => {
            IO_COMPLETIONS.fetch_or(IO_GETUID, Ordering::Release);
            u64::from(crate::task::current_credentials().uid)
        }
        value if value == Number::Getgid as u64 => {
            IO_COMPLETIONS.fetch_or(IO_GETGID, Ordering::Release);
            u64::from(crate::task::current_credentials().gid)
        }
        value if value == Number::Yield as u64 => {
            // Yield immediately from the syscall frame so a userspace
            // fork/wait loop does not depend on a later timer interrupt to
            // schedule another Ready process. The syscall frame resumes here
            // when this task is selected again, then returns to Ring 3.
            crate::task::yield_now();
            0
        }
        value if value == Number::Exit as u64 => crate::task::exit_current_process(arg0 as i32),
        value if value == Number::Getpid as u64 => crate::task::current_process_id(),
        value if value == Number::Waitpid as u64 => match crate::task::wait_process(arg0) {
            Ok(exit_code) => exit_code as u64,
            Err(crate::task::WaitError::StillRunning) => u64::MAX - 1,
            Err(crate::task::WaitError::NoSuchChild) => u64::MAX,
        },
        value if value == Number::Stat as u64 => sys_stat(arg0, arg1),
        value if value == Number::Readdir as u64 => sys_readdir(arg0, arg1, arg2),
        value if value == Number::Mkdir as u64 => sys_mkdir(arg0, arg1),
        value if value == Number::Chdir as u64 => sys_chdir(arg0, arg1),
        value if value == Number::Getcwd as u64 => sys_getcwd(arg0, arg1),
        value if value == Number::Dup as u64 => sys_dup(arg0),
        value if value == Number::Pipe as u64 => sys_pipe(),
        value if value == Number::Dup2 as u64 => sys_dup2(arg0, arg1),
        value if value == Number::Getppid as u64 => sys_getppid(),
        value if value == Number::Kill as u64 => sys_kill(arg0, arg1),
        value if value == Number::Lseek as u64 => sys_lseek(arg0, arg1),
        value if value == Number::Unlink as u64 => sys_unlink(arg0, arg1),
        value if value == Number::Sleep as u64 => sys_sleep(arg0),
        value if value == Number::Rename as u64 => sys_rename(arg0, arg1, arg2),
        value if value == Number::Getticks as u64 => sys_getticks(),
        value if value == Number::Sync as u64 => sys_sync(),
        value if value == Number::Ioctl as u64 => sys_ioctl(arg0, arg1, arg2),
        value if value == Number::Sigaction as u64 => sys_sigaction(arg0, arg1),
        value if value == Number::Getpgrp as u64 => crate::task::current_process_group(),
        value if value == Number::Setpgid as u64 => sys_setpgid(arg0, arg1),
        value if value == Number::Socket as u64 => sys_socket(arg0),
        value if value == Number::Bind as u64 => sys_net_bind(arg0, arg1),
        value if value == Number::Connect as u64 => sys_net_connect(arg0, arg1),
        value if value == Number::NetSend as u64 => sys_net_send(arg0, arg1, arg2),
        value if value == Number::NetRecv as u64 => sys_net_recv(arg0, arg1, arg2),
        value if value == Number::NetClose as u64 => sys_net_close(arg0),
        value if value == Number::NetInfo as u64 => sys_net_info(arg0),
        value if value == Number::DnsStart as u64 => sys_dns_start(arg0, arg1),
        value if value == Number::DnsPoll as u64 => sys_dns_poll(arg0, arg1),
        value if value == Number::NetPeer as u64 => sys_net_peer(arg0),
        value if value == Number::Dhcp as u64 => sys_dhcp(arg0),
        value if value == Number::PingStart as u64 => sys_ping_start(arg0),
        value if value == Number::PingPoll as u64 => sys_ping_poll(),
        value if value == Number::AsyncCreate as u64 => sys_async_create(arg0),
        value if value == Number::AsyncPoll as u64 => sys_async_poll(arg0, arg1),
        value if value == Number::AsyncWait as u64 => sys_async_wait(arg0, arg1),
        value if value == Number::NotificationPoll as u64 => sys_notification_poll(arg0),
        value if value == Number::AsyncCancel as u64 => sys_async_cancel(arg0),
        value if value == Number::AsyncComplete as u64 => sys_async_complete(arg0, arg1, arg2),
        value if value == Number::AsyncBlockRead as u64 => sys_async_block_read(arg0),
        value if value == Number::AsyncBlockWrite as u64 => sys_async_block_write(arg0, arg1),
        value if value == Number::AsyncBlockPoll as u64 => sys_async_block_poll(arg0, arg1),
        value if value == Number::AsyncBlockWait as u64 => sys_async_block_wait(arg0, arg1),
        value if value == Number::AsyncFileRead as u64 => sys_async_file_read(arg0, arg1),
        value if value == Number::AsyncFileWrite as u64 => sys_async_file_write(arg0, arg1),
        value if value == Number::AsyncFilePoll as u64 => sys_async_file_poll(arg0, arg1, arg2),
        value if value == Number::AsyncFileWait as u64 => sys_async_file_wait(arg0, arg1, arg2),
        value if value == Number::AsyncNetSend as u64 => sys_async_net_send(arg0, arg1, arg2),
        value if value == Number::AsyncNetRecv as u64 => sys_async_net_recv(arg0, arg1),
        value if value == Number::AsyncNetPoll as u64 => sys_async_net_poll(arg0, arg1, arg2),
        value if value == Number::AsyncNetWait as u64 => sys_async_net_wait(arg0, arg1, arg2),
        value if value == Number::AsyncTcpConnect as u64 => sys_async_tcp_connect(arg0, arg1),
        value if value == Number::CompletionPortCreate as u64 => crate::completion_port::create(crate::task::current_process_id()).unwrap_or(SYSCALL_ERROR),
        value if value == Number::CompletionPortClose as u64 => crate::completion_port::close(crate::task::current_process_id(), arg0).map_or(SYSCALL_ERROR, |()| 0),
        value if value == Number::CompletionPortAssociate as u64 => async_handle(arg0).ok()
            .and_then(|handle| crate::async_op::associate_current(handle, arg1, arg2).ok()).map_or(SYSCALL_ERROR, |()| 0),
        value if value == Number::CompletionPortPoll as u64 => sys_port_read(arg0, arg1, arg2, false),
        value if value == Number::CompletionPortWait as u64 => sys_port_read(arg0, arg1, arg2, true),
        value if value == Number::TimerCreate as u64 => sys_timer_create(arg0, arg1),
        value if value == Number::TimerWait as u64 => sys_timer_wait(arg0),
        value if value == Number::EventCreate as u64 => sys_event_create(arg0),
        value if value == Number::EventSet as u64 => sys_event_set(arg0, arg1),
        value if value == Number::EventWait as u64 => sys_event_wait(arg0),
        value if value == Number::TimerClose as u64 => sys_timer_close(arg0),
        value if value == Number::EventClose as u64 => sys_event_close(arg0),
        value if value == Number::SleepUntil as u64 => sys_sleep_until(arg0),
        _ => u64::MAX,
    };

    // Scheduler-owned termination requests must be acknowledged before the
    // syscall returns to ring 3. This primarily covers self-termination; a
    // remotely running task is retired after its next physical hand-off.
    crate::task::honor_current_termination_request();
    prepare_signal_delivery(frame);
    LAST_COMPLETED.store(number, Ordering::Release);
    COMPLETIONS.fetch_add(1, Ordering::Release);
    result
}

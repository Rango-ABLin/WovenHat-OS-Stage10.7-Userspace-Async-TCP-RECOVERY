use core::{
    arch::global_asm,
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use crate::{
    capability::{Capability, CapabilitySet},
    config::{MAX_FILE_DESCRIPTORS, MAX_PAGER_REQUESTS, MAX_PROCESSES, MAX_TASKS, TASK_STACK_SIZE},
    gdt, ipc, irq_lock::{IrqMutex, IrqMutexGuard}, paging, timer, userspace, vfs, wovenguard,
};

const KERNEL_TASK_ID: TaskId = TaskId(0);
const IDLE_TASK_ID: TaskId = TaskId(1);

static SCHEDULER: IrqMutex<Scheduler> = IrqMutex::with_rank(Scheduler::empty(), 10);
static PROCESS_TABLE: IrqMutex<[Option<Process>; MAX_PROCESSES]> =
    IrqMutex::with_rank([const { None }; MAX_PROCESSES], 20);

/// Interrupt-safe guard for the global process table.
///
/// A normal spin::Mutex is SMP-safe, but it is not sufficient when the same
/// CPU can be preempted while holding the lock: the newly scheduled task may
/// try to acquire PROCESS_TABLE and spin forever waiting for the task that was
/// just preempted.  Keep local interrupts disabled for the full lifetime of
/// every process-table guard so a holder cannot be involuntarily switched out
/// on its own CPU. Other CPUs can still run and release the same SMP lock.
struct ProcessTableGuard {
    guard: IrqMutexGuard<'static, [Option<Process>; MAX_PROCESSES]>,
}

impl core::ops::Deref for ProcessTableGuard {
    type Target = [Option<Process>; MAX_PROCESSES];

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl core::ops::DerefMut for ProcessTableGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

fn process_table_lock() -> ProcessTableGuard {
    ProcessTableGuard { guard: PROCESS_TABLE.lock() }
}
static IDLE_HEARTBEATS: AtomicU64 = AtomicU64::new(0);
static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(2);
static NEXT_PROCESS_ID: AtomicU64 = AtomicU64::new(1);
static PREEMPTION_REQUESTED: [AtomicBool; crate::smp::MAX_CPUS] =
    [const { AtomicBool::new(false) }; crate::smp::MAX_CPUS];
static PREEMPTION_SWITCHES: AtomicU64 = AtomicU64::new(0);
static REBALANCE_PASSES: AtomicU64 = AtomicU64::new(0);
static REBALANCE_MIGRATIONS: AtomicU64 = AtomicU64::new(0);
static USERSPACE_REBALANCE_MIGRATIONS: AtomicU64 = AtomicU64::new(0);
static REBALANCE_SKIPS: AtomicU64 = AtomicU64::new(0);
static FORK_AFFINITY_INHERITANCES: AtomicU64 = AtomicU64::new(0);
static LAST_REBALANCE_TICK: AtomicU64 = AtomicU64::new(0);
static TASK_STACKS: [TaskStack; MAX_TASKS] = [const { TaskStack::new() }; MAX_TASKS];
// Stage 9.4D SMP sandbox-lifecycle closure state. Each online CPU runs one
// restricted kernel worker. The coordinator tightens that worker's TCB-bound
// sandbox while it is live, then broadens the profile again to prove stripped
// authority cannot resurrect. All synchronization is allocation-free.
static STAGE9_4D_PHASE: AtomicU64 = AtomicU64::new(0);
static STAGE9_4D_READY: AtomicU64 = AtomicU64::new(0);
static STAGE9_4D_PHASE1_OK: AtomicU64 = AtomicU64::new(0);
static STAGE9_4D_PHASE2_OK: AtomicU64 = AtomicU64::new(0);
static STAGE9_4D_DONE: AtomicU64 = AtomicU64::new(0);
static STAGE9_4D_CPU_MASK: AtomicU64 = AtomicU64::new(0);
static STAGE9_4D_FAIL: [AtomicU64; crate::smp::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::smp::MAX_CPUS];

// The pager queue is touched from exception, scheduler, and worker paths.
// Keep its bounded critical sections at the scheduler rank so page-fault
// producers cannot be preempted while publishing a request.
static PAGER: IrqMutex<PagerQueue> = IrqMutex::with_rank(PagerQueue::empty(), 10);
static PAGER_TASK: IrqMutex<Option<TaskId>> = IrqMutex::with_rank(None, 10);
static PAGER_REQUESTS: AtomicU64 = AtomicU64::new(0);
static PAGER_COMPLETIONS: AtomicU64 = AtomicU64::new(0);
// Stage 7.1.4: narrow scheduler tracing enabled only around the pager acceptance loop.
static YIELD_TRACE_ENABLED: AtomicBool = AtomicBool::new(false);
static YIELD_TRACE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

global_asm!(
    ".global wovenhat_context_switch",
    "wovenhat_context_switch:",
    // RFLAGS is part of a schedulable kernel context. In particular, IF must
    // not leak from the task being switched out into the task being resumed.
    // Every caller enters the low-level hand-off with interrupts masked so GDT,
    // CR3 and scheduler metadata remain atomic; an already-started task then
    // receives the exact flags it saved at its own previous hand-off.
    "pushfq",
    "push rbx",
    "push rbp",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov [rdi], rsp",
    "mov rsp, rsi",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    "popfq",
    "ret",
);

global_asm!(
    ".global wovenhat_fork_first_dispatch",
    "wovenhat_fork_first_dispatch:",
    // A fork child does not enter through `task_bootstrap`: its kernel stack
    // already contains a saved syscall/iret frame. Complete the scheduler
    // hand-off exactly once, then continue through the normal syscall resume
    // trampoline with RSP still pointing at that saved frame.
    "call wovenhat_finalize_scheduler_handoff",
    "jmp wovenhat_syscall_resume",
);

global_asm!(
    ".global wovenhat_enter_user_mode",
    "wovenhat_enter_user_mode:",
    "cli",
    "movzx ecx, cx",
    "movzx edx, dx",
    "push rcx",
    "push rsi",
    "pushfq",
    "or qword ptr [rsp], 0x200",
    "push rdx",
    "push rdi",
    "iretq",
);

global_asm!(
    ".global wovenhat_user_stub",
    "wovenhat_user_stub:",
    "mov rax, 3",
    "int 0x80",
    "2:",
    "jmp 2b",
);

unsafe extern "C" {
    fn wovenhat_context_switch(previous_rsp: *mut u64, next_rsp: u64);
    fn wovenhat_fork_first_dispatch();
    fn wovenhat_enter_user_mode(entry: u64, stack_top: u64, user_cs: u16, user_ss: u16) -> !;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TaskId(u64);

impl TaskId {
    pub const fn from_u64(value: u64) -> Self {
        Self(value)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ProcessId(u64);

impl ProcessId {
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Credentials {
    pub uid: u32,
    pub gid: u32,
}

impl Credentials {
    pub const ROOT: Self = Self { uid: 0, gid: 0 };
    pub const USERSPACE: Self = Self {
        uid: 1000,
        gid: 1000,
    };

    pub const fn is_root(self) -> bool {
        self.uid == 0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Ready,
    Exited,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskPriority(u8);

impl TaskPriority {
    pub const LOW: Self = Self(0);
    pub const NORMAL: Self = Self(1);
    pub const HIGH: Self = Self(2);

    pub const fn as_u8(self) -> u8 {
        self.0
    }

    const fn index(self) -> usize {
        self.0 as usize
    }

    const fn quantum(self) -> u8 {
        match self {
            Self::LOW => 1,
            Self::NORMAL => 2,
            Self::HIGH => 4,
            _ => 1,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskState {
    Empty,
    Ready,
    Running,
    /// The task has been deselected in scheduler metadata but its CPU has not
    /// yet completed the physical stack switch. It must not be scheduled or
    /// migrated until the incoming context finalizes the hand-off.
    Switching,
    Blocked,
    Sleeping,
    Dead,
}

impl TaskState {
    const fn name(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Switching => "switching",
            Self::Blocked => "blocked",
            Self::Sleeping => "sleeping",
            Self::Dead => "dead",
        }
    }
}

#[derive(Clone, Copy)]
struct Context {
    stack_pointer: u64,
}

#[repr(align(16))]
struct TaskStack(UnsafeCell<[u8; TASK_STACK_SIZE]>);

// SAFETY: Each stack belongs to exactly one task. A running task is owned by
// one CPU; Stage-6 migration changes CPU ownership only while the task is Ready
// and while the global scheduler lock is held, so stack/context access never
// overlaps across CPUs.
unsafe impl Sync for TaskStack {}

impl TaskStack {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; TASK_STACK_SIZE]))
    }
}

struct TaskControlBlock {
    id: TaskId,
    name: &'static str,
    state: TaskState,
    priority: TaskPriority,
    context: Context,
    capabilities: CapabilitySet,
    /// Optional WovenGuard provenance for each currently delegated capability.
    /// `None` means intrinsic/bootstrap authority; `Some` means the bit is
    /// effective only while the generation-tagged lineage remains live.
    capability_lineages: [Option<wovenguard::LineageId>; Capability::COUNT],
    security_domain: wovenguard::SecurityDomain,
    sandbox_profile: wovenguard::SandboxProfile,
    entry: Option<fn() -> !>,
    wake_tick: u64,
    user_context: Option<UserTaskContext>,
    address_space: Option<paging::AddressSpace>,
    remaining_ticks: u8,
    cpu: usize,
    migratable: bool,
    /// True once the scheduler has selected this task for execution at least once.
    ///
    /// This is execution-history metadata, not a general migration permission.
    /// Stage 7.6's *automatic* Ring-3 balancer uses it to stay within the
    /// pre-first-dispatch migration boundary proven by Stage 7.4. Explicit
    /// Stage 7.4 Ready migration/affinity operations keep their independently
    /// validated Ready + migratable + affinity contract.
    ever_dispatched: bool,
    /// CPUs on which this task is permitted to execute. Stage 7.1 treats
    /// this as a hard scheduler invariant, not a placement hint.
    affinity_mask: usize,
    /// Non-zero when a terminating signal has been requested. Running or
    /// Switching tasks acknowledge this only at a scheduler-owned safe point;
    /// tasks that are not physically executing may be retired immediately.
    termination_signal: u8,
    /// Latched scheduler event used by Stage 8.3 wait/wake handshakes.
    /// A signal delivered before the task actually blocks is remembered here
    /// and consumed by `wait_for_event`, closing the classic lost-wakeup window.
    event_pending: bool,
    event_deadline: Option<u64>,
}

#[derive(Clone, Copy)]
struct ForkSecurityContext {
    capabilities: CapabilitySet,
    capability_lineages: [Option<wovenguard::LineageId>; Capability::COUNT],
    security_domain: wovenguard::SecurityDomain,
    sandbox_profile: wovenguard::SandboxProfile,
}

impl TaskControlBlock {
    const fn empty() -> Self {
        Self {
            id: TaskId(u64::MAX),
            name: "",
            state: TaskState::Empty,
            priority: TaskPriority::NORMAL,
            context: Context { stack_pointer: 0 },
            capabilities: CapabilitySet::empty(),
            capability_lineages: [None; Capability::COUNT],
            security_domain: wovenguard::SecurityDomain::Restricted,
            sandbox_profile: wovenguard::default_sandbox(wovenguard::SecurityDomain::Restricted),
            entry: None,
            wake_tick: 0,
            user_context: None,
            address_space: None,
            remaining_ticks: 0,
            cpu: 0,
            migratable: false,
            ever_dispatched: false,
            affinity_mask: 1,
            termination_signal: 0,
            event_pending: false,
            event_deadline: None,
        }
    }

    fn initialize(
        &mut self,
        slot: usize,
        id: TaskId,
        name: &'static str,
        entry: fn() -> !,
        priority: TaskPriority,
    ) {
        self.id = id;
        self.name = name;
        self.state = TaskState::Ready;
        self.priority = priority;
        self.entry = Some(entry);
        self.wake_tick = 0;
        self.capabilities = CapabilitySet::empty();
        self.capability_lineages = [None; Capability::COUNT];
        self.user_context = None;
        self.address_space = paging::kernel_address_space();
        self.remaining_ticks = priority.quantum();
        self.migratable = false;
        self.ever_dispatched = false;
        self.affinity_mask = 1;
        self.termination_signal = 0;
        self.event_pending = false;
        self.event_deadline = None;
        self.security_domain = wovenguard::SecurityDomain::SystemService;
        self.sandbox_profile = wovenguard::default_sandbox(self.security_domain);

        let stack_start = TASK_STACKS[slot].0.get().cast::<u8>() as usize;
        let stack_top = stack_start + TASK_STACK_SIZE;
        let mut cursor = (stack_top & !0xf) - 8;

        // Build the stack expected by `wovenhat_context_switch`: six saved
        // callee-saved registers, a saved RFLAGS word, then the entry address
        // consumed by `ret`. Fresh tasks start with IF clear; task_bootstrap
        // completes the scheduler hand-off before deliberately enabling IRQs.
        // The reserved eight bytes preserve the SysV entry alignment.
        unsafe {
            push_stack_value(&mut cursor, (task_bootstrap as fn() -> !) as usize as u64);
            push_stack_value(&mut cursor, 0x2);
            for _ in 0..6 {
                push_stack_value(&mut cursor, 0);
            }
        }

        self.context.stack_pointer = cursor as u64;
    }

    fn initialize_user(
        &mut self,
        slot: usize,
        id: TaskId,
        name: &'static str,
        context: UserTaskContext,
        address_space: paging::AddressSpace,
    ) {
        self.id = id;
        self.name = name;
        self.state = TaskState::Ready;
        self.priority = TaskPriority::NORMAL;
        self.entry = None;
        self.wake_tick = 0;
        self.user_context = Some(context);
        self.address_space = Some(address_space);
        self.remaining_ticks = TaskPriority::NORMAL.quantum();
        self.capabilities = CapabilitySet::userspace();
        self.capability_lineages = [None; Capability::COUNT];
        self.security_domain = wovenguard::SecurityDomain::User;
        self.sandbox_profile = wovenguard::default_sandbox(self.security_domain);
        self.migratable = false;
        self.ever_dispatched = false;
        self.affinity_mask = 1;
        self.termination_signal = 0;
        self.event_pending = false;
        self.event_deadline = None;

        let stack_start = TASK_STACKS[slot].0.get().cast::<u8>() as usize;
        let stack_top = stack_start + TASK_STACK_SIZE;
        let mut cursor = (stack_top & !0xf) - 8;

        // The first kernel-side dispatch enters through the common bootstrap;
        // it then installs the saved user context with iretq. The synthetic
        // scheduler frame starts with IF clear until task_bootstrap finalizes
        // the hand-off.
        unsafe {
            push_stack_value(&mut cursor, (task_bootstrap as fn() -> !) as usize as u64);
            push_stack_value(&mut cursor, 0x2);
            for _ in 0..6 {
                push_stack_value(&mut cursor, 0);
            }
        }

        self.context.stack_pointer = cursor as u64;
    }
    fn initialize_fork(
        &mut self,
        slot: usize,
        id: TaskId,
        frame: crate::syscall::UserForkFrame,
        address_space: paging::AddressSpace,
        security: ForkSecurityContext,
    ) {
        self.id = id;
        self.name = "fork-child";
        self.state = TaskState::Ready;
        self.priority = TaskPriority::NORMAL;
        self.entry = None;
        self.wake_tick = 0;
        self.user_context = None;
        self.address_space = Some(address_space);
        self.remaining_ticks = TaskPriority::NORMAL.quantum();
        self.capabilities = security.capabilities;
        self.capability_lineages = security.capability_lineages;
        self.security_domain = security.security_domain;
        self.sandbox_profile = security.sandbox_profile;
        self.migratable = false;
        self.ever_dispatched = false;
        self.affinity_mask = 1;
        self.termination_signal = 0;
        self.event_pending = false;
        self.event_deadline = None;

        let stack_start = TASK_STACKS[slot].0.get().cast::<u8>() as usize;
        let stack_top = (stack_start + TASK_STACK_SIZE) & !0xf;
        let frame_start = stack_top - core::mem::size_of::<crate::syscall::UserForkFrame>();
        unsafe {
            (frame_start as *mut crate::syscall::UserForkFrame).write(frame.child_return());
        }
        let mut cursor = frame_start;
        unsafe {
            // A fresh fork child must complete the scheduler hand-off before
            // returning to ring 3. Jumping directly to syscall_resume would
            // leave switching_out[cpu] stale and eventually trigger the
            // "nested scheduler hand-off" invariant.
            push_stack_value(
                &mut cursor,
                wovenhat_fork_first_dispatch as *const () as u64,
            );
            // Fork's first scheduler dispatch must also enter with IF clear;
            // syscall_resume eventually restores the child's Ring-3 RFLAGS.
            push_stack_value(&mut cursor, 0x2);
            for _ in 0..6 {
                push_stack_value(&mut cursor, 0);
            }
        }
        self.context.stack_pointer = cursor as u64;
    }
}

#[no_mangle]
extern "C" fn wovenhat_finalize_scheduler_handoff() {
    // First-dispatch trampolines execute with interrupts disabled. Keep the
    // scheduler transition atomic while publishing the outgoing task Ready.
    SCHEDULER.lock().finalize_switch();
}

fn task_bootstrap() -> ! {
    // A never-before-run task enters here directly rather than returning from
    // `wovenhat_context_switch`, so it must complete the scheduler hand-off.
    let (entry, user_context) = {
        let mut scheduler = SCHEDULER.lock();
        scheduler.finalize_switch();
        let task = &scheduler.tasks[scheduler.current_slot[crate::smp::cpu_index()]];
        (task.entry, task.user_context)
    };

    x86_64::instructions::interrupts::enable();
    if let Some(context) = user_context {
        enter_user_context(context)
    }

    entry.expect("scheduled task is missing its entry point")()
}

// Stage 7: bounded per-process environment. Entries are stored as KEY=VALUE
// ASCII byte strings so fork inheritance is allocation-free and deterministic.
pub const MAX_ENV_VARS: usize = 8;
pub const MAX_ENV_ENTRY: usize = 96;

#[derive(Clone, Copy)]
struct ProcessEnvironment {
    entries: [[u8; MAX_ENV_ENTRY]; MAX_ENV_VARS],
    lengths: [u8; MAX_ENV_VARS],
}

impl ProcessEnvironment {
    const fn empty() -> Self {
        Self {
            entries: [[0; MAX_ENV_ENTRY]; MAX_ENV_VARS],
            lengths: [0; MAX_ENV_VARS],
        }
    }

    fn defaults() -> Self {
        let mut env = Self::empty();
        env.set_bytes(b"PATH", b"/bin");
        env.set_bytes(b"HOME", b"/");
        env.set_bytes(b"SHELL", b"/bin/sh");
        env.set_bytes(b"TERM", b"wovenhat");
        env
    }

    fn set_bytes(&mut self, key: &[u8], value: &[u8]) -> bool {
        if key.is_empty() || key.contains(&b'=') || !key.is_ascii() || !value.is_ascii() {
            return false;
        }
        let Some(total) = key
            .len()
            .checked_add(value.len())
            .and_then(|n| n.checked_add(1))
        else {
            return false;
        };
        if total > MAX_ENV_ENTRY {
            return false;
        }
        let existing = (0..MAX_ENV_VARS).find(|&i| {
            let len = self.lengths[i] as usize;
            len > key.len()
                && &self.entries[i][..key.len()] == key
                && self.entries[i][key.len()] == b'='
        });
        let slot = existing.or_else(|| (0..MAX_ENV_VARS).find(|&i| self.lengths[i] == 0));
        let Some(slot) = slot else {
            return false;
        };
        self.entries[slot] = [0; MAX_ENV_ENTRY];
        self.entries[slot][..key.len()].copy_from_slice(key);
        self.entries[slot][key.len()] = b'=';
        self.entries[slot][key.len() + 1..total].copy_from_slice(value);
        self.lengths[slot] = total as u8;
        true
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EnvError {
    NoProcess,
    Invalid,
    Full,
    NotFound,
    BufferTooSmall,
}

/// What a process file descriptor refers to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FdKind {
    File {
        id: vfs::OpenFileId,
        scope: wovenguard::FileScope,
    },
    PipeRead(usize),
    PipeWrite(usize),
}

#[derive(Clone, Copy)]
pub struct Process {
    pub id: ProcessId,
    pub task_id: TaskId,
    pub state: ProcessState,
    pub parent: ProcessId,
    pub credentials: Credentials,
    pub exit_code: i32,
    address_space: Option<userspace::AddressSpace>,
    /// Per-process file-descriptor table. Entries are handles into the
    /// system-wide refcounted open-file table in `vfs`.
    files: [Option<FdKind>; MAX_FILE_DESCRIPTORS],
    memory_mappings: [Option<userspace::AnonymousMapping>; userspace::MAX_ANONYMOUS_MAPPINGS],
    memory_mapping_scopes: [Option<wovenguard::FileScope>; userspace::MAX_ANONYMOUS_MAPPINGS],
    cwd: [u8; crate::config::MAX_PATH_SIZE],
    cwd_len: usize,
    pending_signal: u64,
    process_group: u64,
    signal_actions: [u64; 32],
    environment: ProcessEnvironment,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProcessError {
    Full,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WaitError {
    NoSuchChild,
    StillRunning,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileError {
    PermissionDenied,
    NoProcess,
    NotFound,
    TooManyFiles,
    BadDescriptor,
    AlreadyExists,
    NotEmpty,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    PermissionDenied,
    BadDescriptor,
    NoProcess,
    InvalidLength,
    Full,
    NotFound,
    MappingFailed,
}

pub struct Summary {
    pub task_count: usize,
    pub ready_tasks: usize,
    pub blocked_tasks: usize,
    pub current_id: TaskId,
    pub current_name: &'static str,
    pub current_state: &'static str,
    pub current_priority: u8,
    pub context_switches: u64,
    pub preemption_switches: u64,
    pub idle_heartbeats: u64,
}

/// Scheduler-visible load for one logical CPU.
///
/// `runnable` counts non-idle tasks that are either Ready or Running. Keeping
/// Ready and Running separate makes diagnostics useful while giving placement
/// code one stable total that does not oscillate when a timer switches a task
/// between those two states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuRunLoad {
    pub ready: usize,
    pub running: usize,
}

impl CpuRunLoad {
    pub const fn runnable(self) -> usize {
        self.ready + self.running
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RebalanceStats {
    pub passes: u64,
    pub migrations: u64,
    pub skipped: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CapabilityError {
    PermissionDenied,
    UnknownTask,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationError {
    UnknownTask,
    OfflineCpu,
    AffinityDenied,
    Pinned,
    NotReady,
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AffinityError {
    UnknownTask,
    InvalidMask,
    OfflineCpu,
    Pinned,
    NotReady,
}

const fn cpu_bit(cpu: usize) -> usize {
    1usize << cpu
}

fn online_affinity_mask() -> usize {
    crate::smp::online_mask()
}

struct ContextSwitch {
    next_privilege_stack: u64,
    previous_rsp: *mut u64,
    next_rsp: u64,
    next_address_space: paging::AddressSpace,
}
#[derive(Clone, Copy)]
struct PagerRequest {
    task: TaskId,
    space: userspace::AddressSpace,
    slot: usize,
    mapping: userspace::AnonymousMapping,
    address: u64,
    write: bool,
}

#[derive(Clone, Copy)]
struct PagerCompletion {
    task: TaskId,
    address: u64,
    succeeded: bool,
}

struct PagerQueue {
    entries: [Option<PagerRequest>; MAX_PAGER_REQUESTS],
    completions: [Option<PagerCompletion>; MAX_PAGER_REQUESTS],
}

impl PagerQueue {
    const fn empty() -> Self {
        Self {
            entries: [None; MAX_PAGER_REQUESTS],
            completions: [None; MAX_PAGER_REQUESTS],
        }
    }

    fn push(&mut self, request: PagerRequest) -> bool {
        let Some(slot) = self.entries.iter_mut().find(|entry| entry.is_none()) else {
            return false;
        };
        *slot = Some(request);
        true
    }

    fn pop(&mut self) -> Option<PagerRequest> {
        self.entries.iter_mut().find_map(Option::take)
    }

    fn finish(&mut self, completion: PagerCompletion) -> bool {
        let Some(slot) = self.completions.iter_mut().find(|entry| entry.is_none()) else {
            return false;
        };
        *slot = Some(completion);
        true
    }

    fn take_completion(&mut self, task: TaskId, address: u64) -> Option<bool> {
        let slot = self.completions.iter().position(|entry| {
            entry.is_some_and(|completion| completion.task == task && completion.address == address)
        })?;
        self.completions[slot]
            .take()
            .map(|completion| completion.succeeded)
    }
}
struct Scheduler {
    tasks: [TaskControlBlock; MAX_TASKS],
    current_slot: [usize; crate::smp::MAX_CPUS],
    task_count: usize,
    context_switches: u64,
    last_selected: [[usize; 3]; crate::smp::MAX_CPUS],
    /// Slot whose CPU->CPU hand-off is physically in progress. `MAX_TASKS`
    /// means no outgoing runnable task is awaiting finalization.
    switching_out: [usize; crate::smp::MAX_CPUS],
}

impl Scheduler {
    const fn empty() -> Self {
        Self {
            tasks: [const { TaskControlBlock::empty() }; MAX_TASKS],
            current_slot: [0; crate::smp::MAX_CPUS],
            task_count: 0,
            context_switches: 0,
            last_selected: [[0; 3]; crate::smp::MAX_CPUS],
            switching_out: [MAX_TASKS; crate::smp::MAX_CPUS],
        }
    }

    fn initialize(&mut self) {
        assert!(self.task_count == 0, "scheduler initialized more than once");

        self.tasks[0].id = KERNEL_TASK_ID;
        self.tasks[0].name = "kernel";
        self.tasks[0].state = TaskState::Running;
        self.tasks[0].priority = TaskPriority::HIGH;
        self.tasks[0].capabilities = CapabilitySet::kernel_bootstrap();
        self.tasks[0].capability_lineages = [None; Capability::COUNT];
        self.tasks[0].security_domain = wovenguard::SecurityDomain::Kernel;
        self.tasks[0].sandbox_profile = wovenguard::default_sandbox(self.tasks[0].security_domain);
        self.tasks[0].address_space = paging::kernel_address_space();
        self.tasks[0].remaining_ticks = TaskPriority::HIGH.quantum();
        self.tasks[0].cpu = 0;
        self.tasks[0].migratable = false;
        self.tasks[0].ever_dispatched = true;
        self.tasks[0].affinity_mask = cpu_bit(0);
        self.tasks[1].initialize(1, IDLE_TASK_ID, "idle", idle_task, TaskPriority::LOW);
        self.tasks[1].security_domain = wovenguard::SecurityDomain::Restricted;
        self.tasks[1].sandbox_profile = wovenguard::default_sandbox(self.tasks[1].security_domain);
        self.tasks[1].cpu = 0;
        self.tasks[1].affinity_mask = cpu_bit(0);
        self.task_count = 2;
    }

    fn validate_affinity_invariants(&self) {
        let online = online_affinity_mask();
        for task in &self.tasks {
            if matches!(task.state, TaskState::Empty | TaskState::Dead) {
                continue;
            }
            assert!(task.affinity_mask != 0, "live task has empty CPU affinity");
            assert_eq!(
                task.affinity_mask & !online,
                0,
                "live task affinity contains an offline CPU"
            );
            assert!(
                crate::smp::cpu_is_online(task.cpu),
                "live task owner CPU is offline"
            );
            assert!(
                task.affinity_mask & cpu_bit(task.cpu) != 0,
                "task owner CPU is outside its affinity mask"
            );
            if task.entry.is_none() && !task.migratable {
                // Ordinary userspace remains CPU0-owned and Stage 7.3 probes
                // remain hard-pinned. Stage 7.4 adds one narrow class of
                // migratable Ring-3 probe; those tasks use the same general
                // owner-within-affinity invariant checked above.
                assert_eq!(
                    task.affinity_mask,
                    cpu_bit(task.cpu),
                    "pinned userspace affinity must match its owner CPU"
                );
            }
        }
    }

    fn prepare_switch(&mut self) -> Option<ContextSwitch> {
        self.reap_dead();
        self.validate_affinity_invariants();

        let best_priority = self
            .tasks
            .iter()
            .filter(|task| task.state == TaskState::Ready
                && task.cpu == crate::smp::cpu_index()
                && (task.affinity_mask & cpu_bit(crate::smp::cpu_index())) != 0)
            .map(|task| task.priority)
            .max()?;
        let priority_index = best_priority.index();
        let start = self.last_selected[crate::smp::cpu_index()][priority_index];
        let next_slot = (1..=MAX_TASKS)
            .map(|offset| (start + offset) % MAX_TASKS)
            .find(|slot| {
                let task = &self.tasks[*slot];
                task.state == TaskState::Ready
                    && task.priority == best_priority
                    && task.cpu == crate::smp::cpu_index()
                    && (task.affinity_mask & cpu_bit(crate::smp::cpu_index())) != 0
            })?;
        // Resolve every fallible property of the incoming task before mutating
        // scheduler ownership/state. A failed address-space lookup after marking
        // the old task Switching would leave metadata committed without a
        // physical stack switch.
        let next_address_space = self.tasks[next_slot]
            .address_space
            .expect("ready task is missing an address space");

        self.last_selected[crate::smp::cpu_index()][priority_index] = next_slot;

        let cpu = crate::smp::cpu_index();
        let previous_slot = self.current_slot[cpu];
        if self.tasks[previous_slot].state == TaskState::Running {
            // Do NOT publish the outgoing task as Ready yet. The Rust guard is
            // dropped before the assembly stack switch, so another CPU could
            // otherwise migrate or run this task while it is still physically
            // executing on `cpu`. The incoming context finalizes this state.
            assert_eq!(
                self.switching_out[cpu],
                MAX_TASKS,
                "nested scheduler hand-off on one CPU"
            );
            self.tasks[previous_slot].state = TaskState::Switching;
            self.switching_out[cpu] = previous_slot;
        }
        // Publish execution history under the same scheduler lock as the
        // Ready -> Running transition. The Stage 7.6 automatic balancer can
        // therefore never mistake a task that has already been dispatched for
        // a fresh pre-dispatch Ring-3 placement candidate.
        self.tasks[next_slot].ever_dispatched = true;
        self.tasks[next_slot].state = TaskState::Running;
        self.tasks[next_slot].remaining_ticks = self.tasks[next_slot].priority.quantum();
        self.current_slot[crate::smp::cpu_index()] = next_slot;
        self.context_switches += 1;

        let previous_rsp = &mut self.tasks[previous_slot].context.stack_pointer as *mut u64;
        let next_rsp = self.tasks[next_slot].context.stack_pointer;
        Some(ContextSwitch {
            next_privilege_stack: (TASK_STACKS[next_slot].0.get().cast::<u8>() as u64
                + TASK_STACK_SIZE as u64)
                & !0xf,
            previous_rsp,
            next_rsp,
            next_address_space,
        })
    }
    fn finalize_switch(&mut self) {
        let cpu = crate::smp::cpu_index();
        let outgoing = self.switching_out[cpu];
        if outgoing == MAX_TASKS {
            return;
        }
        assert!(
            outgoing != self.current_slot[cpu],
            "outgoing scheduler slot is still current"
        );
        assert!(
            self.tasks[outgoing].state == TaskState::Switching,
            "outgoing task lost switching state"
        );

        let signal = self.tasks[outgoing].termination_signal;
        if signal != 0 {
            let task_id = self.tasks[outgoing].id;
            self.tasks[outgoing].state = TaskState::Dead;
            self.tasks[outgoing].name = "terminated";
            self.tasks[outgoing].termination_signal = 0;
            self.task_count = self.task_count.saturating_sub(1);
            complete_process_termination(task_id, signal);
        } else {
            self.tasks[outgoing].state = TaskState::Ready;
        }
        self.switching_out[cpu] = MAX_TASKS;
    }

    /// Request scheduler-owned termination of one task. The scheduler may
    /// retire a task immediately only when it is not physically executing. A
    /// Running/Switching task carries the request until its next completed
    /// hand-off, at which point `finalize_switch` performs the retirement.
    fn request_termination(&mut self, id: TaskId, signal: u8) -> Option<usize> {
        let slot = self.tasks.iter().position(|task| {
            task.id == id && !matches!(task.state, TaskState::Empty | TaskState::Dead)
        })?;
        let cpu = self.tasks[slot].cpu;
        self.tasks[slot].termination_signal = signal;

        match self.tasks[slot].state {
            TaskState::Ready | TaskState::Blocked | TaskState::Sleeping => {
                self.tasks[slot].state = TaskState::Dead;
                self.tasks[slot].name = "terminated";
                self.tasks[slot].termination_signal = 0;
                self.task_count = self.task_count.saturating_sub(1);
                complete_process_termination(id, signal);
            }
            TaskState::Running | TaskState::Switching => {
                // Physical execution still owns this task. The request is
                // acknowledged only after the CPU has switched away.
            }
            TaskState::Empty | TaskState::Dead => unreachable!(),
        }
        Some(cpu)
    }

    fn reap_dead(&mut self) {
        for (slot, task) in self.tasks.iter_mut().enumerate() {
            if slot != self.current_slot[crate::smp::cpu_index()]
                && task.state == TaskState::Dead
                && task.cpu == crate::smp::cpu_index()
            {
                // Stage 9.2C: a dead task must not leave live delegation roots
                // or children behind in the bounded WovenGuard lineage table.
                // Revoking an owned node also invalidates authority delegated
                // below it before the TCB itself is recycled.
                let _ = wovenguard::revoke_all_owned_lineages(task.id.as_u64());
                *task = TaskControlBlock::empty();
            }
        }
    }

    fn wake_sleeping(&mut self, now: u64) {
        for task in &mut self.tasks {
            if task.state == TaskState::Blocked && task.event_deadline.is_some_and(|deadline| now >= deadline) {
                task.state = TaskState::Ready;
                task.event_deadline = None;
            }
            if task.state == TaskState::Sleeping && task.wake_tick <= now {
                task.state = TaskState::Ready;
                task.wake_tick = 0;
            }
        }
    }

    fn run_loads(&self) -> [CpuRunLoad; crate::smp::MAX_CPUS] {
        let mut loads = [CpuRunLoad { ready: 0, running: 0 }; crate::smp::MAX_CPUS];
        for task in &self.tasks {
            if task.cpu >= crate::smp::MAX_CPUS || matches!(task.name, "idle" | "cpu-idle") {
                continue;
            }
            match task.state {
                TaskState::Ready => loads[task.cpu].ready += 1,
                // `Switching` is a scheduler hand-off state, but the outgoing
                // task is still physically executing until the assembly stack
                // switch completes. Count it as running for placement and
                // rebalance decisions so another CPU never sees an artificially
                // low source load during that short hand-off window.
                TaskState::Running | TaskState::Switching => loads[task.cpu].running += 1,
                _ => {}
            }
        }
        loads
    }

    /// Choose the least-loaded online CPU permitted by `affinity_mask`.
    ///
    /// This is a placement hint only: the scheduler lock makes the snapshot
    /// coherent, but later runnable changes may immediately alter the load.
    fn least_loaded_cpu_for_mask(&self, affinity_mask: usize) -> Option<usize> {
        let online = crate::smp::online_count();
        let loads = self.run_loads();
        let local_domain = crate::smp::cpu_domain(crate::smp::cpu_index());
        (0..online)
            .filter(|cpu| affinity_mask & cpu_bit(*cpu) != 0)
            .min_by_key(|cpu| {
                (
                    usize::from(crate::smp::cpu_domain(*cpu) != local_domain),
                    loads[*cpu].runnable(),
                    *cpu,
                )
            })
    }

    /// Move at most one explicitly migratable Ready task from the busiest CPU to
    /// the least-loaded CPU. A difference of one runnable task is considered
    /// balanced; this avoids pointless ping-pong between equally useful CPUs.
    fn rebalance_one(&mut self) -> Option<(TaskId, usize, usize)> {
        let online = crate::smp::online_count();
        if online < 2 {
            return None;
        }

        let loads = self.run_loads();
        // Evaluate concrete task/target pairs. Affinity is a hard constraint:
        // a low-load CPU that is not in a task's mask is not a destination.
        let mut best: Option<(usize, usize, usize, usize)> = None; // slot, source, target, gain
        for (slot, task) in self.tasks.iter().enumerate() {
            if task.state != TaskState::Ready
                || !task.migratable
                || matches!(task.name, "idle" | "cpu-idle")
                || task.cpu >= online
                // Stage 7.4 proved Ring-3 migration before first dispatch.
                // Do not extend that proof to a process that has already run
                // and only later became Ready again after preemption/wakeup.
                || (task.entry.is_none() && task.ever_dispatched)
            {
                continue;
            }
            let source = task.cpu;
            for target in 0..online {
                if target == source || (task.affinity_mask & cpu_bit(target)) == 0 {
                    continue;
                }
                let source_load = loads[source].runnable();
                let target_load = loads[target].runnable();
                if source_load <= target_load + 1 {
                    continue;
                }
                let gain = source_load - target_load;
                let cross_domain = usize::from(
                    crate::smp::cpu_domain(source) != crate::smp::cpu_domain(target),
                );
                if best.is_none_or(|(_, best_source, best_target, best_gain)| {
                    let best_cross = usize::from(
                        crate::smp::cpu_domain(best_source)
                            != crate::smp::cpu_domain(best_target),
                    );
                    cross_domain < best_cross
                        || (cross_domain == best_cross
                            && (gain > best_gain || (gain == best_gain && target < best_target)))
                }) {
                    best = Some((slot, source, target, gain));
                }
            }
        }

        let (slot, source, target, _) = best?;
        debug_assert!((self.tasks[slot].affinity_mask & cpu_bit(target)) != 0);
        let id = self.tasks[slot].id;
        if self.tasks[slot].entry.is_none() {
            USERSPACE_REBALANCE_MIGRATIONS.fetch_add(1, Ordering::Relaxed);
        }
        self.tasks[slot].cpu = target;
        self.validate_affinity_invariants();
        Some((id, source, target))
    }

    fn summary(&self) -> Summary {
        let task = &self.tasks[self.current_slot[crate::smp::cpu_index()]];
        assert!(task.state == TaskState::Running);

        let ready_tasks = self
            .tasks
            .iter()
            .filter(|task| task.state == TaskState::Ready
                && task.cpu == crate::smp::cpu_index()
                && (task.affinity_mask & cpu_bit(crate::smp::cpu_index())) != 0)
            .count();
        let blocked_tasks = self
            .tasks
            .iter()
            .filter(|task| matches!(task.state, TaskState::Blocked | TaskState::Sleeping))
            .count();

        Summary {
            task_count: self.task_count,
            ready_tasks,
            blocked_tasks,
            current_id: task.id,
            current_name: task.name,
            current_state: task.state.name(),
            current_priority: task.priority.as_u8(),
            context_switches: self.context_switches,
            preemption_switches: PREEMPTION_SWITCHES.load(Ordering::Relaxed),
            idle_heartbeats: IDLE_HEARTBEATS.load(Ordering::Relaxed),
        }
    }
}

unsafe fn switch_stacks(context_switch: ContextSwitch) {
    gdt::set_privilege_stack(context_switch.next_privilege_stack);
    paging::switch_to(context_switch.next_address_space);
    unsafe {
        wovenhat_context_switch(context_switch.previous_rsp, context_switch.next_rsp);
    }

    // We are now executing on the incoming task's stack. Only at this point
    // is it safe to expose the outgoing runnable task as Ready/migratable.
    SCHEDULER.lock().finalize_switch();
}
pub fn init() {
    SCHEDULER.lock().initialize();
}

fn create_process(
    task_id: TaskId,
    parent: ProcessId,
    address_space: userspace::AddressSpace,
) -> Process {
    let mut cwd = [0u8; crate::config::MAX_PATH_SIZE];
    cwd[0] = b'/';
    let mut cwd_len = 1usize;
    let mut inherited_group = None;
    let mut inherited_environment = ProcessEnvironment::defaults();
    {
        let processes = process_table_lock();
        if let Some(parent_proc) = processes.iter().flatten().find(|p| p.id == parent) {
            cwd = parent_proc.cwd;
            cwd_len = parent_proc.cwd_len;
            inherited_group = Some(parent_proc.process_group);
            inherited_environment = parent_proc.environment;
        }
    }
    let id = ProcessId(NEXT_PROCESS_ID.fetch_add(1, Ordering::Relaxed));
    Process {
        id,
        task_id,
        state: ProcessState::Ready,
        parent,
        credentials: Credentials::USERSPACE,
        exit_code: 0,
        address_space: Some(address_space),
        files: [None; MAX_FILE_DESCRIPTORS],
        memory_mappings: [None; userspace::MAX_ANONYMOUS_MAPPINGS],
        memory_mapping_scopes: [None; userspace::MAX_ANONYMOUS_MAPPINGS],
        cwd,
        cwd_len,
        pending_signal: 0,
        process_group: inherited_group.unwrap_or(id.as_u64()),
        signal_actions: [0; 32],
        environment: inherited_environment,
    }
}

/// Clone a parent's file-descriptor table for fork.
/// Each live descriptor bumps the shared open-file refcount so offsets are shared.
fn clone_file_table(
    parent_files: &[Option<FdKind>; MAX_FILE_DESCRIPTORS],
) -> Result<[Option<FdKind>; MAX_FILE_DESCRIPTORS], ProcessError> {
    let mut child_files = [None; MAX_FILE_DESCRIPTORS];
    for (index, entry) in parent_files.iter().enumerate() {
        match entry {
            Some(FdKind::File { id, scope }) => {
                child_files[index] = Some(FdKind::File {
                    id: vfs::clone_open_file(*id).map_err(|_| {
                        release_file_table(&mut child_files);
                        ProcessError::Full
                    })?,
                    scope: *scope,
                });
            }
            Some(FdKind::PipeRead(id)) => {
                crate::pipe::clone_reader(*id).map_err(|_| {
                    release_file_table(&mut child_files);
                    ProcessError::Full
                })?;
                child_files[index] = Some(FdKind::PipeRead(*id));
            }
            Some(FdKind::PipeWrite(id)) => {
                crate::pipe::clone_writer(*id).map_err(|_| {
                    release_file_table(&mut child_files);
                    ProcessError::Full
                })?;
                child_files[index] = Some(FdKind::PipeWrite(*id));
            }
            None => {}
        }
    }
    Ok(child_files)
}

fn release_fd(fd: FdKind) {
    match fd {
        FdKind::File { id, .. } => {
            let _ = vfs::close_open_file(id);
        }
        FdKind::PipeRead(id) => crate::pipe::close_reader(id),
        FdKind::PipeWrite(id) => crate::pipe::close_writer(id),
    }
}

fn release_file_table(files: &mut [Option<FdKind>; MAX_FILE_DESCRIPTORS]) {
    for entry in files.iter_mut() {
        if let Some(fd) = entry.take() {
            release_fd(fd);
        }
    }
}

pub fn spawn_user_process(
    name: &'static str,
    program: userspace::UserProgram,
) -> Result<(ProcessId, UserTaskContext), ProcessError> {
    if !program.image.is_valid() {
        let _ = userspace::destroy(program.address_space);
        return Err(ProcessError::Full);
    }

    let parent = ProcessId(current_process_id());
    let context = prepare_user_context(
        program.image.entry as usize,
        program.image.stack_top as usize,
    );
    let mut scheduler = SCHEDULER.lock();
    scheduler.reap_dead();
    let Some(task_slot) = scheduler
        .tasks
        .iter()
        .position(|task| task.state == TaskState::Empty)
    else {
        let _ = userspace::destroy(program.address_space);
        return Err(ProcessError::Full);
    };

    // Build the Process before taking PROCESS_TABLE. create_process() needs to
    // inspect the parent entry to inherit cwd/process-group state, so calling it
    // while PROCESS_TABLE is already locked would recursively acquire the same
    // non-reentrant spinlock and deadlock during userspace spawn.
    let task_id = TaskId(NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed));
    let process = create_process(task_id, parent, program.address_space);
    let process_id = process.id;

    let mut processes = process_table_lock();
    let Some(process_slot) = processes.iter().position(|entry| entry.is_none()) else {
        let _ = userspace::destroy(program.address_space);
        return Err(ProcessError::Full);
    };
    if ipc::register(process_id.as_u64()).is_err() {
        let _ = userspace::destroy(program.address_space);
        return Err(ProcessError::Full);
    }
    scheduler.tasks[task_slot].initialize_user(
        task_slot,
        task_id,
        name,
        context,
        program.address_space.paging(),
    );
    scheduler.task_count += 1;
    processes[process_slot] = Some(process);
    Ok((process_id, context))
}
/// Spawn a least-privilege Ring-3 system service with an explicit capability set.
///
/// Ordinary applications remain in `SecurityDomain::User` and cannot acquire
/// raw storage/device authority. Stage 10.4 uses this path for the storage ABI
/// acceptance probe, and later service-manager work can reuse the same boundary.
#[cfg(any(feature = "stage10-4-test", feature = "stage10-9-test"))]
pub fn spawn_user_system_service(
    name: &'static str,
    program: userspace::UserProgram,
    capabilities: CapabilitySet,
) -> Result<(ProcessId, TaskId, UserTaskContext), ProcessError> {
    if !program.image.is_valid()
        || !capabilities.is_subset_of(
            wovenguard::domain_ceiling(wovenguard::SecurityDomain::SystemService),
        )
    {
        let _ = userspace::destroy(program.address_space);
        return Err(ProcessError::Full);
    }

    let parent = ProcessId(current_process_id());
    let context = prepare_user_context(
        program.image.entry as usize,
        program.image.stack_top as usize,
    );
    let mut scheduler = SCHEDULER.lock();
    scheduler.reap_dead();
    let Some(task_slot) = scheduler
        .tasks
        .iter()
        .position(|task| task.state == TaskState::Empty)
    else {
        let _ = userspace::destroy(program.address_space);
        return Err(ProcessError::Full);
    };

    let task_id = TaskId(NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed));
    let process = create_process(task_id, parent, program.address_space);
    let process_id = process.id;
    let mut processes = process_table_lock();
    let Some(process_slot) = processes.iter().position(|entry| entry.is_none()) else {
        let _ = userspace::destroy(program.address_space);
        return Err(ProcessError::Full);
    };
    if ipc::register(process_id.as_u64()).is_err() {
        let _ = userspace::destroy(program.address_space);
        return Err(ProcessError::Full);
    }

    scheduler.tasks[task_slot].initialize_user(
        task_slot,
        task_id,
        name,
        context,
        program.address_space.paging(),
    );
    scheduler.tasks[task_slot].security_domain = wovenguard::SecurityDomain::SystemService;
    scheduler.tasks[task_slot].capabilities = capabilities;
    scheduler.tasks[task_slot].capability_lineages = [None; Capability::COUNT];
    scheduler.tasks[task_slot].sandbox_profile = wovenguard::SandboxProfile::new(104, capabilities);
    scheduler.task_count += 1;
    processes[process_slot] = Some(process);
    Ok((process_id, task_id, context))
}

/// Spawn an explicitly SMP-audited userspace process with a hard affinity mask.
///
/// Stage 7.6 keeps legacy `spawn_user_process()` CPU0-owned, but gives audited
/// workloads a general multicore path. The new task is migratable while Ready
/// and initially placed on the least-loaded allowed online CPU. Running and
/// Switching tasks remain protected by the existing scheduler hand-off rules.
#[derive(Clone, Copy)]
pub struct MulticoreSpawnPlacement {
    pub owner_cpu: usize,
    pub affinity_mask: usize,
}

pub fn spawn_multicore_user_process(
    name: &'static str,
    program: userspace::UserProgram,
    affinity_mask: usize,
) -> Result<(ProcessId, TaskId, UserTaskContext, MulticoreSpawnPlacement), ProcessError> {
    let online = online_affinity_mask();
    if !program.image.is_valid()
        || affinity_mask == 0
        || affinity_mask & !online != 0
    {
        let _ = userspace::destroy(program.address_space);
        return Err(ProcessError::Full);
    }

    let parent = ProcessId(current_process_id());
    let context = prepare_user_context(
        program.image.entry as usize,
        program.image.stack_top as usize,
    );

    let result = {
        let mut scheduler = SCHEDULER.lock();
        scheduler.reap_dead();
        let Some(task_slot) = scheduler
            .tasks
            .iter()
            .position(|task| task.state == TaskState::Empty)
        else {
            let _ = userspace::destroy(program.address_space);
            return Err(ProcessError::Full);
        };
        let Some(owner_cpu) = scheduler.least_loaded_cpu_for_mask(affinity_mask) else {
            let _ = userspace::destroy(program.address_space);
            return Err(ProcessError::Full);
        };

        let task_id = TaskId(NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed));
        let process = create_process(task_id, parent, program.address_space);
        let process_id = process.id;

        let mut processes = process_table_lock();
        let Some(process_slot) = processes.iter().position(|entry| entry.is_none()) else {
            let _ = userspace::destroy(program.address_space);
            return Err(ProcessError::Full);
        };
        if ipc::register(process_id.as_u64()).is_err() {
            let _ = userspace::destroy(program.address_space);
            return Err(ProcessError::Full);
        }

        scheduler.tasks[task_slot].initialize_user(
            task_slot,
            task_id,
            name,
            context,
            program.address_space.paging(),
        );
        scheduler.tasks[task_slot].cpu = owner_cpu;
        scheduler.tasks[task_slot].migratable = true;
        scheduler.tasks[task_slot].affinity_mask = affinity_mask;
        scheduler.task_count += 1;
        processes[process_slot] = Some(process);
        scheduler.validate_affinity_invariants();
        let placement = MulticoreSpawnPlacement {
            owner_cpu: scheduler.tasks[task_slot].cpu,
            affinity_mask: scheduler.tasks[task_slot].affinity_mask,
        };
        Ok((process_id, task_id, context, placement))
    };

    let remote_owner = result
        .as_ref()
        .ok()
        .map(|(_, _, _, placement)| placement.owner_cpu)
        .filter(|owner_cpu| *owner_cpu != crate::smp::cpu_index());
    if let Some(owner_cpu) = remote_owner {
        let _ = crate::smp::reschedule_cpu(owner_cpu);
    }
    result
}

/// Spawn a deliberately pinned Ring-3 task on one online CPU.
///
/// Stage 7.3 uses this narrow API to prove that per-CPU TSS/GDT, address-space
/// switching, syscall entry and scheduler exit paths work from APs. It does
/// **not** make general userspace movable yet: the task is hard-pinned to the
/// selected CPU and callers must restrict the program to SMP-audited syscalls.
/// Normal `spawn_user_process()` semantics remain unchanged and CPU0-owned.
pub fn spawn_pinned_user_process_on(
    cpu: usize,
    name: &'static str,
    program: userspace::UserProgram,
) -> Result<(ProcessId, UserTaskContext), ProcessError> {
    if !crate::smp::cpu_is_online(cpu) || !program.image.is_valid() {
        let _ = userspace::destroy(program.address_space);
        return Err(ProcessError::Full);
    }

    let parent = ProcessId(current_process_id());
    let context = prepare_user_context(
        program.image.entry as usize,
        program.image.stack_top as usize,
    );

    let result = {
        let mut scheduler = SCHEDULER.lock();
        scheduler.reap_dead();
        let Some(task_slot) = scheduler
            .tasks
            .iter()
            .position(|task| task.state == TaskState::Empty)
        else {
            let _ = userspace::destroy(program.address_space);
            return Err(ProcessError::Full);
        };

        let task_id = TaskId(NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed));
        let process = create_process(task_id, parent, program.address_space);
        let process_id = process.id;

        let mut processes = process_table_lock();
        let Some(process_slot) = processes.iter().position(|entry| entry.is_none()) else {
            let _ = userspace::destroy(program.address_space);
            return Err(ProcessError::Full);
        };
        if ipc::register(process_id.as_u64()).is_err() {
            let _ = userspace::destroy(program.address_space);
            return Err(ProcessError::Full);
        }

        scheduler.tasks[task_slot].initialize_user(
            task_slot,
            task_id,
            name,
            context,
            program.address_space.paging(),
        );
        scheduler.tasks[task_slot].cpu = cpu;
        scheduler.tasks[task_slot].migratable = false;
        scheduler.tasks[task_slot].affinity_mask = cpu_bit(cpu);
        scheduler.task_count += 1;
        processes[process_slot] = Some(process);
        scheduler.validate_affinity_invariants();
        Ok((process_id, context))
    };

    // CPU0 follows the long-standing Stage 7.2 scheduling path. A remote AP
    // needs a reschedule IPI so the new Ready task is observed promptly.
    if result.is_ok() && cpu != crate::smp::cpu_index() {
        let _ = crate::smp::reschedule_cpu(cpu);
    }
    result
}

/// Spawn a minimal Ring-3 probe that may be migrated while it is still Ready.
///
/// Stage 7.4 deliberately keeps this API narrow: normal userspace remains
/// CPU0-owned, and callers must use a probe whose syscall/service footprint
/// has already been audited for AP execution (the acceptance suite uses
/// `/bin/true`, proven in Stage 7.3). Ownership may change only while Ready.
pub fn spawn_migratable_user_probe(
    name: &'static str,
    program: userspace::UserProgram,
) -> Result<(ProcessId, TaskId, UserTaskContext), ProcessError> {
    if !program.image.is_valid() {
        let _ = userspace::destroy(program.address_space);
        return Err(ProcessError::Full);
    }

    let parent = ProcessId(current_process_id());
    let context = prepare_user_context(
        program.image.entry as usize,
        program.image.stack_top as usize,
    );

    let mut scheduler = SCHEDULER.lock();
    scheduler.reap_dead();
    let Some(task_slot) = scheduler
        .tasks
        .iter()
        .position(|task| task.state == TaskState::Empty)
    else {
        let _ = userspace::destroy(program.address_space);
        return Err(ProcessError::Full);
    };

    let task_id = TaskId(NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed));
    let process = create_process(task_id, parent, program.address_space);
    let process_id = process.id;

    let mut processes = process_table_lock();
    let Some(process_slot) = processes.iter().position(|entry| entry.is_none()) else {
        let _ = userspace::destroy(program.address_space);
        return Err(ProcessError::Full);
    };
    if ipc::register(process_id.as_u64()).is_err() {
        let _ = userspace::destroy(program.address_space);
        return Err(ProcessError::Full);
    }

    scheduler.tasks[task_slot].initialize_user(
        task_slot,
        task_id,
        name,
        context,
        program.address_space.paging(),
    );
    scheduler.tasks[task_slot].cpu = 0;
    scheduler.tasks[task_slot].migratable = true;
    scheduler.tasks[task_slot].affinity_mask = online_affinity_mask();
    scheduler.task_count += 1;
    processes[process_slot] = Some(process);
    scheduler.validate_affinity_invariants();
    Ok((process_id, task_id, context))
}

pub fn process_exited(id: ProcessId) -> bool {
    process_table_lock()
        .iter()
        .flatten()
        .find(|process| process.id == id)
        .is_some_and(|process| process.state == ProcessState::Exited)
}

pub fn current_process_id() -> u64 {
    let task_id = current_task_id();
    process_table_lock()
        .iter()
        .flatten()
        .find(|process| process.task_id == task_id)
        .map_or(task_id.as_u64(), |process| process.id.as_u64())
}

static FILE_IO_DEPTH: [AtomicU64; crate::smp::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::smp::MAX_CPUS];
/// Keep device interrupts live while preventing another task on this CPU from
/// being scheduled onto an I/O lock owned by the current task. The depth is
/// per-CPU so unrelated CPUs remain schedulable. No process/cache lock spans
/// the interrupt-enabled I/O region.
pub fn file_fault_io<T>(operation: impl FnOnce() -> T) -> T {
    let enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();
    let cpu = crate::smp::cpu_index();
    FILE_IO_DEPTH[cpu].fetch_add(1, Ordering::AcqRel);
    let initialized = SCHEDULER.lock().task_count != 0;
    if initialized {
        x86_64::instructions::interrupts::enable();
    }
    let result = operation();
    x86_64::instructions::interrupts::disable();
    FILE_IO_DEPTH[cpu].fetch_sub(1, Ordering::AcqRel);
    if enabled {
        x86_64::instructions::interrupts::enable();
    }
    result
}

pub fn file_fault_io_self_test() -> bool {
    let enabled = x86_64::instructions::interrupts::are_enabled();
    let cpu = crate::smp::cpu_index();
    let id = current_task_id();
    let passed = file_fault_io(|| {
        let start = timer::ticks();
        while timer::ticks().wrapping_sub(start) < 2 {
            x86_64::instructions::hlt();
        }
        x86_64::instructions::interrupts::are_enabled()
            && current_task_id() == id
            && FILE_IO_DEPTH[cpu].load(Ordering::Acquire) == 1
    });
    passed
        && x86_64::instructions::interrupts::are_enabled() == enabled
        && FILE_IO_DEPTH[cpu].load(Ordering::Acquire) == 0
}

/// Queue an absent lazy file page for the pager task and suspend this task's
/// exception frame. When it runs again, the same page-fault handler returns
/// normally and `iretq` retries the original user instruction.
pub fn try_handle_file_fault(address: u64, write: bool) -> bool {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let task_id = current_task_id();
        let pager_task = match *PAGER_TASK.lock() {
            Some(id) => id,
            None => return false,
        };
        let (space, slot, mapping) = {
            let processes = process_table_lock();
            let Some(process) = processes.iter().flatten().find(|p| p.task_id == task_id) else {
                return false;
            };
            let Some(space) = process.address_space else {
                return false;
            };
            let Some((slot, mapping)) =
                process
                    .memory_mappings
                    .iter()
                    .enumerate()
                    .find_map(|(slot, m)| {
                        m.filter(|m| address >= m.address && address < m.address + m.size as u64)
                            .map(|m| (slot, m))
                    })
            else {
                return false;
            };
            (space, slot, mapping)
        };

        if !PAGER.lock().push(PagerRequest {
            task: task_id,
            space,
            slot,
            mapping,
            address,
            write,
        }) {
            return false;
        }
        PAGER_REQUESTS.fetch_add(1, Ordering::Relaxed);

        // Publish the faulting task's Blocked state and make the pager Ready
        // under one scheduler critical section. The old sequence woke the
        // pager first and only then blocked the faulting task. That ordering
        // contains a classic lost-wakeup window as soon as the pager and
        // faulting process can execute on different CPUs: the pager can finish
        // while the requester is still Running, its wake is ignored, and the
        // requester subsequently blocks forever. Atomic state publication
        // makes the wake/block handshake correct for both today's pinned
        // userspace and the multicore/migration stages that follow.
        block_current_and_wake_pager_from_exception(pager_task);

        PAGER
            .lock()
            .take_completion(task_id, address)
            .unwrap_or(false)
    })
}

fn block_current_and_wake_pager_from_exception(pager_id: TaskId) {
    let (switch, pager_cpu) = {
        let mut scheduler = SCHEDULER.lock();
        let cpu = crate::smp::cpu_index();
        let slot = scheduler.current_slot[cpu];
        assert!(
            scheduler.tasks[slot].id != KERNEL_TASK_ID,
            "kernel task cannot fault-block"
        );
        scheduler.tasks[slot].state = TaskState::Blocked;

        let pager_cpu = scheduler
            .tasks
            .iter_mut()
            .find(|task| task.id == pager_id && task.state != TaskState::Empty)
            .map(|pager| {
                if matches!(pager.state, TaskState::Blocked | TaskState::Sleeping) {
                    pager.state = TaskState::Ready;
                    pager.wake_tick = 0;
                }
                pager.cpu
            });

        (scheduler.prepare_switch(), pager_cpu)
    };

    // A remote pager may need an IPI. For a local pager prepare_switch() has
    // already reconsidered the run queue, while request_reschedule is harmless
    // and ensures a promptly-ready pager if another task won the first pick.
    if let Some(cpu) = pager_cpu {
        let _ = crate::smp::reschedule_cpu(cpu);
    }

    if let Some(context_switch) = switch {
        // The saved context includes the CPU exception frame. Resuming this
        // task returns here, consumes its pager completion, returns from the
        // page-fault handler, and hardware retries the original user RIP.
        unsafe { switch_stacks(context_switch) };
    }
}

fn pager_wait_for_request() -> PagerRequest {
    loop {
        let interrupts_were_enabled = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();

        // Hold PAGER across the empty-queue check and publication of Blocked.
        // A producer must acquire PAGER to enqueue first, so it cannot slip a
        // request into the old check/sleep window and then have its wake lost.
        let mut queue = PAGER.lock();
        if let Some(request) = queue.pop() {
            drop(queue);
            if interrupts_were_enabled {
                x86_64::instructions::interrupts::enable();
            }
            return request;
        }

        let switch = {
            let mut scheduler = SCHEDULER.lock();
            let cpu = crate::smp::cpu_index();
            let slot = scheduler.current_slot[cpu];
            assert_eq!(
                scheduler.tasks[slot].name,
                "pager",
                "pager wait executed outside pager task"
            );
            scheduler.tasks[slot].state = TaskState::Blocked;
            scheduler.prepare_switch()
        };

        // The queue lock must be released only after Blocked is visible. Any
        // producer that was waiting to enqueue can now push and wake this task.
        drop(queue);

        let context_switch = switch.expect(
            "blocked pager has no runnable replacement; per-CPU idle fallback is missing",
        );
        unsafe { switch_stacks(context_switch) };

        if interrupts_were_enabled {
            x86_64::instructions::interrupts::enable();
        }
    }
}

fn pager_task() -> ! {
    loop {
        let request = pager_wait_for_request();
        let mut mapping = request.mapping;
        let succeeded = userspace::populate_file_page(
            request.space,
            &mut mapping,
            request.address,
            request.write,
        );
        if succeeded {
            let mut processes = process_table_lock();
            if let Some(process) = processes
                .iter_mut()
                .flatten()
                .find(|p| p.task_id == request.task)
            {
                process.memory_mappings[request.slot] = Some(mapping);
            }
        }
        let recorded = PAGER.lock().finish(PagerCompletion {
            task: request.task,
            address: request.address,
            succeeded,
        });
        PAGER_COMPLETIONS.fetch_add(1, Ordering::Relaxed);
        if recorded {
            let _ = wake_task(request.task);
        }
    }
}

/// Start the single BSP pager worker after scheduler and timer initialization.
pub fn start_pager() -> bool {
    if PAGER_TASK.lock().is_some() {
        return true;
    }
    match spawn_with_priority("pager", pager_task, TaskPriority::NORMAL) {
        Ok(id) => {
            *PAGER_TASK.lock() = Some(id);
            true
        }
        Err(_) => false,
    }
}

pub fn pager_stats() -> (u64, u64) {
    (
        PAGER_REQUESTS.load(Ordering::Acquire),
        PAGER_COMPLETIONS.load(Ordering::Acquire),
    )
}

pub fn evict_one_mapped_file_page() -> bool {
    let mut processes = process_table_lock();
    for process in processes.iter_mut().flatten() {
        let Some(space) = process.address_space else {
            continue;
        };
        for mapping in process.memory_mappings.iter_mut().flatten() {
            let pages = mapping.size / 4096;
            for index in 0..pages {
                let address = mapping.address + (index * 4096) as u64;
                if userspace::evict_file_page(space, mapping, address) {
                    return true;
                }
            }
        }
    }
    false
}

/// Attempt to resolve a user write fault via copy-on-write.
/// Returns true if the fault was handled and the process may resume.
pub fn try_handle_cow_fault(fault_address: u64) -> bool {
    let task_id = current_task_id();
    let processes = process_table_lock();
    let Some(process) = processes
        .iter()
        .flatten()
        .find(|process| process.task_id == task_id)
    else {
        return false;
    };
    let Some(address_space) = process.address_space else {
        return false;
    };
    if !address_space.is_logically_writable(fault_address, &process.memory_mappings) {
        return false;
    }
    let paging_as = address_space.paging();
    drop(processes);
    if !paging::try_break_cow(paging_as, fault_address) {
        return false;
    }
    let mut processes = process_table_lock();
    if let Some(process) = processes
        .iter_mut()
        .flatten()
        .find(|process| process.task_id == task_id)
    {
        if let Some(mapping) = process
            .memory_mappings
            .iter_mut()
            .flatten()
            .find(|mapping| {
                fault_address >= mapping.address
                    && fault_address < mapping.address + mapping.size as u64
            })
        {
            let _ = userspace::mark_file_page_dirty(mapping, fault_address);
        }
    }
    true
}

pub fn current_credentials() -> Credentials {
    let task_id = current_task_id();
    process_table_lock()
        .iter()
        .flatten()
        .find(|process| process.task_id == task_id)
        .map_or(Credentials::ROOT, |process| process.credentials)
}

/// Return the active process credentials without panicking during early boot.
pub fn current_credentials_if_running() -> Option<Credentials> {
    let task_id = current_task_id_if_running()?;
    process_table_lock()
        .iter()
        .flatten()
        .find(|process| process.task_id == task_id)
        .map(|process| process.credentials)
}

pub fn process_credentials(id: ProcessId) -> Option<Credentials> {
    process_table_lock()
        .iter()
        .flatten()
        .find(|process| process.id == id)
        .map(|process| process.credentials)
}

pub fn may_ipc_with(receiver: u64) -> bool {
    let sender = current_credentials();
    let Some(receiver) = process_credentials(ProcessId(receiver)) else {
        return false;
    };
    credentials_may_ipc(sender, receiver)
}

pub fn credential_policy_valid() -> bool {
    let peer = Credentials {
        uid: Credentials::USERSPACE.uid,
        gid: 2000,
    };
    let group_peer = Credentials {
        uid: 2000,
        gid: Credentials::USERSPACE.gid,
    };
    let stranger = Credentials {
        uid: 2000,
        gid: 2000,
    };
    Credentials::ROOT.is_root()
        && !Credentials::USERSPACE.is_root()
        && Credentials::ROOT != Credentials::USERSPACE
        && credentials_may_ipc(Credentials::ROOT, stranger)
        && credentials_may_ipc(Credentials::USERSPACE, peer)
        && credentials_may_ipc(Credentials::USERSPACE, group_peer)
        && !credentials_may_ipc(Credentials::USERSPACE, stranger)
}

fn credentials_may_ipc(sender: Credentials, receiver: Credentials) -> bool {
    sender.is_root() || sender.uid == receiver.uid || sender.gid == receiver.gid
}
pub fn fork_current(frame: crate::syscall::UserForkFrame) -> Result<ProcessId, ProcessError> {
    let task_id = current_task_id();
    let (
        parent,
        capabilities,
        capability_lineages,
        security_domain,
        sandbox_profile,
        parent_cpu,
        parent_migratable,
        parent_affinity,
    ) = {
        let scheduler = SCHEDULER.lock();
        let parent_task = &scheduler.tasks[scheduler.current_slot[crate::smp::cpu_index()]];
        let capabilities = parent_task.capabilities;
        let capability_lineages = parent_task.capability_lineages;
        let security_domain = parent_task.security_domain;
        let sandbox_profile = parent_task.sandbox_profile;
        let parent_cpu = parent_task.cpu;
        let parent_migratable = parent_task.migratable;
        let parent_affinity = parent_task.affinity_mask;
        let processes = process_table_lock();
        let parent = processes
            .iter()
            .flatten()
            .find(|process| process.task_id == task_id)
            .copied()
            .ok_or(ProcessError::Full)?;
        (
            parent,
            capabilities,
            capability_lineages,
            security_domain,
            sandbox_profile,
            parent_cpu,
            parent_migratable,
            parent_affinity,
        )
    };
    let source_address_space = parent.address_space.ok_or(ProcessError::Full)?;
    let cloned_address_space =
        userspace::clone_address_space(source_address_space, &parent.memory_mappings)
            .ok_or(ProcessError::Full)?;

    let mut scheduler = SCHEDULER.lock();
    scheduler.reap_dead();
    let Some(task_slot) = scheduler
        .tasks
        .iter()
        .position(|task| task.state == TaskState::Empty)
    else {
        let _ =
            userspace::destroy_process_address_space(cloned_address_space, parent.memory_mappings);
        return Err(ProcessError::Full);
    };
    let mut processes = process_table_lock();
    let Some(process_slot) = processes.iter().position(Option::is_none) else {
        drop(processes);
        drop(scheduler);
        let _ =
            userspace::destroy_process_address_space(cloned_address_space, parent.memory_mappings);
        return Err(ProcessError::Full);
    };
    let child_task_id = TaskId(NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed));
    let child_id = ProcessId(NEXT_PROCESS_ID.fetch_add(1, Ordering::Relaxed));
    if ipc::register(child_id.as_u64()).is_err() {
        drop(processes);
        drop(scheduler);
        let _ =
            userspace::destroy_process_address_space(cloned_address_space, parent.memory_mappings);
        return Err(ProcessError::Full);
    }
    let child_files = match clone_file_table(&parent.files) {
        Ok(files) => files,
        Err(err) => {
            let _ = ipc::unregister(child_id.as_u64());
            drop(processes);
            drop(scheduler);
            let _ = userspace::destroy_process_address_space(
                cloned_address_space,
                parent.memory_mappings,
            );
            return Err(err);
        }
    };
    scheduler.tasks[task_slot].initialize_fork(
        task_slot,
        child_task_id,
        frame,
        cloned_address_space.paging(),
        ForkSecurityContext {
            capabilities,
            capability_lineages,
            security_domain,
            sandbox_profile,
        },
    );
    // Fork preserves the parent's CPU-placement contract. This keeps pinned
    // service workloads pinned and lets Stage 7.6 multicore processes keep
    // their audited affinity/migratability without silently falling back to CPU0.
    scheduler.tasks[task_slot].cpu = parent_cpu;
    scheduler.tasks[task_slot].migratable = parent_migratable;
    scheduler.tasks[task_slot].affinity_mask = parent_affinity;
    scheduler.validate_affinity_invariants();
    if parent_cpu != 0 {
        FORK_AFFINITY_INHERITANCES.fetch_add(1, Ordering::Relaxed);
    }
    scheduler.task_count += 1;
    processes[process_slot] = Some(Process {
        id: child_id,
        task_id: child_task_id,
        state: ProcessState::Ready,
        parent: parent.id,
        credentials: parent.credentials,
        exit_code: 0,
        address_space: Some(cloned_address_space),
        files: child_files,
        memory_mappings: parent.memory_mappings,
        memory_mapping_scopes: parent.memory_mapping_scopes,
        cwd: parent.cwd,
        cwd_len: parent.cwd_len,
        pending_signal: 0,
        process_group: parent.process_group,
        signal_actions: parent.signal_actions,
        environment: parent.environment,
    });
    Ok(child_id)
}
pub fn exec_current(program: userspace::UserProgram) -> ! {
    assert!(program.image.is_valid(), "exec received an invalid image");
    x86_64::instructions::interrupts::disable();
    let context = prepare_user_context(
        program.image.entry as usize,
        program.image.stack_top as usize,
    );
    let new_address_space = program.address_space;
    let task_id = current_task_id();
    let (old_address_space, old_mappings) = {
        let mut scheduler = SCHEDULER.lock();
        let slot = scheduler.current_slot[crate::smp::cpu_index()];
        assert!(scheduler.tasks[slot].id == task_id);

        let mut processes = process_table_lock();
        let process = processes
            .iter_mut()
            .flatten()
            .find(|process| process.task_id == task_id)
            .expect("exec process is missing from the process table");
        let old_address_space = process
            .address_space
            .replace(new_address_space)
            .expect("exec process has no owned address space");
        let old_mappings = core::mem::replace(
            &mut process.memory_mappings,
            [None; userspace::MAX_ANONYMOUS_MAPPINGS],
        );

        scheduler.tasks[slot].user_context = Some(context);
        scheduler.tasks[slot].address_space = Some(new_address_space.paging());
        (old_address_space, old_mappings)
    };

    paging::switch_to(new_address_space.paging());
    for mapping in old_mappings.into_iter().flatten() {
        assert!(
            userspace::unmap_anonymous(old_address_space, mapping),
            "failed to release an exec mapping"
        );
    }
    assert!(
        userspace::destroy(old_address_space),
        "failed to release the replaced exec image"
    );
    x86_64::instructions::interrupts::enable();
    enter_user_context(context)
}
pub fn exit_current_process(exit_code: i32) -> ! {
    let exiting_pid = current_process_id();
    crate::terminal::release_foreground(exiting_pid);
    crate::network::close_process_sockets(exiting_pid);
    x86_64::instructions::interrupts::disable();
    let context_switch_result = {
        let mut scheduler = SCHEDULER.lock();
        let slot = scheduler.current_slot[crate::smp::cpu_index()];
        let task_id = scheduler.tasks[slot].id;
        assert!(
            task_id != KERNEL_TASK_ID,
            "kernel task cannot exit as a process"
        );

        // Stage 10.3: an exiting owner must not strand generic async slots.
        // Producers racing this teardown are serialized by async_op::TABLE;
        // a late completion sees a stale generation-tagged handle.
        crate::async_network::release_owner(task_id);
        crate::async_events::release_owner(exiting_pid);
        crate::async_file::release_owner(task_id);
        crate::block_io::release_owner(task_id);
        crate::async_op::release_owner(task_id);
        crate::completion_port::release_owner(exiting_pid);
        // Reclaim all registry records before the process becomes a zombie;
        // generation advancement invalidates handles held by the exiting task.
        crate::thread::reap_owner(exiting_pid);
        crate::notifications::discard_recipient(exiting_pid);

        // Interrupts are already disabled here. Use the same process-table
        // guard as every other path so the lock's IRQ-safety contract is not
        // bypassed during process teardown. Lock order is SCHEDULER ->
        // PROCESS_TABLE throughout this critical section.
        let address_space_result = process_table_lock()
            .iter_mut()
            .flatten()
            .find(|process| process.task_id == task_id)
            .and_then(|process| {
                process.state = ProcessState::Exited;
                process.exit_code = exit_code;
                // Drop open-file references as soon as the process exits so
                // offsets and description slots are not held by zombies.
                release_file_table(&mut process.files);
                process.address_space.take().map(|address_space| {
                    let memory_mappings = core::mem::replace(
                        &mut process.memory_mappings,
                        [None; userspace::MAX_ANONYMOUS_MAPPINGS],
                    );
                    (address_space, memory_mappings)
                })
            });

        scheduler.tasks[slot].state = TaskState::Dead;
        scheduler.tasks[slot].name = "exited";
        scheduler.task_count = scheduler.task_count.saturating_sub(1);

        address_space_result.map(|(address_space, memory_mappings)| {
            (scheduler.prepare_switch(), address_space, memory_mappings)
        })
    };

    if let Some((context_switch, address_space, memory_mappings)) = context_switch_result {
        if let Some(context_switch) = context_switch {
            paging::switch_to(context_switch.next_address_space);
            for mapping in memory_mappings.into_iter().flatten() {
                assert!(
                    userspace::unmap_anonymous(address_space, mapping),
                    "failed to release exiting process mapping"
                );
            }
            assert!(
                userspace::destroy(address_space),
                "failed to release exiting process address space"
            );
            unsafe { switch_stacks(context_switch) };
        }
    } else {
        crate::serial::write_line(format_args!(
            "[TASK] exiting process without address space; entering idle"
        ));
    }

    loop {
        x86_64::instructions::hlt();
    }
}
pub fn wait_process(child_id: u64) -> Result<i32, WaitError> {
    let parent = ProcessId(current_process_id());
    let child = ProcessId(child_id);
    let (exit_code, deferred_space) = {
        let mut processes = process_table_lock();
        let Some(slot) = processes.iter().position(
            |entry| matches!(entry, Some(process) if process.id == child && process.parent == parent),
        ) else {
            return Err(WaitError::NoSuchChild);
        };
        let process = processes[slot].as_mut().ok_or(WaitError::NoSuchChild)?;
        if process.state != ProcessState::Exited {
            return Err(WaitError::StillRunning);
        }
        release_file_table(&mut process.files);
        let exit_code = process.exit_code;
        let deferred_space = process.address_space.take().map(|address_space| {
            let mappings = core::mem::replace(
                &mut process.memory_mappings,
                [None; userspace::MAX_ANONYMOUS_MAPPINGS],
            );
            (address_space, mappings)
        });
        assert!(
            ipc::unregister(child.as_u64()).is_ok(),
            "reaped process has no IPC endpoint"
        );
        processes[slot] = None;
        (exit_code, deferred_space)
    };

    // A signal-terminated zombie deliberately keeps its address space until
    // reap time. By definition its scheduler task is already retired here, so
    // destruction cannot invalidate a live CPU context.
    if let Some((address_space, mappings)) = deferred_space {
        for mapping in mappings.into_iter().flatten() {
            assert!(
                userspace::unmap_anonymous(address_space, mapping),
                "failed to release reaped process mapping"
            );
        }
        assert!(
            userspace::destroy(address_space),
            "failed to release reaped process address space"
        );
    }
    Ok(exit_code)
}

pub fn zombie_count() -> usize {
    process_table_lock()
        .iter()
        .flatten()
        .filter(|process| process.state == ProcessState::Exited)
        .count()
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ProcessInfo {
    pub pid: u64,
    pub ppid: u64,
    pub pgrp: u64,
    pub state: u64,
    pub uid: u32,
    pub gid: u32,
    pub exit_code: i32,
    pub reserved: u32,
}

pub fn process_count() -> usize {
    process_table_lock().iter().flatten().count()
}

pub fn process_info(index: usize) -> Option<ProcessInfo> {
    let processes = process_table_lock();
    let process = processes.iter().flatten().nth(index)?;
    let state = match process.state {
        ProcessState::Ready => 1,
        ProcessState::Exited => 4,
    };
    Some(ProcessInfo {
        pid: process.id.as_u64(),
        ppid: process.parent.as_u64(),
        pgrp: process.process_group,
        state,
        uid: process.credentials.uid,
        gid: process.credentials.gid,
        exit_code: process.exit_code,
        reserved: 0,
    })
}

pub fn env_get(key: &[u8], out: &mut [u8]) -> Result<usize, EnvError> {
    if key.is_empty() || key.contains(&b'=') || !key.is_ascii() {
        return Err(EnvError::Invalid);
    }
    let task_id = current_task_id();
    let processes = process_table_lock();
    let process = processes
        .iter()
        .flatten()
        .find(|p| p.task_id == task_id)
        .ok_or(EnvError::NoProcess)?;
    for i in 0..MAX_ENV_VARS {
        let len = process.environment.lengths[i] as usize;
        let entry = &process.environment.entries[i][..len];
        if len > key.len() && &entry[..key.len()] == key && entry[key.len()] == b'=' {
            let value = &entry[key.len() + 1..];
            if value.len() > out.len() {
                return Err(EnvError::BufferTooSmall);
            }
            out[..value.len()].copy_from_slice(value);
            return Ok(value.len());
        }
    }
    Err(EnvError::NotFound)
}

pub fn env_set(key: &[u8], value: &[u8]) -> Result<(), EnvError> {
    let task_id = current_task_id();
    let mut processes = process_table_lock();
    let process = processes
        .iter_mut()
        .flatten()
        .find(|p| p.task_id == task_id)
        .ok_or(EnvError::NoProcess)?;
    if key.is_empty() || key.contains(&b'=') || !key.is_ascii() || !value.is_ascii() {
        return Err(EnvError::Invalid);
    }
    if key.len().saturating_add(value.len()).saturating_add(1) > MAX_ENV_ENTRY {
        return Err(EnvError::Invalid);
    }
    if process.environment.set_bytes(key, value) {
        Ok(())
    } else {
        Err(EnvError::Full)
    }
}

pub fn env_count() -> usize {
    let task_id = current_task_id();
    process_table_lock()
        .iter()
        .flatten()
        .find(|p| p.task_id == task_id)
        .map(|p| {
            p.environment
                .lengths
                .iter()
                .filter(|&&len| len != 0)
                .count()
        })
        .unwrap_or(0)
}

pub fn env_entry(index: usize, out: &mut [u8]) -> Result<usize, EnvError> {
    let task_id = current_task_id();
    let processes = process_table_lock();
    let process = processes
        .iter()
        .flatten()
        .find(|p| p.task_id == task_id)
        .ok_or(EnvError::NoProcess)?;
    let mut seen = 0usize;
    for i in 0..MAX_ENV_VARS {
        let len = process.environment.lengths[i] as usize;
        if len == 0 {
            continue;
        }
        if seen == index {
            if len > out.len() {
                return Err(EnvError::BufferTooSmall);
            }
            out[..len].copy_from_slice(&process.environment.entries[i][..len]);
            return Ok(len);
        }
        seen += 1;
    }
    Err(EnvError::NotFound)
}

pub fn mmap_current(length: u64, writable: bool) -> Result<u64, MemoryError> {
    let length = usize::try_from(length).map_err(|_| MemoryError::InvalidLength)?;
    let size = length
        .checked_add(4095)
        .map(|size| size & !4095)
        .filter(|size| *size != 0)
        .ok_or(MemoryError::InvalidLength)?;
    let task_id = current_task_id();
    let (process_index, slot, address_space) = {
        let processes = process_table_lock();
        let process_index = processes
            .iter()
            .position(|process| process.is_some_and(|process| process.task_id == task_id))
            .ok_or(MemoryError::NoProcess)?;
        let process = processes[process_index].ok_or(MemoryError::NoProcess)?;
        let slot = process
            .memory_mappings
            .iter()
            .position(Option::is_none)
            .ok_or(MemoryError::Full)?;
        let address_space = process.address_space.ok_or(MemoryError::NoProcess)?;
        (process_index, slot, address_space)
    };
    let mapping = userspace::map_anonymous(address_space, slot, size, writable)
        .ok_or(MemoryError::MappingFailed)?;
    let mut processes = process_table_lock();
    let process = processes[process_index]
        .as_mut()
        .ok_or(MemoryError::NoProcess)?;
    if process.memory_mappings[slot].is_some() {
        let _ = userspace::unmap_anonymous(address_space, mapping);
        return Err(MemoryError::Full);
    }
    process.memory_mappings[slot] = Some(mapping);
    process.memory_mapping_scopes[slot] = None;
    Ok(mapping.address)
}

pub fn mmap_file_current(
    descriptor: u64,
    length: u64,
    offset: u64,
    writable: bool,
    lazy: bool,
    shared: bool,
) -> Result<u64, MemoryError> {
    let (can_read, can_write, profile) = current_file_authority().ok_or(MemoryError::NoProcess)?;
    if !can_read || (shared && !can_write) {
        return Err(MemoryError::PermissionDenied);
    }
    let descriptor = usize::try_from(descriptor).map_err(|_| MemoryError::BadDescriptor)?;
    let length = usize::try_from(length).map_err(|_| MemoryError::InvalidLength)?;
    let offset = usize::try_from(offset).map_err(|_| MemoryError::InvalidLength)?;
    let task_id = current_task_id();
    let (index, slot, address_space, file, file_scope) = {
        let processes = process_table_lock();
        let index = processes
            .iter()
            .position(|p| p.is_some_and(|p| p.task_id == task_id))
            .ok_or(MemoryError::NoProcess)?;
        let process = processes[index].ok_or(MemoryError::NoProcess)?;
        let Some(FdKind::File { id: file, scope }) = process.files.get(descriptor).copied().flatten() else {
            return Err(MemoryError::BadDescriptor);
        };
        if !profile.allows_file_read(scope) || (shared && !profile.allows_file_write(scope)) {
            return Err(MemoryError::PermissionDenied);
        };

        let slot = process
            .memory_mappings
            .iter()
            .position(Option::is_none)
            .ok_or(MemoryError::Full)?;
        (
            index,
            slot,
            process.address_space.ok_or(MemoryError::NoProcess)?,
            file,
            scope,
        )
    };
    let mapping = if shared {
        userspace::map_file_shared(address_space, slot, file, offset, length)
    } else if lazy {
        userspace::map_file_lazy(address_space, slot, file, offset, length, writable)
    } else {
        userspace::map_file_private(address_space, slot, file, offset, length, writable)
    }
    .ok_or(MemoryError::MappingFailed)?;
    let mut processes = process_table_lock();
    if let Some(process) = processes[index].as_mut().filter(|p| p.task_id == task_id) {
        if process.memory_mappings[slot].is_none() {
            process.memory_mappings[slot] = Some(mapping);
            process.memory_mapping_scopes[slot] = Some(file_scope);
            return Ok(mapping.address);
        }
    }
    let _ = userspace::unmap_anonymous(address_space, mapping);
    Err(MemoryError::Full)
}

pub fn msync_current(address: u64, length: u64) -> Result<(), MemoryError> {
    let (_can_read, can_write, profile) = current_file_authority().ok_or(MemoryError::NoProcess)?;
    if !can_write {
        return Err(MemoryError::PermissionDenied);
    }
    let size = length
        .checked_add(4095)
        .filter(|_| length != 0)
        .ok_or(MemoryError::InvalidLength)?
        & !4095;
    let task_id = current_task_id();
    let (space, mapping) = {
        let processes = process_table_lock();
        let process = processes
            .iter()
            .flatten()
            .find(|p| p.task_id == task_id)
            .ok_or(MemoryError::NoProcess)?;
        let slot = process
            .memory_mappings
            .iter()
            .position(|mapping| {
                mapping.is_some_and(|m| m.address == address && size == m.size as u64)
            })
            .ok_or(MemoryError::NotFound)?;
        let mapping = process.memory_mappings[slot].ok_or(MemoryError::NotFound)?;
        if let Some(scope) = process.memory_mapping_scopes[slot] {
            if !profile.allows_file_write(scope) {
                return Err(MemoryError::PermissionDenied);
            }
        }
        (
            process.address_space.ok_or(MemoryError::NoProcess)?,
            mapping,
        )
    };
    if userspace::sync_file_mapping(space, mapping, true) {
        Ok(())
    } else {
        Err(MemoryError::MappingFailed)
    }
}

pub fn munmap_current(address: u64, length: u64) -> Result<(), MemoryError> {
    let length = usize::try_from(length).map_err(|_| MemoryError::InvalidLength)?;
    let size = length
        .checked_add(4095)
        .map(|size| size & !4095)
        .filter(|size| *size != 0)
        .ok_or(MemoryError::InvalidLength)?;
    let task_id = current_task_id();
    let (process_index, slot, address_space, mapping) = {
        let processes = process_table_lock();
        let process_index = processes
            .iter()
            .position(|process| process.is_some_and(|process| process.task_id == task_id))
            .ok_or(MemoryError::NoProcess)?;
        let process = processes[process_index].ok_or(MemoryError::NoProcess)?;
        let slot = process
            .memory_mappings
            .iter()
            .position(|mapping| {
                mapping.is_some_and(|mapping| mapping.address == address && mapping.size == size)
            })
            .ok_or(MemoryError::NotFound)?;
        let mapping = process.memory_mappings[slot].ok_or(MemoryError::NotFound)?;
        let address_space = process.address_space.ok_or(MemoryError::NoProcess)?;
        (process_index, slot, address_space, mapping)
    };
    if !userspace::unmap_anonymous(address_space, mapping) {
        return Err(MemoryError::MappingFailed);
    }
    let mut processes = process_table_lock();
    let process = processes[process_index].as_mut().ok_or(MemoryError::NoProcess)?;
    process.memory_mappings[slot] = None;
    process.memory_mapping_scopes[slot] = None;
    Ok(())
}

pub fn anonymous_mapping_count() -> usize {
    process_table_lock()
        .iter()
        .flatten()
        .map(|process| process.memory_mappings.iter().flatten().count())
        .sum()
}
pub fn open_current(path: &str) -> Result<u64, FileError> {
    if !current_has(Capability::FileRead) {
        return Err(FileError::PermissionDenied);
    }
    let path = resolve_path_for_current(path)?;
    if !authorize_current_file_path(&path, false) {
        return Err(FileError::PermissionDenied);
    }
    if touches_mnt(&path) {
        if !crate::storage::mnt_mounted() {
            return Err(FileError::NotFound);
        }
        match crate::storage::ensure_path(&path) {
            Ok(()) => {}
            Err(crate::storage::EnsureError::NotFound) => ensure_mnt_parent(&path)?,
            Err(error) => return Err(map_ensure_file_error(error)),
        }
    }
    let file = match vfs::open(&path) {
        Ok(id) => id,
        Err(_) => {
            // POSIX-ish O_CREAT for missing files when writer-capable.
            if !current_has(Capability::FileWrite) || !authorize_current_file_path(&path, true) {
                return Err(FileError::NotFound);
            }
            vfs::write_file(&path, &[]).map_err(|_| FileError::NotFound)?;
            if is_mnt_child(&path) {
                crate::storage::mark_mnt_dirty();
            }
            vfs::open(&path).map_err(|_| FileError::NotFound)?
        }
    };
    let task_id = current_task_id();
    let mut processes = process_table_lock();
    let process = processes
        .iter_mut()
        .flatten()
        .find(|process| process.task_id == task_id)
        .ok_or(FileError::NoProcess)?;
    let descriptor = (3..MAX_FILE_DESCRIPTORS)
        .find(|descriptor| process.files[*descriptor].is_none())
        .ok_or(FileError::TooManyFiles)?;
    process.files[descriptor] = Some(FdKind::File {
        id: file,
        scope: wovenguard::classify_file_path(&path),
    });
    Ok(descriptor as u64)
}
pub fn descriptor_is_open(descriptor: u64) -> bool {
    let Ok(descriptor) = usize::try_from(descriptor) else {
        return false;
    };
    let task_id = current_task_id();
    process_table_lock()
        .iter()
        .flatten()
        .find(|process| process.task_id == task_id)
        .and_then(|process| process.files.get(descriptor))
        .is_some_and(Option::is_some)
}

/// Pin the exact open-file description behind a userspace descriptor for an
/// asynchronous request. The returned OpenFileId owns an extra VFS reference,
/// so closing/reusing the numeric fd cannot redirect an in-flight operation.
pub fn pin_current_file_for_async(
    descriptor: u64,
    write: bool,
) -> Result<(vfs::OpenFileId, wovenguard::FileScope, bool), FileError> {
    let descriptor = usize::try_from(descriptor).map_err(|_| FileError::BadDescriptor)?;
    let task_id = current_task_id();
    let processes = process_table_lock();
    let process = processes
        .iter()
        .flatten()
        .find(|process| process.task_id == task_id)
        .ok_or(FileError::NoProcess)?;
    let fd = process
        .files
        .get(descriptor)
        .copied()
        .flatten()
        .ok_or(FileError::BadDescriptor)?;
    let FdKind::File { id, scope } = fd else {
        return Err(FileError::BadDescriptor);
    };
    drop(processes);
    if !current_file_access_allowed(
        if write { Capability::FileWrite } else { Capability::FileRead },
        scope,
        write,
    ) {
        return Err(FileError::PermissionDenied);
    }
    let under_mnt = vfs::open_file_path_starts_with(id, "/mnt/").unwrap_or(false);
    if under_mnt && !crate::storage::mnt_mounted() {
        return Err(FileError::NotFound);
    }
    let pinned = vfs::clone_open_file(id).map_err(|_| FileError::TooManyFiles)?;
    Ok((pinned, scope, under_mnt))
}

/// Re-check the caller's current WovenGuard file authority when collecting an
/// async result. Cancellation itself intentionally does not require this check.
pub fn authorize_current_file_scope(scope: wovenguard::FileScope, write: bool) -> bool {
    current_file_access_allowed(
        if write { Capability::FileWrite } else { Capability::FileRead },
        scope,
        write,
    )
}

pub fn read_current(descriptor: u64, buffer: &mut [u8]) -> Result<usize, FileError> {
    let descriptor = usize::try_from(descriptor).map_err(|_| FileError::BadDescriptor)?;
    let task_id = current_task_id();
    let processes = process_table_lock();
    let process = processes
        .iter()
        .flatten()
        .find(|process| process.task_id == task_id)
        .ok_or(FileError::NoProcess)?;
    let fd = process
        .files
        .get(descriptor)
        .copied()
        .flatten()
        .ok_or(FileError::BadDescriptor)?;
    drop(processes);
    match fd {
        FdKind::File { id, scope } => {
            if !current_file_access_allowed(Capability::FileRead, scope, false) {
                return Err(FileError::PermissionDenied);
            }
            vfs::read(id, buffer).map_err(|_| FileError::BadDescriptor)
        },
        FdKind::PipeRead(id) => crate::pipe::read(id, buffer).map_err(|_| FileError::BadDescriptor),
        FdKind::PipeWrite(_) => Err(FileError::BadDescriptor),
    }
}

pub fn write_current(descriptor: u64, buffer: &[u8]) -> Result<usize, FileError> {
    let descriptor = usize::try_from(descriptor).map_err(|_| FileError::BadDescriptor)?;
    let task_id = current_task_id();
    let processes = process_table_lock();
    let process = processes
        .iter()
        .flatten()
        .find(|process| process.task_id == task_id)
        .ok_or(FileError::NoProcess)?;
    let fd = process
        .files
        .get(descriptor)
        .copied()
        .flatten()
        .ok_or(FileError::BadDescriptor)?;
    drop(processes);
    match fd {
        FdKind::File { id, scope } => {
            if !current_file_access_allowed(Capability::FileWrite, scope, true) {
                return Err(FileError::PermissionDenied);
            }
            let under_mnt = vfs::open_file_path_starts_with(id, "/mnt/").unwrap_or(false);
            if under_mnt && !crate::storage::mnt_mounted() {
                return Err(FileError::NotFound);
            }
            let written = vfs::write(id, buffer).map_err(|_| FileError::BadDescriptor);
            if written.is_ok() && under_mnt {
                crate::storage::mark_mnt_dirty();
            }
            written
        }
        FdKind::PipeWrite(id) => {
            crate::pipe::write(id, buffer).map_err(|_| FileError::BadDescriptor)
        }
        FdKind::PipeRead(_) => Err(FileError::BadDescriptor),
    }
}

pub fn dup_current(descriptor: u64) -> Result<u64, FileError> {
    let descriptor = usize::try_from(descriptor).map_err(|_| FileError::BadDescriptor)?;
    let task_id = current_task_id();
    let mut processes = process_table_lock();
    let process = processes
        .iter_mut()
        .flatten()
        .find(|process| process.task_id == task_id)
        .ok_or(FileError::NoProcess)?;
    let fd = process
        .files
        .get(descriptor)
        .copied()
        .flatten()
        .ok_or(FileError::BadDescriptor)?;
    let cloned = match fd {
        FdKind::File { id, scope } => FdKind::File {
            id: vfs::clone_open_file(id).map_err(|_| FileError::TooManyFiles)?,
            scope,
        },
        FdKind::PipeRead(id) => {
            crate::pipe::clone_reader(id).map_err(|_| FileError::TooManyFiles)?;
            FdKind::PipeRead(id)
        },
        FdKind::PipeWrite(id) => {
            crate::pipe::clone_writer(id).map_err(|_| FileError::TooManyFiles)?;
            FdKind::PipeWrite(id)
        },
    };
    let new_descriptor = (3..MAX_FILE_DESCRIPTORS)
        .find(|slot| process.files[*slot].is_none())
        .ok_or(FileError::TooManyFiles)?;
    process.files[new_descriptor] = Some(cloned);
    Ok(new_descriptor as u64)
}

pub fn dup2_current(old: u64, new: u64) -> Result<u64, FileError> {
    let old = usize::try_from(old).map_err(|_| FileError::BadDescriptor)?;
    let new = usize::try_from(new).map_err(|_| FileError::BadDescriptor)?;
    if new >= MAX_FILE_DESCRIPTORS {
        return Err(FileError::BadDescriptor);
    }
    if old == new {
        return Ok(new as u64);
    }
    let task_id = current_task_id();
    let mut processes = process_table_lock();
    let process = processes
        .iter_mut()
        .flatten()
        .find(|process| process.task_id == task_id)
        .ok_or(FileError::NoProcess)?;
    let fd = process
        .files
        .get(old)
        .copied()
        .flatten()
        .ok_or(FileError::BadDescriptor)?;
    let cloned = match fd {
        FdKind::File { id, scope } => FdKind::File {
            id: vfs::clone_open_file(id).map_err(|_| FileError::TooManyFiles)?,
            scope,
        },
        FdKind::PipeRead(id) => {
            crate::pipe::clone_reader(id).map_err(|_| FileError::TooManyFiles)?;
            FdKind::PipeRead(id)
        },
        FdKind::PipeWrite(id) => {
            crate::pipe::clone_writer(id).map_err(|_| FileError::TooManyFiles)?;
            FdKind::PipeWrite(id)
        },
    };
    if let Some(prev) = process.files[new].take() {
        release_fd(prev);
    }
    process.files[new] = Some(cloned);
    Ok(new as u64)
}

pub fn pipe_current() -> Result<(u64, u64), FileError> {
    let pipe_id = crate::pipe::create().map_err(|_| FileError::TooManyFiles)?;
    let task_id = current_task_id();
    let mut processes = process_table_lock();
    let process = processes
        .iter_mut()
        .flatten()
        .find(|process| process.task_id == task_id)
        .ok_or(FileError::NoProcess)?;
    let read_fd = (3..MAX_FILE_DESCRIPTORS)
        .find(|slot| process.files[*slot].is_none())
        .ok_or(FileError::TooManyFiles)?;
    process.files[read_fd] = Some(FdKind::PipeRead(pipe_id));
    let write_fd = (3..MAX_FILE_DESCRIPTORS)
        .find(|slot| process.files[*slot].is_none())
        .ok_or(FileError::TooManyFiles)?;
    process.files[write_fd] = Some(FdKind::PipeWrite(pipe_id));
    Ok((read_fd as u64, write_fd as u64))
}

pub fn getppid_current() -> u64 {
    // Global lock order is SCHEDULER -> PROCESS_TABLE. Resolve the task id
    // before taking PROCESS_TABLE so SMP callers can never form an AB-BA
    // cycle with fork/exec/exit, which legitimately publish both tables.
    let task_id = current_task_id();
    let processes = process_table_lock();
    processes
        .iter()
        .flatten()
        .find(|p| p.task_id == task_id)
        .map(|p| p.parent.as_u64())
        .unwrap_or(0)
}

pub fn close_current(descriptor: u64) -> Result<(), FileError> {
    let descriptor = usize::try_from(descriptor).map_err(|_| FileError::BadDescriptor)?;
    // Standard streams cannot be closed.
    if descriptor < 3 {
        return Err(FileError::BadDescriptor);
    }
    let task_id = current_task_id();
    let mut processes = process_table_lock();
    let process = processes
        .iter_mut()
        .flatten()
        .find(|process| process.task_id == task_id)
        .ok_or(FileError::NoProcess)?;
    let fd = process
        .files
        .get_mut(descriptor)
        .ok_or(FileError::BadDescriptor)?
        .take()
        .ok_or(FileError::BadDescriptor)?;
    drop(processes);
    release_fd(fd);
    Ok(())
}

pub fn open_file_count() -> usize {
    process_table_lock()
        .iter()
        .flatten()
        .map(|process| process.files.iter().flatten().count())
        .sum()
}

pub fn stat_path(path: &str) -> Result<vfs::Stat, FileError> {
    if !current_has(Capability::FileRead) {
        return Err(FileError::PermissionDenied);
    }
    let path = resolve_path_for_current(path)?;
    if !authorize_current_file_path(&path, false) {
        return Err(FileError::PermissionDenied);
    }
    ensure_existing_mnt_path(&path)?;
    vfs::stat(&path).map_err(|_| FileError::NotFound)
}

pub fn readdir_path(path: &str, index: usize) -> Result<vfs::DirEntry, FileError> {
    if !current_has(Capability::FileRead) {
        return Err(FileError::PermissionDenied);
    }
    let path = resolve_path_for_current(path)?;
    if !authorize_current_file_path(&path, false) {
        return Err(FileError::PermissionDenied);
    }
    ensure_existing_mnt_path(&path)?;
    vfs::readdir(&path, index).map_err(|_| FileError::NotFound)
}

pub fn mkdir_path(path: &str) -> Result<(), FileError> {
    if !current_has(Capability::FileWrite) {
        return Err(FileError::PermissionDenied);
    }
    let path = resolve_path_for_current(path)?;
    if !authorize_current_file_path(&path, true) {
        return Err(FileError::PermissionDenied);
    }
    if touches_mnt(&path) && !crate::storage::mnt_mounted() {
        return Err(FileError::NotFound);
    }
    if is_mnt_child(&path) {
        ensure_mnt_parent(&path)?;
    }
    match vfs::mkdir(&path) {
        Ok(()) => {}
        Err(vfs::Error::AlreadyExists) => return Err(FileError::AlreadyExists),
        Err(vfs::Error::Full) => return Err(FileError::TooManyFiles),
        Err(_) => return Err(FileError::NotFound),
    }
    if is_mnt_child(&path) {
        if let Err(error) = crate::storage::persist_directory(&path) {
            let _ = vfs::remove(&path);
            return Err(map_persist_file_error(error));
        }
    }
    Ok(())
}

pub fn current_cwd_str(buf: &mut [u8]) -> Result<usize, FileError> {
    let (cwd, len) = current_cwd();
    if buf.len() < len {
        return Err(FileError::TooManyFiles);
    }
    buf[..len].copy_from_slice(&cwd[..len]);
    Ok(len)
}

pub fn current_cwd() -> ([u8; crate::config::MAX_PATH_SIZE], usize) {
    // Preserve the kernel-wide SCHEDULER -> PROCESS_TABLE lock order.
    let task_id = current_task_id();
    let processes = process_table_lock();
    if let Some(p) = processes.iter().flatten().find(|p| p.task_id == task_id) {
        return (p.cwd, p.cwd_len);
    }
    let mut root = [0u8; crate::config::MAX_PATH_SIZE];
    root[0] = b'/';
    (root, 1)
}

pub fn chdir_current(path: &str) -> Result<(), FileError> {
    if !current_has(Capability::FileRead) {
        return Err(FileError::PermissionDenied);
    }
    let absolute = resolve_path_for_current(path)?;
    if !authorize_current_file_path(&absolute, false) {
        return Err(FileError::PermissionDenied);
    }
    ensure_existing_mnt_path(&absolute)?;
    match vfs::stat(&absolute) {
        Ok(stat) if stat.kind == vfs::NodeKind::Directory => {}
        _ => return Err(FileError::NotFound),
    }
    let bytes = absolute.as_bytes();
    if bytes.len() > crate::config::MAX_PATH_SIZE {
        return Err(FileError::NotFound);
    }
    // Resolve scheduler-owned identity before taking PROCESS_TABLE. Keeping
    // this order consistent is required for deadlock-free SMP execution.
    let task_id = current_task_id();
    let mut processes = process_table_lock();
    let process = processes
        .iter_mut()
        .flatten()
        .find(|p| p.task_id == task_id)
        .ok_or(FileError::NoProcess)?;
    process.cwd = [0; crate::config::MAX_PATH_SIZE];
    process.cwd[..bytes.len()].copy_from_slice(bytes);
    process.cwd_len = bytes.len();
    Ok(())
}
pub fn resolve_path_for_current(path: &str) -> Result<alloc::string::String, FileError> {
    let cwd = current_cwd();
    resolve_path_with_cwd(path, &cwd)
}

fn resolve_path_with_cwd(
    path: &str,
    cwd: &([u8; crate::config::MAX_PATH_SIZE], usize),
) -> Result<alloc::string::String, FileError> {
    if path.is_empty() {
        return Err(FileError::NotFound);
    }
    if path.starts_with('/') {
        return normalize_absolute(path);
    }
    let cwd_str = core::str::from_utf8(&cwd.0[..cwd.1]).map_err(|_| FileError::NotFound)?;
    let mut joined = alloc::string::String::new();
    if cwd_str == "/" {
        joined.push('/');
        joined.push_str(path);
    } else {
        joined.push_str(cwd_str);
        joined.push('/');
        joined.push_str(path);
    }
    normalize_absolute(&joined)
}

fn normalize_absolute(path: &str) -> Result<alloc::string::String, FileError> {
    if !path.starts_with('/') {
        return Err(FileError::NotFound);
    }
    let mut stack = alloc::vec::Vec::new();
    for component in path.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            let _ = stack.pop();
            continue;
        }
        if component.as_bytes().contains(&0) {
            return Err(FileError::NotFound);
        }
        stack.push(component);
    }
    if stack.is_empty() {
        return Ok(alloc::string::String::from("/"));
    }
    let mut out = alloc::string::String::from("/");
    for (i, part) in stack.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        out.push_str(part);
    }
    if out.len() > crate::config::MAX_PATH_SIZE {
        return Err(FileError::NotFound);
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug)]
pub struct UserTaskContext {
    pub entry: u64,
    pub stack_top: u64,
    pub code_segment: u16,
    pub data_segment: u16,
}

pub fn prepare_user_context(entry: usize, stack_top: usize) -> UserTaskContext {
    let (code_segment, data_segment) = gdt::user_segments();
    // The GDT entries are DPL3 user descriptors, but SegmentSelector values
    // returned by GDT::append carry RPL0 unless the requestor privilege bits
    // are set explicitly.  iretq validates both DPL and RPL when crossing
    // from CPL0 to CPL3, so pass selectors with RPL3 (low two bits = 3).
    UserTaskContext {
        entry: entry as u64,
        stack_top: stack_top as u64,
        code_segment: code_segment.0 | 3,
        data_segment: data_segment.0 | 3,
    }
}

fn enter_user_context(context: UserTaskContext) -> ! {
    unsafe {
        wovenhat_enter_user_mode(
            context.entry,
            context.stack_top,
            context.code_segment,
            context.data_segment,
        )
    }
}

pub fn spawn(name: &'static str, entry: fn() -> !) -> Result<TaskId, SpawnError> {
    spawn_with_priority(name, entry, TaskPriority::NORMAL)
}

pub fn spawn_with_priority(
    name: &'static str,
    entry: fn() -> !,
    priority: TaskPriority,
) -> Result<TaskId, SpawnError> {
    let mut scheduler = SCHEDULER.lock();
    assert!(scheduler.task_count != 0, "scheduler not initialized");

    let slot = scheduler
        .tasks
        .iter()
        .position(|task| task.state == TaskState::Empty)
        .ok_or(SpawnError::Full)?;

    let id = TaskId(NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed));
    scheduler.tasks[slot].initialize(slot, id, name, entry, priority);
    scheduler.task_count += 1;
    Ok(id)
}

pub fn set_yield_trace_enabled(enabled: bool) {
    YIELD_TRACE_ENABLED.store(enabled, Ordering::Release);
    if enabled {
        YIELD_TRACE_SEQUENCE.store(0, Ordering::Release);
    }
}

pub fn yield_now() {
    let trace = YIELD_TRACE_ENABLED.load(Ordering::Acquire);
    let sequence = if trace {
        YIELD_TRACE_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1
    } else {
        0
    };
    let cpu = crate::smp::cpu_index();

    if trace {
        crate::serial::write_line(format_args!(
            "[YIELD-TRACE] seq={} cpu={} phase=A enter",
            sequence, cpu
        ));
    }

    let interrupts_were_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();

    if trace {
        crate::serial::write_line(format_args!(
            "[YIELD-TRACE] seq={} cpu={} phase=B interrupts-disabled was_enabled={}",
            sequence, cpu, interrupts_were_enabled
        ));
        crate::serial::write_line(format_args!(
            "[YIELD-TRACE] seq={} cpu={} phase=C scheduler-lock-wait",
            sequence, cpu
        ));
    }

    let switch = {
        let mut scheduler = SCHEDULER.lock();
        if trace {
            let slot = scheduler.current_slot[cpu];
            let task = &scheduler.tasks[slot];
            crate::serial::write_line(format_args!(
                "[YIELD-TRACE] seq={} cpu={} phase=D lock-acquired current_slot={} current_id={} current_name={} current_state={} switching_out={}",
                sequence,
                cpu,
                slot,
                task.id.as_u64(),
                task.name,
                task.state.name(),
                scheduler.switching_out[cpu]
            ));
        }

        assert!(scheduler.task_count != 0, "scheduler not initialized");
        let switch = scheduler.prepare_switch();

        if trace {
            let slot = scheduler.current_slot[cpu];
            let task = &scheduler.tasks[slot];
            crate::serial::write_line(format_args!(
                "[YIELD-TRACE] seq={} cpu={} phase=E prepared switch={} selected_slot={} selected_id={} selected_name={} selected_state={} switching_out={}",
                sequence,
                cpu,
                switch.is_some(),
                slot,
                task.id.as_u64(),
                task.name,
                task.state.name(),
                scheduler.switching_out[cpu]
            ));
        }
        switch
    };

    if let Some(context_switch) = switch {
        if trace {
            crate::serial::write_line(format_args!(
                "[YIELD-TRACE] seq={} cpu={} phase=F switch-enter next_rsp={:#x}",
                sequence, cpu, context_switch.next_rsp
            ));
        }

        // SAFETY: Interrupts stay disabled between publishing scheduler state
        // and switching stacks, so an IRQ cannot save a context into the wrong
        // task control block.
        unsafe { switch_stacks(context_switch) };

        if trace {
            crate::serial::write_line(format_args!(
                "[YIELD-TRACE] seq={} cpu={} phase=G switch-return",
                sequence, cpu
            ));
        }
    } else if trace {
        crate::serial::write_line(format_args!(
            "[YIELD-TRACE] seq={} cpu={} phase=G no-switch",
            sequence, cpu
        ));
    }

    if interrupts_were_enabled {
        x86_64::instructions::interrupts::enable();
    }

    if trace {
        crate::serial::write_line(format_args!(
            "[YIELD-TRACE] seq={} cpu={} phase=H exit",
            sequence, cpu
        ));
    }
}

pub fn tick() {
    let Some(mut scheduler) = SCHEDULER.try_lock() else {
        PREEMPTION_REQUESTED[crate::smp::cpu_index()].store(true, Ordering::Release);
        return;
    };
    scheduler.wake_sleeping(timer::ticks());
    let slot = scheduler.current_slot[crate::smp::cpu_index()];
    let task = &mut scheduler.tasks[slot];
    if task.state != TaskState::Running {
        return;
    }
    if task.remaining_ticks > 1 {
        task.remaining_ticks -= 1;
    } else {
        task.remaining_ticks = 0;
        PREEMPTION_REQUESTED[crate::smp::cpu_index()].store(true, Ordering::Release);
    }
}
pub fn preempt_from_interrupt() {
    let cpu = crate::smp::cpu_index();
    if FILE_IO_DEPTH[cpu].load(Ordering::Acquire) != 0 {
        return;
    }
    if !PREEMPTION_REQUESTED[cpu].swap(false, Ordering::AcqRel) {
        return;
    }

    let switch = {
        let Some(mut scheduler) = SCHEDULER.try_lock() else {
            PREEMPTION_REQUESTED[cpu].store(true, Ordering::Release);
            return;
        };
        scheduler.wake_sleeping(timer::ticks());
        scheduler.prepare_switch()
    };

    if let Some(context_switch) = switch {
        PREEMPTION_SWITCHES.fetch_add(1, Ordering::Relaxed);
        // SAFETY: Hardware interrupts are disabled by the interrupt gate. The
        // scheduler lock is released, and both contexts belong to live tasks.
        unsafe { switch_stacks(context_switch) };
    }
}

pub fn preemption_point() {
    let cpu = crate::smp::cpu_index();
    // Consume the request before yielding. If another task/interrupt requests a
    // reschedule while this task is switched out, that newer request remains
    // set instead of being accidentally cleared when this task resumes.
    if PREEMPTION_REQUESTED[cpu].swap(false, Ordering::AcqRel) {
        yield_now();
    }
}

pub fn block_current() {
    let interrupts_were_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();

    let switch = {
        let mut scheduler = SCHEDULER.lock();
        let slot = scheduler.current_slot[crate::smp::cpu_index()];
        assert!(
            scheduler.tasks[slot].id != KERNEL_TASK_ID,
            "kernel task cannot block"
        );
        scheduler.tasks[slot].state = TaskState::Blocked;
        scheduler.prepare_switch()
    };

    let context_switch = switch.expect(
        "blocked current task has no runnable replacement; scheduler metadata would diverge from physical execution",
    );
    // SAFETY: The blocked task remains allocated and can only run again
    // after an explicit wakeup changes its state back to ready.
    unsafe { switch_stacks(context_switch) };

    if interrupts_were_enabled {
        x86_64::instructions::interrupts::enable();
    }
}

pub fn wake_task(id: TaskId) -> bool {
    let cpu = {
        let mut scheduler = SCHEDULER.lock();
        let Some(task) = scheduler.tasks.iter_mut().find(|task| {
            task.id == id && matches!(task.state, TaskState::Blocked | TaskState::Sleeping)
        }) else {
            return false;
        };

        task.state = TaskState::Ready;
        task.wake_tick = 0;
        task.event_deadline = None;
        task.cpu
    };
    crate::smp::reschedule_cpu(cpu);
    true
}

/// Stage 8.3 scheduler-latched event signal.
///
/// Unlike `wake_task`, this also succeeds when the target has registered an
/// event wait but has not yet published `Blocked`. In that case the signal is
/// latched in the TCB. `wait_for_event` consumes the latch atomically under the
/// scheduler lock instead of sleeping, so a wake cannot be lost between an
/// IPC queue check and the scheduler transition.
pub fn signal_event(id: TaskId) -> bool {
    let target = {
        let mut scheduler = SCHEDULER.lock();
        let Some(task) = scheduler
            .tasks
            .iter_mut()
            .find(|task| task.id == id && task.state != TaskState::Empty)
        else {
            return false;
        };

        if task.state == TaskState::Blocked {
            task.state = TaskState::Ready;
            task.wake_tick = 0;
            task.event_deadline = None;
            Some(task.cpu)
        } else {
            task.event_pending = true;
            None
        }
    };

    if let Some(cpu) = target {
        crate::smp::reschedule_cpu(cpu);
    }
    true
}

/// Wait for one scheduler-latched event.
///
/// If a matching signal arrived just before this call, the pending permit is
/// consumed and the task never blocks. Otherwise the Running -> Blocked state
/// transition is published while holding the scheduler lock. A later
/// `signal_event` therefore observes either the pending pre-block window or the
/// Blocked state; there is no third state in which a wake can disappear.
pub fn wait_for_event() {
    wait_for_event_until(u64::MAX);
}

/// Absolute monotonic tick deadline; u64::MAX means no timeout. Deadline and
/// event checks share the scheduler lock with wake publication.
pub fn wait_for_event_until(deadline: u64) {
    let interrupts_were_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();

    let switch = {
        let mut scheduler = SCHEDULER.lock();
        let slot = scheduler.current_slot[crate::smp::cpu_index()];
        assert!(
            scheduler.tasks[slot].id != KERNEL_TASK_ID,
            "kernel task cannot wait for an event"
        );

        if scheduler.tasks[slot].event_pending {
            scheduler.tasks[slot].event_pending = false;
            None
        } else if deadline != u64::MAX && timer::ticks() >= deadline {
            None
        } else {
            scheduler.tasks[slot].state = TaskState::Blocked;
            scheduler.tasks[slot].event_deadline = (deadline != u64::MAX).then_some(deadline);
            Some(scheduler.prepare_switch().expect(
                "event-waiting task has no runnable replacement; scheduler metadata would diverge from physical execution",
            ))
        }
    };

    if let Some(context_switch) = switch {
        // Keep IF masked until the GDT/CR3/stack hand-off is complete. The
        // low-level context switch now saves/restores RFLAGS, so the incoming
        // task receives its own interrupt state instead of inheriting ours.
        // SAFETY: the task remains allocated while Blocked and can only become
        // runnable through the scheduler-owned wake paths.
        unsafe { switch_stacks(context_switch) };
    }

    if interrupts_were_enabled {
        x86_64::instructions::interrupts::enable();
    }
}

pub fn sleep_current(ticks: u64) {
    let interrupts_were_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();

    let switch = {
        let mut scheduler = SCHEDULER.lock();
        let slot = scheduler.current_slot[crate::smp::cpu_index()];
        assert!(
            scheduler.tasks[slot].id != KERNEL_TASK_ID,
            "kernel task cannot sleep"
        );
        scheduler.tasks[slot].wake_tick = timer::ticks().saturating_add(ticks.max(1));
        scheduler.tasks[slot].state = TaskState::Sleeping;
        scheduler.prepare_switch()
    };

    let context_switch = switch.expect(
        "sleeping current task has no runnable replacement; scheduler metadata would diverge from physical execution",
    );
    // Keep IF masked through the scheduler/GDT hand-off. `switch_stacks`
    // restores the incoming task's saved RFLAGS, so timer-capable tasks no
    // longer inherit the sleeper's syscall-gate IF=0 state.
    // SAFETY: The sleeping task remains allocated and will only become
    // ready after its wake deadline is observed by the timer path.
    unsafe { switch_stacks(context_switch) };

    if interrupts_were_enabled {
        x86_64::instructions::interrupts::enable();
    }
}

pub fn exit_current_task() -> ! {
    x86_64::instructions::interrupts::disable();
    let switch = {
        let mut scheduler = SCHEDULER.lock();
        let slot = scheduler.current_slot[crate::smp::cpu_index()];
        assert!(
            scheduler.tasks[slot].id != KERNEL_TASK_ID,
            "kernel task cannot exit"
        );
        scheduler.tasks[slot].state = TaskState::Dead;
        scheduler.task_count = scheduler.task_count.saturating_sub(1);
        scheduler.prepare_switch()
    };

    if let Some(context_switch) = switch {
        // SAFETY: The exiting task will never be selected again, and the next
        // context belongs to a live task selected by the scheduler.
        unsafe { switch_stacks(context_switch) };
    }

    loop {
        x86_64::instructions::hlt();
    }
}

pub fn summary() -> Summary {
    let scheduler = SCHEDULER.lock();
    assert!(scheduler.task_count != 0, "scheduler not initialized");
    scheduler.summary()
}

pub fn current_task_id() -> TaskId {
    let scheduler = SCHEDULER.lock();
    assert!(scheduler.task_count != 0, "scheduler not initialized");
    scheduler.tasks[scheduler.current_slot[crate::smp::cpu_index()]].id
}

pub fn current_task_id_if_running() -> Option<TaskId> {
    let scheduler = SCHEDULER.lock();
    if scheduler.task_count == 0 {
        return None;
    }
    Some(scheduler.tasks[scheduler.current_slot[crate::smp::cpu_index()]].id)
}

fn task_effective_has(task: &TaskControlBlock, capability: Capability) -> bool {
    if !task.capabilities.contains(capability) || !task.sandbox_profile.allows(capability) {
        return false;
    }
    match task.capability_lineages[capability.index()] {
        None => true,
        Some(lineage) => wovenguard::lineage_authorizes(lineage, capability),
    }
}

fn task_effective_capabilities(task: &TaskControlBlock) -> CapabilitySet {
    let mut effective = CapabilitySet::empty();
    for capability in Capability::ALL {
        if task_effective_has(task, capability) {
            effective = effective.with(capability);
        }
    }
    effective
}

pub fn authorize_current_device(class: wovenguard::DeviceClass) -> bool {
    let actor = current_process_id();
    let (allowed, capability) = {
        let scheduler = SCHEDULER.lock();
        assert!(scheduler.task_count != 0, "scheduler not initialized");
        let task = &scheduler.tasks[scheduler.current_slot[crate::smp::cpu_index()]];
        let authority = task_effective_capabilities(task);
        let decision = wovenguard::authorize_device_access(authority, task.sandbox_profile, class);
        (decision.allowed, wovenguard::device_capability(class))
    };
    crate::audit::record_detail(
        actor,
        crate::audit::Action::SandboxDeviceAccess,
        class as u64,
        capability as u64,
        allowed,
    );
    allowed
}

pub fn current_has(capability: Capability) -> bool {
    let scheduler = SCHEDULER.lock();
    assert!(scheduler.task_count != 0, "scheduler not initialized");
    task_effective_has(
        &scheduler.tasks[scheduler.current_slot[crate::smp::cpu_index()]],
        capability,
    )
}

pub fn grant(target: TaskId, capability: Capability) -> Result<(), CapabilityError> {
    let audit_actor = current_process_id();
    let (result, decision) = {
        let mut scheduler = SCHEDULER.lock();
        assert!(scheduler.task_count != 0, "scheduler not initialized");

        let actor_index = scheduler.current_slot[crate::smp::cpu_index()];
        let actor_domain = scheduler.tasks[actor_index].security_domain;
        let authority = task_effective_capabilities(&scheduler.tasks[actor_index]);
        let actor_task_id = scheduler.tasks[actor_index].id;
        let Some(target_index) = scheduler
            .tasks
            .iter()
            .position(|task| task.state != TaskState::Empty && task.id == target)
        else {
            drop(scheduler);
            crate::audit::record(audit_actor, crate::audit::Action::WovenGuardDeny, target.as_u64(), false);
            crate::audit::record(audit_actor, crate::audit::Action::CapabilityGrant, target.as_u64(), false);
            return Err(CapabilityError::UnknownTask);
        };
        let target_domain = scheduler.tasks[target_index].security_domain;
        let target_sandbox = scheduler.tasks[target_index].sandbox_profile;
        let decision = wovenguard::authorize_grant(
            actor_domain,
            authority,
            target_domain,
            target_sandbox,
            capability,
        );
        if !decision.allowed {
            (Err(CapabilityError::PermissionDenied), decision)
        } else if task_effective_has(&scheduler.tasks[target_index], capability)
            && scheduler.tasks[target_index].capability_lineages[capability.index()].is_none()
        {
            // Do not replace intrinsic authority with a revocable child token.
            (Ok(()), decision)
        } else {
            let parent_lineage = match scheduler.tasks[actor_index].capability_lineages[capability.index()] {
                Some(lineage) if wovenguard::lineage_authorizes(lineage, capability) => lineage,
                _ => {
                    match wovenguard::issue_lineage_root(
                        actor_task_id.as_u64(),
                        CapabilitySet::only(capability),
                    ) {
                        Ok(lineage) => {
                            scheduler.tasks[actor_index].capability_lineages[capability.index()] = Some(lineage);
                            lineage
                        }
                        Err(_) => return Err(CapabilityError::PermissionDenied),
                    }
                }
            };
            match wovenguard::derive_lineage(
                actor_task_id.as_u64(),
                parent_lineage,
                target.as_u64(),
                CapabilitySet::only(capability),
            ) {
                Ok(child) => {
                    scheduler.tasks[target_index].capabilities =
                        scheduler.tasks[target_index].capabilities.with(capability);
                    scheduler.tasks[target_index].capability_lineages[capability.index()] = Some(child);
                    (Ok(()), decision)
                }
                Err(_) => (Err(CapabilityError::PermissionDenied), decision),
            }
        }
    };
    let _ = decision.reason;
    crate::audit::record(
        audit_actor,
        if result.is_ok() { crate::audit::Action::WovenGuardAllow } else { crate::audit::Action::WovenGuardDeny },
        target.as_u64(),
        result.is_ok(),
    );
    crate::audit::record(audit_actor, crate::audit::Action::CapabilityGrant, target.as_u64(), result.is_ok());
    result
}

/// Bind a Stage 9.4A sandbox profile to a task. Tightening is destructive:
/// raw/intrinsic authority and delegated lineage bindings above the new ceiling
/// are cleared so a later profile change cannot resurrect hidden authority.
pub fn bind_sandbox_profile(
    target: TaskId,
    profile: wovenguard::SandboxProfile,
) -> Result<(), CapabilityError> {
    let audit_actor = current_process_id();
    let result = {
        let mut scheduler = SCHEDULER.lock();
        assert!(scheduler.task_count != 0, "scheduler not initialized");
        let actor_index = scheduler.current_slot[crate::smp::cpu_index()];
        let actor_domain = scheduler.tasks[actor_index].security_domain;
        let authority = task_effective_capabilities(&scheduler.tasks[actor_index]);
        let controller = scheduler.tasks[actor_index].id.as_u64();
        let Some(target_index) = scheduler
            .tasks
            .iter()
            .position(|task| task.state != TaskState::Empty && task.id == target)
        else {
            return Err(CapabilityError::UnknownTask);
        };
        let target_domain = scheduler.tasks[target_index].security_domain;
        let decision = wovenguard::authorize_sandbox_bind(
            actor_domain,
            authority,
            target_domain,
            profile,
        );
        if !decision.allowed {
            Err(CapabilityError::PermissionDenied)
        } else {
            for capability in Capability::ALL {
                if profile.allows(capability) {
                    continue;
                }
                if let Some(lineage) = scheduler.tasks[target_index].capability_lineages[capability.index()] {
                    let _ = wovenguard::revoke_lineage_subtree_by_controller(controller, lineage);
                }
                scheduler.tasks[target_index].capabilities =
                    scheduler.tasks[target_index].capabilities.without(capability);
                scheduler.tasks[target_index].capability_lineages[capability.index()] = None;
            }
            scheduler.tasks[target_index].sandbox_profile = profile;
            Ok(())
        }
    };
    crate::audit::record_detail(
        audit_actor,
        if result.is_ok() {
            crate::audit::Action::SandboxProfileBind
        } else {
            crate::audit::Action::SandboxProfileDeny
        },
        target.as_u64(),
        profile.id() as u64,
        result.is_ok(),
    );
    result
}

pub fn task_sandbox_profile(target: TaskId) -> Option<wovenguard::SandboxProfile> {
    let scheduler = SCHEDULER.lock();
    scheduler
        .tasks
        .iter()
        .find(|task| task.state != TaskState::Empty && task.id == target)
        .map(|task| task.sandbox_profile)
}

/// Resolve a live process ID to its TCB-bound sandbox profile without exposing
/// scheduler internals to IPC. Synthetic kernel-only IPC test owners have no
/// TCB and therefore return `None`.
pub fn sandbox_profile_for_process(process_id: u64) -> Option<wovenguard::SandboxProfile> {
    let scheduler = SCHEDULER.lock();
    scheduler
        .tasks
        .iter()
        .find(|task| {
            task.state != TaskState::Empty && task.id.as_u64() == process_id
        })
        .map(|task| task.sandbox_profile)
}

fn current_file_access_allowed(
    capability: Capability,
    scope: wovenguard::FileScope,
    write: bool,
) -> bool {
    let scheduler = SCHEDULER.lock();
    if scheduler.task_count == 0 {
        return false;
    }
    let task = &scheduler.tasks[scheduler.current_slot[crate::smp::cpu_index()]];
    if !task_effective_has(task, capability) {
        return false;
    }
    if write {
        task.sandbox_profile.allows_file_write(scope)
    } else {
        task.sandbox_profile.allows_file_read(scope)
    }
}

fn current_file_authority() -> Option<(bool, bool, wovenguard::SandboxProfile)> {
    let scheduler = SCHEDULER.lock();
    if scheduler.task_count == 0 {
        return None;
    }
    let task = &scheduler.tasks[scheduler.current_slot[crate::smp::cpu_index()]];
    Some((
        task_effective_has(task, Capability::FileRead),
        task_effective_has(task, Capability::FileWrite),
        task.sandbox_profile,
    ))
}

fn file_path_hash(path: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in path.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

fn sandbox_file_access_for_task(target: TaskId, path: &str, write: bool) -> bool {
    let scope = wovenguard::classify_file_path(path);
    let profile = task_sandbox_profile(target);
    let allowed = profile.is_some_and(|profile| {
        if write {
            profile.allows_file_write(scope)
        } else {
            profile.allows_file_read(scope)
        }
    });
    crate::audit::record_detail(
        target.as_u64(),
        if write {
            crate::audit::Action::SandboxFileWrite
        } else {
            crate::audit::Action::SandboxFileRead
        },
        scope as u64,
        file_path_hash(path),
        allowed,
    );
    allowed
}

fn authorize_current_file_path(path: &str, write: bool) -> bool {
    let scope = wovenguard::classify_file_path(path);
    let allowed = current_file_access_allowed(
        if write { Capability::FileWrite } else { Capability::FileRead },
        scope,
        write,
    );
    let actor = current_process_id();
    crate::audit::record_detail(
        actor,
        if write {
            crate::audit::Action::SandboxFileWrite
        } else {
            crate::audit::Action::SandboxFileRead
        },
        scope as u64,
        file_path_hash(path),
        allowed,
    );
    allowed
}

pub fn revoke(target: TaskId, capability: Capability) -> Result<(), CapabilityError> {
    let audit_actor = current_process_id();
    let (result, decision) = {
        let mut scheduler = SCHEDULER.lock();
        assert!(scheduler.task_count != 0, "scheduler not initialized");

        let actor_index = scheduler.current_slot[crate::smp::cpu_index()];
        let authority = task_effective_capabilities(&scheduler.tasks[actor_index]);
        let controller = scheduler.tasks[actor_index].id.as_u64();
        let decision = wovenguard::authorize_revoke(authority);
        if !decision.allowed {
            (Err(CapabilityError::PermissionDenied), decision)
        } else if let Some(target_index) = scheduler
            .tasks
            .iter()
            .position(|task| task.state != TaskState::Empty && task.id == target)
        {
            if let Some(lineage) = scheduler.tasks[target_index].capability_lineages[capability.index()] {
                let parent = wovenguard::lineage_info(lineage).ok().and_then(|info| info.parent);
                let _ = wovenguard::revoke_lineage_subtree_by_controller(controller, lineage);

                // Lazily-created intrinsic roots exist only to give one task
                // a provenance anchor for delegation. Once their last child is
                // gone, release the empty root and restore the actor's intrinsic
                // binding to `None`; this keeps the bounded lineage table from
                // accumulating one permanent root per exercised capability.
                if let Some(parent) = parent {
                    let releasable = wovenguard::lineage_info(parent).is_ok_and(|info| {
                        info.owner == controller && info.parent.is_none() && info.child_count == 0
                    });
                    if releasable
                        && wovenguard::release_lineage(controller, parent).is_ok()
                        && scheduler.tasks[actor_index].capability_lineages[capability.index()]
                            == Some(parent)
                    {
                        scheduler.tasks[actor_index].capability_lineages[capability.index()] = None;
                    }
                }
            }
            scheduler.tasks[target_index].capabilities =
                scheduler.tasks[target_index].capabilities.without(capability);
            scheduler.tasks[target_index].capability_lineages[capability.index()] = None;
            (Ok(()), decision)
        } else {
            (Err(CapabilityError::UnknownTask), decision)
        }
    };
    let _ = decision.reason;
    crate::audit::record(
        audit_actor,
        if result.is_ok() { crate::audit::Action::WovenGuardAllow } else { crate::audit::Action::WovenGuardDeny },
        target.as_u64(),
        result.is_ok(),
    );
    crate::audit::record(audit_actor, crate::audit::Action::CapabilityRevoke, target.as_u64(), result.is_ok());
    result
}

fn task_has(task_id: TaskId, capability: Capability) -> bool {
    SCHEDULER
        .lock()
        .tasks
        .iter()
        .find(|task| task.state != TaskState::Empty && task.id == task_id)
        .is_some_and(|task| task_effective_has(task, capability))
}

/// Stage 9.4C production proof for filesystem/resource scope policy.
/// The idle task is safe to use as a policy subject because its Restricted
/// domain cannot gain FileRead/FileWrite capabilities; this probe validates
/// the TCB-bound scope policy itself, profile tightening, path classification,
/// and audit denial without mutating live kernel filesystem authority.
pub fn filesystem_sandbox_foundation_valid() -> bool {
    let baseline = task_sandbox_profile(IDLE_TASK_ID);
    if baseline != Some(wovenguard::SandboxProfile::RESTRICTED) {
        return false;
    }

    let read_mask = wovenguard::FilePolicy::scope_mask(wovenguard::FileScope::System)
        | wovenguard::FilePolicy::scope_mask(wovenguard::FileScope::Temporary);
    let write_mask = wovenguard::FilePolicy::scope_mask(wovenguard::FileScope::Temporary);
    let scoped = wovenguard::SandboxProfile::new(
        0x94c0,
        CapabilitySet::only(Capability::TimerRead),
    )
    .with_file_policy(wovenguard::FilePolicy::scoped(read_mask, write_mask));

    if bind_sandbox_profile(IDLE_TASK_ID, scoped).is_err()
        || !sandbox_file_access_for_task(IDLE_TASK_ID, "/etc/motd", false)
        || sandbox_file_access_for_task(IDLE_TASK_ID, "/etc/motd", true)
        || !sandbox_file_access_for_task(IDLE_TASK_ID, "/tmp/stage94c", false)
        || !sandbox_file_access_for_task(IDLE_TASK_ID, "/tmp/stage94c", true)
        || sandbox_file_access_for_task(IDLE_TASK_ID, "/mnt/private", false)
    {
        let _ = bind_sandbox_profile(IDLE_TASK_ID, wovenguard::SandboxProfile::RESTRICTED);
        return false;
    }

    let tightened = wovenguard::SandboxProfile::new(
        0x94c1,
        CapabilitySet::only(Capability::TimerRead),
    )
    .with_file_policy(wovenguard::FilePolicy::NONE);
    if bind_sandbox_profile(IDLE_TASK_ID, tightened).is_err()
        || sandbox_file_access_for_task(IDLE_TASK_ID, "/tmp/stage94c", false)
        || !crate::audit::latest().is_some_and(|event| {
            event.actor == IDLE_TASK_ID.as_u64()
                && event.action == crate::audit::Action::SandboxFileRead
                && !event.allowed
        })
    {
        let _ = bind_sandbox_profile(IDLE_TASK_ID, wovenguard::SandboxProfile::RESTRICTED);
        return false;
    }

    bind_sandbox_profile(IDLE_TASK_ID, wovenguard::SandboxProfile::RESTRICTED).is_ok()
        && task_sandbox_profile(IDLE_TASK_ID) == baseline
        && wovenguard::classify_file_path("/") == wovenguard::FileScope::Root
        && wovenguard::classify_file_path("/bin/sh") == wovenguard::FileScope::System
        && wovenguard::classify_file_path("/home/user/a") == wovenguard::FileScope::Home
}

fn stage9_4d_fail(cpu: usize, code: u64) {
    let _ = STAGE9_4D_FAIL[cpu].compare_exchange(0, code, Ordering::AcqRel, Ordering::Acquire);
}

fn stage9_4d_worker_task() -> ! {
    let cpu = crate::smp::cpu_index();
    let me = TaskId::from_u64(current_process_id());
    STAGE9_4D_CPU_MASK.fetch_or(1u64 << cpu, Ordering::AcqRel);
    STAGE9_4D_READY.fetch_add(1, Ordering::AcqRel);

    while STAGE9_4D_PHASE.load(Ordering::Acquire) < 1 {
        yield_now();
    }
    let expected_scoped = wovenguard::SandboxProfile::new(
        0x94d0 + cpu as u32,
        CapabilitySet::only(Capability::TimerRead),
    )
    .with_service_policy(wovenguard::ServicePolicy::only(
        wovenguard::ServiceClass::Core,
        wovenguard::ServiceClass::Core,
    ))
    .with_file_policy(wovenguard::FilePolicy::scoped(
        wovenguard::FilePolicy::scope_mask(wovenguard::FileScope::Temporary),
        0,
    ));
    if task_sandbox_profile(me) != Some(expected_scoped)
        || task_has(me, Capability::DeviceIo)
        || !task_has(me, Capability::TimerRead)
    {
        stage9_4d_fail(cpu, 1);
    } else {
        STAGE9_4D_PHASE1_OK.fetch_add(1, Ordering::AcqRel);
    }

    while STAGE9_4D_PHASE.load(Ordering::Acquire) < 2 {
        yield_now();
    }
    let expected_tight = wovenguard::SandboxProfile::new(
        0x94e0 + cpu as u32,
        CapabilitySet::only(Capability::TimerRead),
    )
    .with_service_policy(wovenguard::ServicePolicy::NONE)
    .with_file_policy(wovenguard::FilePolicy::NONE);
    if task_sandbox_profile(me) != Some(expected_tight)
        || task_has(me, Capability::DeviceIo)
        || !task_has(me, Capability::TimerRead)
    {
        stage9_4d_fail(cpu, 2);
    } else {
        STAGE9_4D_PHASE2_OK.fetch_add(1, Ordering::AcqRel);
    }

    while STAGE9_4D_PHASE.load(Ordering::Acquire) < 3 {
        yield_now();
    }
    // Re-broadening the profile must not resurrect DeviceIo that the first
    // destructive tightening removed from the TCB.
    if task_sandbox_profile(me) != Some(wovenguard::SandboxProfile::SYSTEM_SERVICE)
        || task_has(me, Capability::DeviceIo)
        || !task_has(me, Capability::TimerRead)
    {
        stage9_4d_fail(cpu, 3);
    }

    STAGE9_4D_DONE.fetch_add(1, Ordering::Release);
    exit_current_task();
}

/// Stage 9.4D production closure proof. One live task is pinned to every online
/// CPU. The BSP repeatedly changes each task's sandbox through the same global
/// scheduler lock that readers use, making profile replacement the SMP
/// linearization point. Workers prove post-tightening visibility, destructive
/// capability stripping, a second service/file-policy tightening, and the
/// non-resurrection rule when the profile is later broadened again within the worker's SystemService domain ceiling.
pub fn sandbox_lifecycle_smp_closure_valid() -> bool {
    let online = crate::smp::online_count();
    if online == 0 || online > crate::smp::MAX_CPUS {
        return false;
    }

    STAGE9_4D_PHASE.store(0, Ordering::Release);
    STAGE9_4D_READY.store(0, Ordering::Release);
    STAGE9_4D_PHASE1_OK.store(0, Ordering::Release);
    STAGE9_4D_PHASE2_OK.store(0, Ordering::Release);
    STAGE9_4D_DONE.store(0, Ordering::Release);
    STAGE9_4D_CPU_MASK.store(0, Ordering::Release);
    for fail in &STAGE9_4D_FAIL {
        fail.store(0, Ordering::Release);
    }

    let mut ids = [TaskId::from_u64(0); crate::smp::MAX_CPUS];
    for (cpu, slot) in ids.iter_mut().enumerate().take(online) {
        // SAFETY: the worker uses only atomics, scheduler-owned sandbox/capability
        // queries, yield, and exit; all are already SMP-safe kernel-task services.
        let Ok(id) = (unsafe { spawn_on(cpu, "s9.4d-sandbox-worker", stage9_4d_worker_task) }) else {
            return false;
        };
        *slot = id;
    }

    let ready_deadline = timer::ticks().saturating_add(1024);
    while STAGE9_4D_READY.load(Ordering::Acquire) < online as u64 {
        if timer::ticks() >= ready_deadline {
            return false;
        }
        yield_now();
    }

    for (cpu, id) in ids.iter().copied().enumerate().take(online) {
        // Kernel workers begin with an empty capability set. Delegate two live
        // authorities first so the closure proves that tightening removes a
        // real lineage-backed DeviceIo grant while preserving TimerRead.
        if grant(id, Capability::TimerRead).is_err()
            || grant(id, Capability::DeviceIo).is_err()
        {
            return false;
        }
        let scoped = wovenguard::SandboxProfile::new(
            0x94d0 + cpu as u32,
            CapabilitySet::only(Capability::TimerRead),
        )
        .with_service_policy(wovenguard::ServicePolicy::only(
            wovenguard::ServiceClass::Core,
            wovenguard::ServiceClass::Core,
        ))
        .with_file_policy(wovenguard::FilePolicy::scoped(
            wovenguard::FilePolicy::scope_mask(wovenguard::FileScope::Temporary),
            0,
        ));
        if bind_sandbox_profile(id, scoped).is_err() {
            return false;
        }
    }
    STAGE9_4D_PHASE.store(1, Ordering::Release);

    let phase1_deadline = timer::ticks().saturating_add(1024);
    while STAGE9_4D_PHASE1_OK.load(Ordering::Acquire) < online as u64 {
        if STAGE9_4D_FAIL.iter().take(online).any(|f| f.load(Ordering::Acquire) != 0)
            || timer::ticks() >= phase1_deadline
        {
            return false;
        }
        yield_now();
    }

    for (cpu, id) in ids.iter().copied().enumerate().take(online) {
        let tightened = wovenguard::SandboxProfile::new(
            0x94e0 + cpu as u32,
            CapabilitySet::only(Capability::TimerRead),
        )
        .with_service_policy(wovenguard::ServicePolicy::NONE)
        .with_file_policy(wovenguard::FilePolicy::NONE);
        if bind_sandbox_profile(id, tightened).is_err() {
            return false;
        }
    }
    STAGE9_4D_PHASE.store(2, Ordering::Release);

    let phase2_deadline = timer::ticks().saturating_add(1024);
    while STAGE9_4D_PHASE2_OK.load(Ordering::Acquire) < online as u64 {
        if STAGE9_4D_FAIL.iter().take(online).any(|f| f.load(Ordering::Acquire) != 0)
            || timer::ticks() >= phase2_deadline
        {
            return false;
        }
        yield_now();
    }

    for id in ids.iter().copied().take(online) {
        if bind_sandbox_profile(id, wovenguard::SandboxProfile::SYSTEM_SERVICE).is_err() {
            return false;
        }
    }
    STAGE9_4D_PHASE.store(3, Ordering::Release);

    let done_deadline = timer::ticks().saturating_add(2048);
    while STAGE9_4D_DONE.load(Ordering::Acquire) < online as u64 {
        if timer::ticks() >= done_deadline {
            return false;
        }
        yield_now();
    }

    STAGE9_4D_CPU_MASK.load(Ordering::Acquire) == (1u64 << online) - 1
        && STAGE9_4D_PHASE1_OK.load(Ordering::Acquire) == online as u64
        && STAGE9_4D_PHASE2_OK.load(Ordering::Acquire) == online as u64
        && STAGE9_4D_FAIL.iter().take(online).all(|f| f.load(Ordering::Acquire) == 0)
}

/// Stage 9.5 production proof for class-specific resource/device gates.
/// This proves domain ceilings, capability decomposition and sandbox device
/// masks without mutating the validated bootstrap task authority.
pub fn device_capability_gates_valid() -> bool {
    let kernel = CapabilitySet::kernel_bootstrap();
    let user = CapabilitySet::userspace();
    let system = wovenguard::domain_ceiling(wovenguard::SecurityDomain::SystemService);

    let user_network = wovenguard::authorize_device_access(
        user,
        wovenguard::SandboxProfile::USER_DEFAULT,
        wovenguard::DeviceClass::Network,
    ).allowed;
    let user_storage_denied = !wovenguard::authorize_device_access(
        user,
        wovenguard::SandboxProfile::USER_DEFAULT,
        wovenguard::DeviceClass::Storage,
    ).allowed;
    let system_storage = wovenguard::authorize_device_access(
        system,
        wovenguard::SandboxProfile::SYSTEM_SERVICE,
        wovenguard::DeviceClass::Storage,
    ).allowed;
    let system_display = wovenguard::authorize_device_access(
        system,
        wovenguard::SandboxProfile::SYSTEM_SERVICE,
        wovenguard::DeviceClass::Display,
    ).allowed;
    let system_input = wovenguard::authorize_device_access(
        system,
        wovenguard::SandboxProfile::SYSTEM_SERVICE,
        wovenguard::DeviceClass::Input,
    ).allowed;
    let masked_storage_denied = !wovenguard::authorize_device_access(
        kernel,
        wovenguard::SandboxProfile::KERNEL_TRUSTED.with_device_policy(
            wovenguard::DevicePolicy::only(wovenguard::DeviceClass::Network),
        ),
        wovenguard::DeviceClass::Storage,
    ).allowed;
    let masked_network_denied = !wovenguard::authorize_device_access(
        kernel,
        wovenguard::SandboxProfile::KERNEL_TRUSTED
            .with_device_policy(wovenguard::DevicePolicy::NONE),
        wovenguard::DeviceClass::Network,
    ).allowed;
    let legacy_generic = wovenguard::authorize_device_access(
        kernel,
        wovenguard::SandboxProfile::KERNEL_TRUSTED,
        wovenguard::DeviceClass::Generic,
    ).allowed;

    user_network
        && user_storage_denied
        && system_storage
        && system_display
        && system_input
        && masked_storage_denied
        && masked_network_denied
        && legacy_generic
}

pub fn capability_policy_valid() -> bool {
    let scheduler = SCHEDULER.lock();
    assert!(scheduler.task_count >= 2, "bootstrap tasks are missing");

    let required = [
        Capability::Console,
        Capability::TimerRead,
        Capability::TaskInspect,
        Capability::TaskControl,
        Capability::DeviceIo,
        Capability::InterruptControl,
        Capability::MemoryInspect,
    ];

    required
        .iter()
        .all(|capability| scheduler.tasks[0].capabilities.contains(*capability))
        && required
            .iter()
            .all(|capability| !scheduler.tasks[1].capabilities.contains(*capability))
}

pub fn wovenguard_domain_policy_valid() -> bool {
    let scheduler = SCHEDULER.lock();
    assert!(scheduler.task_count >= 2, "bootstrap tasks are missing");

    scheduler.tasks[0].security_domain == wovenguard::SecurityDomain::Kernel
        && scheduler.tasks[1].security_domain == wovenguard::SecurityDomain::Restricted
        && scheduler.tasks[0]
            .capabilities
            .contains(Capability::TaskControl)
        && !scheduler.tasks[1]
            .capabilities
            .contains(Capability::TaskControl)
        && wovenguard::domain_allows(
            wovenguard::SecurityDomain::Restricted,
            Capability::TimerRead,
        )
        && !wovenguard::domain_allows(
            wovenguard::SecurityDomain::Restricted,
            Capability::DeviceIo,
        )
}

pub fn capability_delegation_valid() -> bool {
    let ceiling_denied = grant(IDLE_TASK_ID, Capability::DeviceIo).is_err();
    let no_escalation = !task_has(IDLE_TASK_ID, Capability::DeviceIo);
    let granted = grant(IDLE_TASK_ID, Capability::TimerRead).is_ok();
    let observed = task_has(IDLE_TASK_ID, Capability::TimerRead);
    let revoked = revoke(IDLE_TASK_ID, Capability::TimerRead).is_ok();
    let denied_after_revoke = !task_has(IDLE_TASK_ID, Capability::TimerRead);

    ceiling_denied && no_escalation && granted && observed && revoked && denied_after_revoke
}

/// Stage 9.2C proof that lineage metadata now gates real task authority.
/// A delegated capability must work while its lineage is live and stop working
/// immediately when that lineage is recursively revoked.
pub fn capability_lineage_enforcement_valid() -> bool {
    let baseline = wovenguard::lineage_count();
    if grant(IDLE_TASK_ID, Capability::TimerRead).is_err()
        || !task_has(IDLE_TASK_ID, Capability::TimerRead)
    {
        return false;
    }

    let (root, child) = {
        let scheduler = SCHEDULER.lock();
        let child = scheduler.tasks[1].capability_lineages[Capability::TimerRead.index()];
        let root = scheduler.tasks[0].capability_lineages[Capability::TimerRead.index()];
        (root, child)
    };
    let (Some(root), Some(child)) = (root, child) else {
        return false;
    };
    if !wovenguard::lineage_is_descendant_of(child, root) {
        return false;
    }

    // Revoke the delegator root, not merely the leaf. The descendant token
    // becomes stale atomically and the real task capability check must deny it.
    if wovenguard::revoke_lineage_subtree(KERNEL_TASK_ID.as_u64(), root) != Ok(2) {
        return false;
    }
    let denied_after_lineage_revoke = !task_has(IDLE_TASK_ID, Capability::TimerRead);

    // The test revoked the kernel's temporary provenance root as well. Restore
    // the bootstrap capability's intrinsic representation and remove the idle
    // task's stale delegated bit/binding so later acceptance starts clean.
    {
        let mut scheduler = SCHEDULER.lock();
        scheduler.tasks[0].capability_lineages[Capability::TimerRead.index()] = None;
        scheduler.tasks[1].capabilities = scheduler.tasks[1].capabilities.without(Capability::TimerRead);
        scheduler.tasks[1].capability_lineages[Capability::TimerRead.index()] = None;
    }

    denied_after_lineage_revoke
        && current_has(Capability::TimerRead)
        && wovenguard::lineage_count() == baseline
}

/// Stage 9.4A production proof for sandbox profile binding and capability
/// ceilings. It uses the real idle TCB and real grant/revoke path, then restores
/// the validated Restricted baseline before returning.
pub fn sandbox_profile_foundation_valid() -> bool {
    let baseline = task_sandbox_profile(IDLE_TASK_ID);
    if baseline != Some(wovenguard::SandboxProfile::RESTRICTED) {
        return false;
    }

    let narrow = wovenguard::SandboxProfile::new(0x94a0, CapabilitySet::empty());
    if bind_sandbox_profile(IDLE_TASK_ID, narrow).is_err()
        || task_sandbox_profile(IDLE_TASK_ID) != Some(narrow)
    {
        return false;
    }

    let denied = grant(IDLE_TASK_ID, Capability::TimerRead).is_err()
        && !task_has(IDLE_TASK_ID, Capability::TimerRead);

    let invalid_broaden = bind_sandbox_profile(
        IDLE_TASK_ID,
        wovenguard::SandboxProfile::USER_DEFAULT,
    )
    .is_err();

    if bind_sandbox_profile(IDLE_TASK_ID, wovenguard::SandboxProfile::RESTRICTED).is_err() {
        return false;
    }
    let restored_grant = grant(IDLE_TASK_ID, Capability::TimerRead).is_ok()
        && task_has(IDLE_TASK_ID, Capability::TimerRead);
    let restored_revoke = revoke(IDLE_TASK_ID, Capability::TimerRead).is_ok()
        && !task_has(IDLE_TASK_ID, Capability::TimerRead);

    denied
        && invalid_broaden
        && restored_grant
        && restored_revoke
        && task_sandbox_profile(IDLE_TASK_ID) == baseline
}

fn idle_task() -> ! {
    loop {
        IDLE_HEARTBEATS.fetch_add(1, Ordering::Relaxed);

        if crate::smp::offline_checkpoint(crate::smp::cpu_index()) {
            loop {
                x86_64::instructions::interrupts::disable();
                x86_64::instructions::hlt();
            }
        }

        // Consume any already-published scheduling request first. If none is
        // pending, park with STI+HLT as one architectural idle sequence. This
        // avoids the check-then-HLT window where an interrupt could arrive
        // after the check but before HLT and leave runnable work sleeping until
        // an unrelated later interrupt.
        preemption_point();
        x86_64::instructions::interrupts::disable();
        if PREEMPTION_REQUESTED[crate::smp::cpu_index()].load(Ordering::Acquire) {
            x86_64::instructions::interrupts::enable();
            continue;
        }
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

/// Push a value onto a task stack being prepared for its first context switch.
///
/// # Safety
///
/// `cursor` must point within a writable, properly aligned task stack and have
/// room for another eight-byte value.
unsafe fn push_stack_value(cursor: &mut usize, value: u64) {
    *cursor -= size_of::<u64>();

    // SAFETY: The caller guarantees that the decremented cursor is a writable,
    // aligned location within the task's private stack.
    unsafe { (*cursor as *mut u64).write(value) };
}

/// Complete process-visible termination after the scheduler has proven the
/// task is no longer physically executing. Caller holds SCHEDULER, preserving
/// the global SCHEDULER -> PROCESS_TABLE lock order. Address-space destruction
/// is deferred to `wait_process`; this keeps teardown out of the scheduler
/// critical section while making the zombie state truthful.
fn complete_process_termination(task_id: TaskId, signal: u8) {
    // Remote/scheduler-driven termination bypasses exit_current_process(), so
    // it needs the same Stage 10.3 async-owner reclamation guarantee.
    crate::async_network::release_owner(task_id);
    crate::async_file::release_owner(task_id);
    crate::block_io::release_owner(task_id);
    crate::async_op::release_owner(task_id);
    // Mutate process-visible state and take ownership of the file table while
    // PROCESS_TABLE is held.  Every operation below can acquire another
    // subsystem lock, so it must run after this guard is dropped; otherwise a
    // lower-ranked completion or VFS lock would invert PROCESS_TABLE's rank.
    let (process_id, parent_id, exit_code, mut files) = {
        let mut processes = process_table_lock();
        let Some(process) = processes
            .iter_mut()
            .flatten()
            .find(|process| process.task_id == task_id)
        else {
            return;
        };
        if process.state == ProcessState::Exited {
            return;
        }
        process.pending_signal = u64::from(signal);
        process.state = ProcessState::Exited;
        process.exit_code = 128 + i32::from(signal);
        (
            process.id,
            process.parent,
            process.exit_code,
            core::mem::take(&mut process.files),
        )
    };

    // These releases may take completion-port, event, VFS, or pipe locks;
    // keeping them outside PROCESS_TABLE makes the global lock order explicit.
    crate::completion_port::release_owner(process_id.as_u64());
    crate::async_events::release_owner(process_id.as_u64());
    let _ = crate::notifications::publish(crate::notifications::Notification {
        recipient: parent_id.as_u64(),
        kind: crate::notifications::Kind::ChildExit,
        source: process_id.as_u64(),
        payload: exit_code as u64,
    });
    release_file_table(&mut files);
}

/// If this process requested its own termination (for example `kill(getpid(),
/// SIGKILL)`), acknowledge it before returning to ring 3. Remote running tasks
/// are retired by the scheduler hand-off path instead.
pub fn honor_current_termination_request() {
    let signal = {
        let scheduler = SCHEDULER.lock();
        if scheduler.task_count == 0 {
            0
        } else {
            scheduler.tasks[scheduler.current_slot[crate::smp::cpu_index()]].termination_signal
        }
    };
    if signal != 0 {
        exit_current_process(128 + i32::from(signal));
    }
}

pub fn kill_process(pid: u64, sig: u64) -> Result<(), FileError> {
    if sig > 31 {
        return Err(FileError::NotFound);
    }
    let current_pid = current_process_id();
    let mut processes = process_table_lock();
    let current_group = processes
        .iter()
        .flatten()
        .find(|p| p.id.as_u64() == current_pid)
        .map(|p| p.process_group)
        .unwrap_or(current_pid);

    // POSIX-ish targets: pid>0 one process; pid==0 caller's process group;
    // a two's-complement negative pid targets process group abs(pid).
    let signed = pid as i64;
    let target_group = if pid == 0 {
        Some(current_group)
    } else if signed < 0 {
        Some(signed.wrapping_neg() as u64)
    } else {
        None
    };
    let mut matched = false;
    let mut termination_requests = alloc::vec::Vec::new();
    for process in processes.iter_mut().flatten() {
        let selected = if let Some(group) = target_group {
            process.process_group == group
        } else {
            process.id.as_u64() == pid
        };
        if !selected {
            continue;
        }
        matched = true;
        if sig == 0 {
            continue;
        }
        let action = process.signal_actions[sig as usize];
        if action == 1 {
            continue;
        } // SIG_IGN
        if action > 1 && sig != 9 {
            process.pending_signal = sig;
            continue;
        }
        if sig == 9 || sig == 15 || sig == 2 {
            // Do not publish ProcessState::Exited or release resources here.
            // This process may still be physically executing on a CPU. The
            // scheduler owns the transition and acknowledges it at a safe
            // state/stack hand-off boundary.
            process.pending_signal = sig;
            if process.state != ProcessState::Exited {
                termination_requests.push((process.task_id, sig as u8));
            }
        } else {
            process.pending_signal = sig;
        }
    }
    drop(processes);

    if !termination_requests.is_empty() {
        let mut reschedule_mask = 0usize;
        {
            let mut scheduler = SCHEDULER.lock();
            for (task_id, signal) in termination_requests {
                if let Some(cpu) = scheduler.request_termination(task_id, signal) {
                    reschedule_mask |= cpu_bit(cpu);
                }
            }
        }
        for cpu in 0..crate::smp::MAX_CPUS {
            if reschedule_mask & cpu_bit(cpu) != 0 {
                let _ = crate::smp::reschedule_cpu(cpu);
            }
        }
    }

    if matched {
        Ok(())
    } else {
        Err(FileError::NotFound)
    }
}

/// Set/query a minimal sigaction disposition. `handler` uses 0=SIG_DFL, 1=SIG_IGN,
/// otherwise it is a userspace handler address. Returns the previous disposition.
pub fn sigaction_current(sig: u64, handler: u64) -> Result<u64, FileError> {
    if sig == 0 || sig > 31 || sig == 9 {
        return Err(FileError::PermissionDenied);
    }
    let task_id = current_task_id();
    let mut processes = process_table_lock();
    let process = processes
        .iter_mut()
        .flatten()
        .find(|p| p.task_id == task_id)
        .ok_or(FileError::NoProcess)?;
    let slot = &mut process.signal_actions[sig as usize];
    let old = *slot;
    *slot = handler;
    Ok(old)
}

pub fn current_process_group() -> u64 {
    let task_id = current_task_id();
    let group = {
        let processes = process_table_lock();
        processes
            .iter()
            .flatten()
            .find(|p| p.task_id == task_id)
            .map(|p| p.process_group)
    };
    group.unwrap_or_else(current_process_id)
}

pub fn set_process_group(pid: u64, pgid: u64) -> Result<(), FileError> {
    let caller = current_process_id();
    let target = if pid == 0 { caller } else { pid };
    let group = if pgid == 0 { target } else { pgid };
    let mut processes = process_table_lock();
    let process = processes
        .iter_mut()
        .flatten()
        .find(|p| p.id.as_u64() == target)
        .ok_or(FileError::NotFound)?;
    if target != caller && process.parent.as_u64() != caller {
        return Err(FileError::PermissionDenied);
    }
    process.process_group = group;
    Ok(())
}

/// Return and clear a pending caught signal for the current process.
pub fn take_pending_signal_current() -> Option<(u64, u64)> {
    let task_id = current_task_id();
    let mut processes = process_table_lock();
    let process = processes
        .iter_mut()
        .flatten()
        .find(|p| p.task_id == task_id)?;
    let sig = process.pending_signal;
    if sig == 0 || sig > 31 {
        return None;
    }
    let handler = process.signal_actions[sig as usize];
    if handler <= 1 {
        return None;
    }
    process.pending_signal = 0;
    Some((sig, handler))
}

pub fn seek_current(descriptor: u64, offset: u64) -> Result<u64, FileError> {
    let descriptor = usize::try_from(descriptor).map_err(|_| FileError::BadDescriptor)?;
    let offset = usize::try_from(offset).map_err(|_| FileError::BadDescriptor)?;
    let task_id = current_task_id();
    let processes = process_table_lock();
    let process = processes
        .iter()
        .flatten()
        .find(|process| process.task_id == task_id)
        .ok_or(FileError::NoProcess)?;
    let fd = process
        .files
        .get(descriptor)
        .copied()
        .flatten()
        .ok_or(FileError::BadDescriptor)?;
    drop(processes);
    match fd {
        FdKind::File { id, scope } => {
            if !current_file_access_allowed(Capability::FileRead, scope, false) {
                return Err(FileError::PermissionDenied);
            }
            vfs::seek(id, offset)
                .map(|pos| pos as u64)
                .map_err(|_| FileError::BadDescriptor)
        },
        _ => Err(FileError::BadDescriptor),
    }
}

pub fn unlink_current(path: &str) -> Result<(), FileError> {
    if !current_has(Capability::FileWrite) {
        return Err(FileError::PermissionDenied);
    }
    let path = resolve_path_for_current(path)?;
    if !authorize_current_file_path(&path, true) {
        return Err(FileError::PermissionDenied);
    }
    if is_mnt_child(&path) {
        if !crate::storage::mnt_mounted() {
            return Err(FileError::NotFound);
        }
        ensure_existing_mnt_path(&path)?;
        vfs::can_remove(&path).map_err(map_vfs_file_error)?;
        vfs::prepare_remove(&path).map_err(map_vfs_file_error)?;
        crate::storage::delete_path(&path).map_err(map_mutation_file_error)?;
    }
    vfs::remove(&path).map_err(map_vfs_file_error)
}

pub fn rename_current(old: &str, new: &str) -> Result<(), FileError> {
    if !current_has(Capability::FileWrite) {
        return Err(FileError::PermissionDenied);
    }
    let old = resolve_path_for_current(old)?;
    let new = resolve_path_for_current(new)?;
    if !authorize_current_file_path(&old, true) || !authorize_current_file_path(&new, true) {
        return Err(FileError::PermissionDenied);
    }
    if touches_mnt(&old) || touches_mnt(&new) {
        if !crate::storage::mnt_mounted() {
            return Err(FileError::NotFound);
        }
        if !is_mnt_child(&old) || !is_mnt_child(&new) {
            return Err(FileError::NotFound);
        }
        ensure_existing_mnt_path(&old)?;
        ensure_mnt_parent(&new)?;
        vfs::can_rename(&old, &new).map_err(map_vfs_file_error)?;
        crate::storage::rename_path(&old, &new).map_err(map_mutation_file_error)?;
    }
    vfs::rename(&old, &new).map_err(map_vfs_file_error)
}
fn ensure_existing_mnt_path(path: &str) -> Result<(), FileError> {
    if touches_mnt(path) {
        if !crate::storage::mnt_mounted() {
            return Err(FileError::NotFound);
        }
        crate::storage::ensure_path(path).map_err(map_ensure_file_error)?;
    }
    Ok(())
}

fn ensure_mnt_parent(path: &str) -> Result<(), FileError> {
    if let Some(parent) = parent_path_string(path) {
        if touches_mnt(&parent) {
            ensure_existing_mnt_path(&parent)?;
        }
    }
    Ok(())
}

fn map_ensure_file_error(error: crate::storage::EnsureError) -> FileError {
    match error {
        crate::storage::EnsureError::TooLarge => FileError::TooManyFiles,
        _ => FileError::NotFound,
    }
}

fn map_persist_file_error(error: crate::storage::PersistError) -> FileError {
    match error {
        crate::storage::PersistError::TooLarge => FileError::TooManyFiles,
        _ => FileError::NotFound,
    }
}
fn is_mnt_child(path: &str) -> bool {
    path.starts_with("/mnt/")
}

fn touches_mnt(path: &str) -> bool {
    path == "/mnt" || path.starts_with("/mnt/")
}

fn parent_path_string(path: &str) -> Option<alloc::string::String> {
    if !path.starts_with('/') || path == "/" {
        return None;
    }
    let index = path.rfind('/')?;
    if index == 0 {
        Some(alloc::string::String::from("/"))
    } else {
        Some(alloc::string::String::from(&path[..index]))
    }
}

fn map_vfs_file_error(error: vfs::Error) -> FileError {
    match error {
        vfs::Error::NotFound => FileError::NotFound,
        vfs::Error::AlreadyExists => FileError::AlreadyExists,
        vfs::Error::ReadOnly => FileError::PermissionDenied,
        vfs::Error::NotEmpty => FileError::NotEmpty,
        vfs::Error::Full => FileError::TooManyFiles,
        _ => FileError::NotFound,
    }
}

fn map_mutation_file_error(error: crate::storage::MutationError) -> FileError {
    match error {
        crate::storage::MutationError::AlreadyExists => FileError::AlreadyExists,
        crate::storage::MutationError::NotEmpty => FileError::NotEmpty,
        crate::storage::MutationError::ReadOnly => FileError::PermissionDenied,
        crate::storage::MutationError::Unmounted
        | crate::storage::MutationError::NoDevice
        | crate::storage::MutationError::NotSupported
        | crate::storage::MutationError::NotFound
        | crate::storage::MutationError::BadName
        | crate::storage::MutationError::Failed => FileError::NotFound,
    }
}

/// The AP bootstrap becomes its permanent, CPU-owned idle context.
pub fn init_ap(cpu: usize) {
    let mut scheduler = SCHEDULER.lock();
    let slot = scheduler
        .tasks
        .iter()
        .position(|task| task.state == TaskState::Empty)
        .expect("AP idle slot");
    let id = TaskId(NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed));
    scheduler.tasks[slot].initialize(slot, id, "cpu-idle", idle_task, TaskPriority::LOW);
    scheduler.tasks[slot].cpu = cpu;
    scheduler.tasks[slot].affinity_mask = cpu_bit(cpu);
    scheduler.tasks[slot].state = TaskState::Running;
    scheduler.current_slot[cpu] = slot;
    scheduler.task_count += 1;
}

fn evacuate_cpu(scheduler: &mut Scheduler, target: usize) -> bool {
    let current_slot = scheduler.current_slot[target];
    let target_bit = cpu_bit(target);
    for (slot, task) in scheduler.tasks.iter_mut().enumerate() {
        if task.state == TaskState::Empty {
            continue;
        }
        if task.state == TaskState::Dead {
            if task.cpu == target {
                // A dead task owns no runnable context. Move it to the BSP so
                // its scheduler-owned reclamation can run after the AP parks.
                task.cpu = 0;
                task.affinity_mask = cpu_bit(0);
            }
            continue;
        }
        if task.cpu != target && task.affinity_mask & target_bit == 0 {
            continue;
        }
        // The CPU-owned idle task is handled by the checkpoint after all
        // runnable work has been transferred. A currently running task is
        // allowed to finish its hand-off; the checkpoint sees it as Ready.
        if slot == current_slot && matches!(task.name, "idle" | "cpu-idle") {
            continue;
        }
        if matches!(task.name, "idle" | "cpu-idle") {
            continue;
        }
        if task.cpu == target {
            if matches!(task.state, TaskState::Running | TaskState::Switching) {
                if slot == current_slot {
                    continue;
                }
                return false;
            }
            if !task.migratable {
                return false;
            }
            let Some(destination) = (0..target)
                .find(|cpu| task.affinity_mask & cpu_bit(*cpu) != 0)
            else {
                return false;
            };
            task.cpu = destination;
        } else if !task.migratable {
            // A task owned by another CPU with a hard affinity bit for the
            // target cannot safely retain that bit after the target leaves.
            return false;
        }
        task.affinity_mask &= !target_bit;
        if task.affinity_mask == 0 {
            return false;
        }
    }
    true
}

/// Prepare an AP for offline transition by evacuating every non-running task
/// that has a legal online destination. The current task is left in place
/// until its CPU reaches the idle checkpoint, preserving the running-task
/// ownership rule of the scheduler.
#[allow(dead_code)] // Called by the Stage 6 lifecycle control plane.
pub fn prepare_cpu_offline(target: usize) -> bool {
    if target == 0 || target + 1 != crate::smp::online_count() {
        return false;
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler = SCHEDULER.lock();
        let passed = evacuate_cpu(&mut scheduler, target);
        if passed {
            scheduler.validate_affinity_invariants();
        }
        passed
    })
}

/// Complete the transition from the AP-owned idle task. This is called only
/// on the target CPU after the reschedule request has selected its idle task.
pub fn offline_cpu_checkpoint(target: usize) -> bool {
    if target == 0 || target + 1 != crate::smp::online_count() {
        return false;
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler = SCHEDULER.lock();
        let current_slot = scheduler.current_slot[target];
        if !matches!(scheduler.tasks[current_slot].name, "idle" | "cpu-idle")
            || scheduler.tasks[current_slot].state != TaskState::Running
        {
            return false;
        }
        if !evacuate_cpu(&mut scheduler, target) {
            return false;
        }
        let idle = &mut scheduler.tasks[current_slot];
        idle.state = TaskState::Dead;
        idle.name = "offline-idle";
        idle.cpu = 0;
        idle.affinity_mask = cpu_bit(0);
        scheduler.task_count = scheduler.task_count.saturating_sub(1);
        scheduler.validate_affinity_invariants();
        true
    })
}

/// Recreate the idle task for a CPU whose AP context was parked by the
/// hotplug checkpoint. The previous `offline-idle` entry is reused to keep
/// repeated offline/online cycles bounded by the fixed task table.
pub fn online_cpu_checkpoint(target: usize) -> bool {
    if target == 0
        || target >= crate::smp::online_count()
        || !crate::smp::cpu_is_online(target)
    {
        return false;
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler = SCHEDULER.lock();
        let Some(slot) = scheduler.tasks.iter().position(|task| {
            task.state == TaskState::Empty
                || (task.state == TaskState::Dead && task.name == "offline-idle")
        }) else {
            return false;
        };
        let id = TaskId(NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed));
        scheduler.tasks[slot].initialize(slot, id, "cpu-idle", idle_task, TaskPriority::LOW);
        scheduler.tasks[slot].cpu = target;
        scheduler.tasks[slot].affinity_mask = cpu_bit(target);
        scheduler.tasks[slot].state = TaskState::Running;
        scheduler.current_slot[target] = slot;
        scheduler.task_count += 1;
        scheduler.validate_affinity_invariants();
        true
    })
}

fn spawn_kernel_on(
    cpu: usize,
    name: &'static str,
    entry: fn() -> !,
    migratable: bool,
    affinity_mask: usize,
) -> Result<TaskId, SpawnError> {
    assert!(cpu < crate::smp::online_count());
    assert!((affinity_mask & cpu_bit(cpu)) != 0, "spawn CPU must be inside affinity mask");
    assert_eq!(affinity_mask & !online_affinity_mask(), 0, "affinity includes offline CPU");
    let result = x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler = SCHEDULER.lock();
        let slot = scheduler
            .tasks
            .iter()
            .position(|task| task.state == TaskState::Empty)
            .ok_or(SpawnError::Full)?;
        let id = TaskId(NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed));
        scheduler.tasks[slot].initialize(slot, id, name, entry, TaskPriority::NORMAL);
        scheduler.tasks[slot].cpu = cpu;
        scheduler.tasks[slot].migratable = migratable;
        scheduler.tasks[slot].affinity_mask = affinity_mask;
        scheduler.task_count += 1;
        Ok(id)
    });
    if result.is_ok() {
        crate::smp::reschedule_cpu(cpu);
    }
    result
}

/// Explicit affinity for kernel jobs whose shared state is multicore-safe.
///
/// The task stays pinned to `cpu`. Use `spawn_migratable_on` only when the
/// task also obeys the Stage-6 migration ownership contract.
///
/// # Safety
/// AP entries may use atomics and task yield/sleep/exit primitives. They must
/// not call legacy allocator, paging, userspace, file, network or GUI services:
/// those domains still depend on BSP interrupt/preemption ownership.
pub unsafe fn spawn_on(
    cpu: usize,
    name: &'static str,
    entry: fn() -> !,
) -> Result<TaskId, SpawnError> {
    spawn_kernel_on(cpu, name, entry, false, cpu_bit(cpu))
}

/// Spawn a restricted kernel job that may move between CPUs while `Ready`.
///
/// # Safety
/// The entry must obey the same restricted service contract as `spawn_on` and
/// may not retain CPU-local pointers, per-CPU lock ownership, or interrupt-state
/// assumptions across a scheduling point. Stage 6 permits migration only while
/// the task is `Ready`; a running task is never moved.
pub unsafe fn spawn_migratable_on(
    cpu: usize,
    name: &'static str,
    entry: fn() -> !,
) -> Result<TaskId, SpawnError> {
    spawn_kernel_on(cpu, name, entry, true, online_affinity_mask())
}

/// Start a globally synchronized kernel service that may move between CPUs
/// while it is Ready. Services must keep all state behind a global lock, retain
/// no per-CPU pointers or interrupt-state assumptions across waits, and use
/// only the scheduler/event primitives plus their audited shared subsystem.
/// This is the controlled bridge for asynchronous I/O workers; it does not
/// make ordinary userspace or legacy device paths freely migratable.
pub fn spawn_io_service(name: &'static str, entry: fn() -> !) -> Result<TaskId, SpawnError> {
    let cpu = {
        let scheduler = SCHEDULER.lock();
        let loads = scheduler.run_loads();
        (0..crate::smp::online_count())
            .min_by_key(|cpu| {
                (
                    usize::from(crate::smp::cpu_domain(*cpu) != crate::smp::cpu_domain(crate::smp::cpu_index())),
                    loads[*cpu].runnable(),
                    *cpu,
                )
            })
            .unwrap_or(0)
    };
    // SAFETY: callers are the audited asynchronous service workers. Their
    // queues and subsystem state are globally synchronized and they retain no
    // CPU-local ownership across scheduler waits.
    unsafe { spawn_migratable_on(cpu, name, entry) }
}

/// Move one explicitly migratable task to another online CPU.
///
/// The global scheduler lock is the ownership hand-off: migration is accepted
/// only while the task is `Ready`, so no CPU can be executing or saving its
/// stack/context while the owner CPU field changes. Stage 7.4 permits this for
/// the narrow migratable Ring-3 probe class as well as audited kernel tasks.
///
/// `ever_dispatched` deliberately does not alter this explicit Stage 7.4 API:
/// it is execution-history metadata used to bound Stage 7.6 *automatic* Ring-3
/// rebalancing, not a replacement for the validated Ready-state contract.
pub fn migrate_ready_task(id: TaskId, target_cpu: usize) -> Result<(), MigrationError> {
    if target_cpu >= crate::smp::online_count() {
        return Err(MigrationError::OfflineCpu);
    }

    let result = x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler = SCHEDULER.lock();
        let task = scheduler
            .tasks
            .iter_mut()
            .find(|task| task.id == id && task.state != TaskState::Empty)
            .ok_or(MigrationError::UnknownTask)?;

        if !task.migratable {
            return Err(MigrationError::Pinned);
        }
        if (task.affinity_mask & cpu_bit(target_cpu)) == 0 {
            return Err(MigrationError::AffinityDenied);
        }
        if task.state != TaskState::Ready {
            return Err(MigrationError::NotReady);
        }
        task.cpu = target_cpu;
        scheduler.validate_affinity_invariants();
        Ok(())
    });
    if result.is_ok() {
        crate::smp::reschedule_cpu(target_cpu);
    }
    result
}

/// Return the hard CPU-affinity mask for a live task.
pub fn task_affinity(id: TaskId) -> Option<usize> {
    let scheduler = SCHEDULER.lock();
    scheduler
        .tasks
        .iter()
        .find(|task| task.id == id && task.state != TaskState::Empty)
        .map(|task| task.affinity_mask)
}

/// Restrict or expand the allowed CPUs for a Ready, explicitly migratable
/// task. Pinned jobs are rejected. Stage 7.4 extends the same hard-affinity
/// contract to the narrow migratable Ring-3 probe class. If the current owner
/// CPU is excluded, ownership is moved atomically under the scheduler lock to
/// the lowest-numbered allowed CPU. Execution-history metadata does not weaken
/// this Stage 7.4 contract; Stage 7.6 consumes it only in automatic balancing.
pub fn set_ready_task_affinity(id: TaskId, affinity_mask: usize) -> Result<(), AffinityError> {
    if affinity_mask == 0 {
        return Err(AffinityError::InvalidMask);
    }
    let online = online_affinity_mask();
    if affinity_mask & !online != 0 {
        return Err(AffinityError::OfflineCpu);
    }

    let target = x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler = SCHEDULER.lock();
        let task = scheduler
            .tasks
            .iter_mut()
            .find(|task| task.id == id && task.state != TaskState::Empty)
            .ok_or(AffinityError::UnknownTask)?;
        if !task.migratable {
            return Err(AffinityError::Pinned);
        }
        if task.state != TaskState::Ready {
            return Err(AffinityError::NotReady);
        }
        task.affinity_mask = affinity_mask;
        let target = if affinity_mask & cpu_bit(task.cpu) != 0 {
            None
        } else {
            let new_cpu = (0..crate::smp::online_count())
                .find(|cpu| affinity_mask & cpu_bit(*cpu) != 0)
                .expect("validated affinity must contain an online CPU");
            task.cpu = new_cpu;
            Some(new_cpu)
        };
        scheduler.validate_affinity_invariants();
        Ok(target)
    })?;

    if let Some(cpu) = target {
        crate::smp::reschedule_cpu(cpu);
    }
    Ok(())
}

/// Snapshot the non-idle runnable load of every CPU while holding the global
/// scheduler lock. The returned values are point-in-time diagnostics/placement
/// data; callers must not treat them as reservations.
pub fn cpu_run_loads() -> [CpuRunLoad; crate::smp::MAX_CPUS] {
    SCHEDULER.lock().run_loads()
}

/// Perform one bounded automatic load-balancing transfer. Only explicitly
/// migratable Ready tasks are candidates. Legacy CPU0-owned userspace and
/// pinned I/O/service workloads remain excluded by policy. The destination
/// receives a reschedule IPI after ownership is
/// published and the scheduler lock has been released.
pub fn rebalance_once() -> bool {
    REBALANCE_PASSES.fetch_add(1, Ordering::Relaxed);
    let migration = {
        let mut scheduler = SCHEDULER.lock();
        scheduler.rebalance_one()
    };
    let Some((_id, _source, target)) = migration else {
        REBALANCE_SKIPS.fetch_add(1, Ordering::Relaxed);
        return false;
    };
    REBALANCE_MIGRATIONS.fetch_add(1, Ordering::Relaxed);
    crate::smp::reschedule_cpu(target);
    true
}

/// Periodic conservative balancing hook used by the BSP LAPIC timer. The
/// cadence is intentionally low because this is a foundation scheduler, not a
/// topology/NUMA-aware production policy yet.
pub fn rebalance_tick() {
    if crate::smp::cpu_index() != 0 || crate::smp::online_count() < 2 {
        return;
    }
    let now = timer::ticks();
    let last = LAST_REBALANCE_TICK.load(Ordering::Relaxed);
    if now.wrapping_sub(last) < 8 {
        return;
    }
    if LAST_REBALANCE_TICK
        .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    REBALANCE_PASSES.fetch_add(1, Ordering::Relaxed);
    let migration = {
        let Some(mut scheduler) = SCHEDULER.try_lock() else {
            REBALANCE_SKIPS.fetch_add(1, Ordering::Relaxed);
            return;
        };
        scheduler.rebalance_one()
    };
    if let Some((_id, _source, target)) = migration {
        REBALANCE_MIGRATIONS.fetch_add(1, Ordering::Relaxed);
        crate::smp::reschedule_cpu(target);
    } else {
        REBALANCE_SKIPS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Count automatic rebalancer transfers of Ready Ring-3 tasks.
pub fn userspace_rebalance_migrations() -> u64 {
    USERSPACE_REBALANCE_MIGRATIONS.load(Ordering::Relaxed)
}

/// Count fork operations that preserved an AP parent's CPU/affinity contract.
pub fn fork_affinity_inheritances() -> u64 {
    FORK_AFFINITY_INHERITANCES.load(Ordering::Relaxed)
}

pub fn rebalance_stats() -> RebalanceStats {
    RebalanceStats {
        passes: REBALANCE_PASSES.load(Ordering::Relaxed),
        migrations: REBALANCE_MIGRATIONS.load(Ordering::Relaxed),
        skipped: REBALANCE_SKIPS.load(Ordering::Relaxed),
    }
}

/// Mark the current CPU for an immediate scheduling pass. Called from the
/// dedicated reschedule-IPI handler after the destination has acknowledged the
/// interrupt at the LAPIC.
pub fn request_reschedule() {
    PREEMPTION_REQUESTED[crate::smp::cpu_index()].store(true, Ordering::Release);
}

pub fn task_cpu(id: TaskId) -> Option<usize> {
    let scheduler = SCHEDULER.lock();
    scheduler
        .tasks
        .iter()
        .find(|task| task.id == id && task.state != TaskState::Empty)
        .map(|task| task.cpu)
}

/// Place an explicitly parallel kernel job on the least-loaded online CPU.
/// The job is marked migratable so a later Stage-6 balancer may hand a `Ready`
/// task to another CPU without changing the restricted-kernel-job contract.
///
/// # Safety
/// The entry must obey the same restricted service contract as `spawn_on` plus
/// the migration rules documented by `spawn_migratable_on`.
pub unsafe fn spawn_parallel(name: &'static str, entry: fn() -> !) -> Result<TaskId, SpawnError> {
    let cpu = {
        let scheduler = SCHEDULER.lock();
        let loads = scheduler.run_loads();
        (0..crate::smp::online_count())
            .min_by_key(|cpu| loads[*cpu].runnable())
            .unwrap_or(0)
    };
    unsafe { spawn_migratable_on(cpu, name, entry) }
}

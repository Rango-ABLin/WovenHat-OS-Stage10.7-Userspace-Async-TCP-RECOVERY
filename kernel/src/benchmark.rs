use spin::Mutex;

#[derive(Clone, Copy, Default)]
pub struct Snapshot {
    pub ticks: u64,
    pub context_switches: u64,
    pub preemptions: u64,
    pub idle_heartbeats: u64,
    pub allocated_frames: u64,
    pub heap_bytes: u64,
    pub heap_allocations: u64,
}

#[derive(Clone, Copy, Default)]
pub struct Delta {
    pub baseline_ready: bool,
    pub ticks: u64,
    pub context_switches: u64,
    pub preemptions: u64,
    pub idle_heartbeats: u64,
    pub frame_change: i64,
    pub heap_byte_change: i64,
    pub heap_allocations: u64,
}

static BASELINE: Mutex<Option<Snapshot>> = Mutex::new(None);

pub fn capture() -> Snapshot {
    let tasks = crate::task::summary();
    let memory = crate::memory::stats();
    let heap = crate::heap::stats();
    Snapshot {
        ticks: crate::timer::ticks(),
        context_switches: tasks.context_switches,
        preemptions: tasks.preemption_switches,
        idle_heartbeats: tasks.idle_heartbeats,
        allocated_frames: memory.allocated_frames,
        heap_bytes: heap.allocated_bytes as u64,
        heap_allocations: heap.allocations as u64,
    }
}

pub fn sample() -> Delta {
    let current = capture();
    let mut baseline = BASELINE.lock();
    let result = baseline.map_or(Delta::default(), |previous| between(previous, current));
    *baseline = Some(current);
    result
}

fn between(previous: Snapshot, current: Snapshot) -> Delta {
    Delta {
        baseline_ready: true,
        ticks: current.ticks.saturating_sub(previous.ticks),
        context_switches: current
            .context_switches
            .saturating_sub(previous.context_switches),
        preemptions: current.preemptions.saturating_sub(previous.preemptions),
        idle_heartbeats: current
            .idle_heartbeats
            .saturating_sub(previous.idle_heartbeats),
        frame_change: signed_change(previous.allocated_frames, current.allocated_frames),
        heap_byte_change: signed_change(previous.heap_bytes, current.heap_bytes),
        heap_allocations: current
            .heap_allocations
            .saturating_sub(previous.heap_allocations),
    }
}

fn signed_change(previous: u64, current: u64) -> i64 {
    if current >= previous {
        current.saturating_sub(previous).min(i64::MAX as u64) as i64
    } else {
        -(previous.saturating_sub(current).min(i64::MAX as u64) as i64)
    }
}

pub fn self_test() -> bool {
    let previous = Snapshot {
        ticks: 10,
        context_switches: 4,
        preemptions: 2,
        idle_heartbeats: 20,
        allocated_frames: 8,
        heap_bytes: 128,
        heap_allocations: 3,
    };
    let current = Snapshot {
        ticks: 17,
        context_switches: 9,
        preemptions: 5,
        idle_heartbeats: 24,
        allocated_frames: 6,
        heap_bytes: 192,
        heap_allocations: 5,
    };
    let delta = between(previous, current);
    delta.baseline_ready
        && delta.ticks == 7
        && delta.context_switches == 5
        && delta.preemptions == 3
        && delta.idle_heartbeats == 4
        && delta.frame_change == -2
        && delta.heap_byte_change == 64
        && delta.heap_allocations == 2
}

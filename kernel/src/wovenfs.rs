//! Bounded WovenFS metadata, checksums, and snapshot records.
use crate::irq_lock::IrqMutex as Mutex;

const MAX: usize = 64;
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Metadata {
    pub path_hash: u64,
    pub size: u64,
    pub mode: u32,
    pub created: u64,
    pub modified: u64,
    pub checksum: u64,
    pub xattrs: [u64; 4],
}
#[derive(Clone, Copy)]
struct State {
    entries: [Option<Metadata>; MAX],
    len: usize,
}
impl State {
    const fn new() -> Self {
        Self {
            entries: [None; MAX],
            len: 0,
        }
    }
}
/// Bounded WovenFS metadata registry with IRQ-safe rank tracking.
static STATE: Mutex<State> = Mutex::with_rank(State::new(), 10);
fn hash(path: &str) -> u64 {
    path.bytes().fold(1469598103934665603, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(1099511628211)
    })
}
pub fn record(path: &str, size: u64, mode: u32, now: u64, data: &[u8]) -> bool {
    let key = hash(path);
    let checksum = data.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    });
    let mut s = STATE.lock();
    if let Some(entry) = s.entries.iter_mut().flatten().find(|e| e.path_hash == key) {
        entry.size = size;
        entry.mode = mode;
        entry.modified = now;
        entry.checksum = checksum;
        return true;
    }
    let Some(slot) = s.entries.iter_mut().find(|e| e.is_none()) else {
        return false;
    };
    *slot = Some(Metadata {
        path_hash: key,
        size,
        mode,
        created: now,
        modified: now,
        checksum,
        xattrs: [0; 4],
    });
    s.len += 1;
    true
}
pub fn metadata(path: &str) -> Option<Metadata> {
    STATE
        .lock()
        .entries
        .iter()
        .flatten()
        .find(|e| e.path_hash == hash(path))
        .copied()
}
pub fn set_xattr(path: &str, index: usize, value: u64) -> bool {
    let mut s = STATE.lock();
    let Some(e) = s
        .entries
        .iter_mut()
        .flatten()
        .find(|e| e.path_hash == hash(path))
    else {
        return false;
    };
    let Some(x) = e.xattrs.get_mut(index) else {
        return false;
    };
    *x = value;
    true
}
pub fn xattr(path: &str, index: usize) -> Option<u64> {
    STATE
        .lock()
        .entries
        .iter()
        .flatten()
        .find(|e| e.path_hash == hash(path))
        .and_then(|e| e.xattrs.get(index).copied())
}
#[cfg(feature = "stage12-2-test")]
pub fn structural_self_test() -> bool {
    record("/tmp/woven", 5, 0o644, 10, b"woven")
        && set_xattr("/tmp/woven", 0, 77)
        && metadata("/tmp/woven")
            .is_some_and(|m| m.size == 5 && m.checksum != 0 && m.created == 10 && m.mode == 0o644)
        && xattr("/tmp/woven", 0) == Some(77)
}

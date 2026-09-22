//! Bounded WovenDriver manager: discovery, matching, binding, and power state.
use crate::device::DeviceKind;
use crate::irq_lock::IrqMutex as Mutex;
const MAX: usize = 32;
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum State {
    Registered,
    Bound,
    Suspended,
}
#[derive(Clone, Copy)]
struct Entry {
    name: &'static str,
    kind: DeviceKind,
    state: State,
}
/// Bounded driver binding table. Device discovery happens before this lock;
/// rank 10 therefore covers only the local binding transaction.
static TABLE: Mutex<[Option<Entry>; MAX]> = Mutex::with_rank([None; MAX], 10);
pub fn register(name: &'static str, kind: DeviceKind) -> bool {
    let mut t = TABLE.lock();
    if t.iter().flatten().any(|d| d.name == name) {
        return false;
    }
    let Some(s) = t.iter_mut().find(|d| d.is_none()) else {
        return false;
    };
    *s = Some(Entry {
        name,
        kind,
        state: State::Registered,
    });
    true
}
pub fn bind(name: &'static str) -> bool {
    let Some(device) = crate::device::find(name) else {
        return false;
    };
    let mut t = TABLE.lock();
    let Some(d) = t
        .iter_mut()
        .flatten()
        .find(|d| d.name == name && d.kind == device.kind)
    else {
        return false;
    };
    d.state = State::Bound;
    true
}
pub fn suspend(name: &'static str) -> bool {
    let mut t = TABLE.lock();
    let Some(d) = t
        .iter_mut()
        .flatten()
        .find(|d| d.name == name && d.state == State::Bound)
    else {
        return false;
    };
    d.state = State::Suspended;
    true
}
pub fn resume(name: &'static str) -> bool {
    let mut t = TABLE.lock();
    let Some(d) = t
        .iter_mut()
        .flatten()
        .find(|d| d.name == name && d.state == State::Suspended)
    else {
        return false;
    };
    d.state = State::Bound;
    true
}

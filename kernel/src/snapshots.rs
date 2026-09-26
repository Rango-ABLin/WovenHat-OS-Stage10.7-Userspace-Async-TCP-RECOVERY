//! Bounded copy-on-write snapshot catalog for filesystem generations.
use crate::irq_lock::IrqMutex as Mutex;
const MAX: usize = 8;
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub id: u64,
    pub generation: u64,
    pub checksum: u64,
}
/// Bounded snapshot catalog. Rank 10 keeps catalog updates interrupt-safe;
/// snapshot records contain no references that require a lower-ranked lock.
static TABLE: Mutex<[Option<Snapshot>; MAX]> = Mutex::with_rank([None; MAX], 10);
pub fn create(generation: u64, checksum: u64) -> Option<u64> {
    let mut t = TABLE.lock();
    let slot = t.iter_mut().position(|s| s.is_none())?;
    let id = slot as u64 + 1;
    t[slot] = Some(Snapshot {
        id,
        generation,
        checksum,
    });
    Some(id)
}
pub fn get(id: u64) -> Option<Snapshot> {
    TABLE.lock().iter().flatten().find(|s| s.id == id).copied()
}
pub fn restore(id: u64) -> Option<(u64, u64)> {
    get(id).map(|s| (s.generation, s.checksum))
}
pub fn remove(id: u64) -> bool {
    let mut t = TABLE.lock();
    if let Some(s) = t.iter_mut().find(|s| s.is_some_and(|v| v.id == id)) {
        *s = None;
        true
    } else {
        false
    }
}
#[cfg(feature = "stage12-4-test")]
pub fn structural_self_test() -> bool {
    let Some(id) = create(4, 99) else {
        return false;
    };
    get(id)
        == Some(Snapshot {
            id,
            generation: 4,
            checksum: 99,
        })
        && restore(id) == Some((4, 99))
        && remove(id)
        && get(id).is_none()
}

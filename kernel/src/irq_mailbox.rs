//! A single coalescing interrupt subscription. Epochs are never reused by the
//! owner; a producer that captured an old epoch cannot publish into a new one.
use core::sync::atomic::{AtomicU64, Ordering};

pub struct Mailbox(AtomicU64);

impl Mailbox {
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    pub fn activate(&self, epoch: u64) -> bool {
        epoch != 0
            && epoch <= u64::MAX >> 1
            && self
                .0
                .compare_exchange(0, epoch << 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }

    pub fn epoch(&self) -> u64 {
        self.0.load(Ordering::Acquire) >> 1
    }

    pub fn publish(&self, epoch: u64) -> bool {
        if epoch == 0 || epoch > u64::MAX >> 1 {
            return false;
        }
        match self.0.compare_exchange(
            epoch << 1,
            (epoch << 1) | 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => true,
            Err(current) => current == (epoch << 1) | 1,
        }
    }

    pub fn claim(&self) -> Option<u64> {
        let current = self.0.load(Ordering::Acquire);
        if current & 1 == 0 {
            return None;
        }
        self.0
            .compare_exchange(current, current & !1, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|value| value >> 1)
    }

    pub fn retire(&self, epoch: u64) {
        let mut current = self.0.load(Ordering::Acquire);
        while current >> 1 == epoch && current != 0 {
            match self
                .0
                .compare_exchange(current, 0, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return,
                Err(value) => current = value,
            }
        }
    }
}

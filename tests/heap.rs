//! Exercise the production heap state machine with host lock/paging doubles.
#![allow(dead_code)]
extern crate alloc;

mod memory {
    pub struct Stats {
        pub remaining_frames: u64,
    }
    pub fn stats() -> Stats {
        Stats {
            remaining_frames: 4096,
        }
    }
}

mod paging {
    pub fn map_range(_: u64, _: usize) -> Result<(), ()> {
        Ok(())
    }
}

mod irq_lock {
    pub struct IrqMutex<T>(std::sync::Mutex<T>);
    impl<T> IrqMutex<T> {
        pub const fn with_rank(value: T, _: u8) -> Self {
            Self(std::sync::Mutex::new(value))
        }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
            self.0.lock().unwrap()
        }
    }
}

#[path = "../kernel/src/heap.rs"]
mod heap;

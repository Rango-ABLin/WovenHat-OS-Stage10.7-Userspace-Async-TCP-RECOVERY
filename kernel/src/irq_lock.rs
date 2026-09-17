//! Spin locks shared by preemptible workers and syscall/teardown paths.
//! Disable local interrupts before acquisition so a timer cannot deschedule a
//! lock holder and run a second task that spins on the same CPU. Other CPUs are
//! serialized by the underlying mutex. Never sleep or switch tasks under a guard.

use core::{marker::PhantomData, ops::{Deref, DerefMut}};
use x86_64::instructions::interrupts;

#[cfg(not(test))]
mod lock_order {
    use core::sync::atomic::{AtomicU8, AtomicU32, AtomicUsize, Ordering};

    const MAX_DEPTH: usize = 8;
    static DEPTH: [AtomicU8; crate::smp::MAX_CPUS] =
        [const { AtomicU8::new(0) }; crate::smp::MAX_CPUS];
    static HIGHEST_RANK: [AtomicU8; crate::smp::MAX_CPUS] =
        [const { AtomicU8::new(0) }; crate::smp::MAX_CPUS];
    static HELD: [[AtomicUsize; MAX_DEPTH]; crate::smp::MAX_CPUS] =
        [const { [const { AtomicUsize::new(0) }; MAX_DEPTH] }; crate::smp::MAX_CPUS];
    static HELD_LINE: [[AtomicU32; MAX_DEPTH]; crate::smp::MAX_CPUS] =
        [const { [const { AtomicU32::new(0) }; MAX_DEPTH] }; crate::smp::MAX_CPUS];

    pub struct Token {
        cpu: usize,
        address: usize,
        previous_rank: u8,
        line: u32,
    }

    #[track_caller]
    pub fn enter(address: usize, rank: u8) -> Token {
        let cpu = crate::smp::lock_cpu_index();
        let line = core::panic::Location::caller().line();
        let previous_rank = HIGHEST_RANK[cpu].load(Ordering::Relaxed);
        if rank != 0 && previous_rank != 0 && rank < previous_rank {
            panic!(
                "IrqMutex lock-order inversion cpu={} requested={} held={}",
                cpu,
                rank,
                previous_rank
            );
        }
        let depth = DEPTH[cpu].load(Ordering::Relaxed) as usize;
        if depth >= MAX_DEPTH {
            panic!("IrqMutex nesting depth exceeded");
        }
        for held in &HELD[cpu][..depth] {
            if held.load(Ordering::Relaxed) == address {
                panic!("recursive IrqMutex acquisition");
            }
        }
        HELD[cpu][depth].store(address, Ordering::Relaxed);
        HELD_LINE[cpu][depth].store(line, Ordering::Relaxed);
        DEPTH[cpu].store((depth + 1) as u8, Ordering::Relaxed);
        if rank > previous_rank {
            HIGHEST_RANK[cpu].store(rank, Ordering::Relaxed);
        }
        Token { cpu, address, previous_rank, line }
    }

    pub fn exit(token: Token) {
        let depth = DEPTH[token.cpu].load(Ordering::Relaxed) as usize;
        assert!(depth != 0, "IrqMutex lock-order underflow");
        let held = HELD[token.cpu][depth - 1].load(Ordering::Relaxed);
        if held != token.address {
            panic!(
                "IrqMutex guards must be released in nesting order cpu={} releasing={:#x} at line={} held={:#x} at line={}",
                token.cpu, token.address, token.line, held,
                HELD_LINE[token.cpu][depth - 1].load(Ordering::Relaxed)
            );
        }
        HELD[token.cpu][depth - 1].store(0, Ordering::Relaxed);
        HELD_LINE[token.cpu][depth - 1].store(0, Ordering::Relaxed);
        DEPTH[token.cpu].store((depth - 1) as u8, Ordering::Relaxed);
        HIGHEST_RANK[token.cpu].store(token.previous_rank, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod lock_order {
    pub struct Token;
    pub fn enter(_: usize, _: u8) -> Token { Token }
    pub fn exit(_: Token) {}
}

pub struct IrqMutex<T> {
    inner: spin::Mutex<T>,
    rank: u8,
}

impl<T> IrqMutex<T> {
    pub const fn new(value: T) -> Self { Self::with_rank(value, 0) }

    /// Construct a lock with a monotonic nesting rank. A nonzero rank is
    /// checked against locks already held by this CPU; lower-ranked nesting is
    /// rejected before spinning, making lock-order bugs fail deterministically.
    pub const fn with_rank(value: T, rank: u8) -> Self {
        Self { inner: spin::Mutex::new(value), rank }
    }

    #[track_caller]
    pub fn lock(&self) -> IrqMutexGuard<'_, T> {
        let restore_interrupts = interrupts::are_enabled();
        interrupts::disable();
        let token = lock_order::enter(self as *const Self as usize, self.rank);
        IrqMutexGuard {
            guard: Some(self.inner.lock()),
            restore_interrupts,
            token: Some(token),
            // Interrupt state belongs to the acquiring CPU, not another thread.
            _not_send: PhantomData,
        }
    }

    /// Attempt an interrupt-safe acquisition without spinning. The order
    /// tracker is entered before probing the mutex so an inversion fails at
    /// the call site; a failed probe rolls the tracker state back atomically.
    #[track_caller]
    pub fn try_lock(&self) -> Option<IrqMutexGuard<'_, T>> {
        let restore_interrupts = interrupts::are_enabled();
        interrupts::disable();
        let token = lock_order::enter(self as *const Self as usize, self.rank);
        let Some(guard) = self.inner.try_lock() else {
            lock_order::exit(token);
            if restore_interrupts { interrupts::enable(); }
            return None;
        };
        Some(IrqMutexGuard {
            guard: Some(guard),
            restore_interrupts,
            token: Some(token),
            _not_send: PhantomData,
        })
    }
}

pub struct IrqMutexGuard<'a, T> {
    guard: Option<spin::MutexGuard<'a, T>>,
    restore_interrupts: bool,
    token: Option<lock_order::Token>,
    _not_send: PhantomData<*mut ()>,
}

impl<T> Deref for IrqMutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T { self.guard.as_ref().unwrap() }
}

impl<T> DerefMut for IrqMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T { self.guard.as_mut().unwrap() }
}

impl<T> Drop for IrqMutexGuard<'_, T> {
    fn drop(&mut self) {
        // Unlock first: enabling IF while still holding the mutex would reopen
        // the exact preemption deadlock this guard prevents. Nested guards see
        // IF=0 and leave restoration to the outermost guard.
        drop(self.guard.take());
        lock_order::exit(self.token.take().unwrap());
        if self.restore_interrupts { interrupts::enable(); }
    }
}

/// Ranked spin mutex for long CPU-only sections that must keep IRQs live.
/// Local timer preemption is deferred while held. Never use from an IRQ
/// handler, and never block or voluntarily switch while holding this guard.
#[cfg(not(test))]
pub struct PreemptMutex<T> {
    inner: spin::Mutex<T>,
    rank: u8,
}

#[cfg(not(test))]
impl<T> PreemptMutex<T> {
    pub const fn with_rank(value: T, rank: u8) -> Self {
        Self { inner: spin::Mutex::new(value), rank }
    }

    #[track_caller]
    pub fn lock(&self) -> PreemptMutexGuard<'_, T> {
        let preemption = crate::task::local_preemption_guard();
        // The rank stack is per-CPU and can also be used by an IRQ handler.
        // Keep each tracker update atomic with respect to local IRQ entry.
        let token = interrupts::without_interrupts(|| {
            lock_order::enter(self as *const Self as usize, self.rank)
        });
        PreemptMutexGuard {
            guard: Some(self.inner.lock()),
            token: Some(token),
            preemption: Some(preemption),
            _not_send: PhantomData,
        }
    }
}

#[cfg(not(test))]
pub struct PreemptMutexGuard<'a, T> {
    guard: Option<spin::MutexGuard<'a, T>>,
    token: Option<lock_order::Token>,
    preemption: Option<crate::task::LocalPreemptGuard>,
    _not_send: PhantomData<*mut ()>,
}

#[cfg(not(test))]
impl<T> Deref for PreemptMutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T { self.guard.as_ref().unwrap() }
}

#[cfg(not(test))]
impl<T> DerefMut for PreemptMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T { self.guard.as_mut().unwrap() }
}

#[cfg(not(test))]
impl<T> Drop for PreemptMutexGuard<'_, T> {
    fn drop(&mut self) {
        drop(self.guard.take());
        interrupts::without_interrupts(|| lock_order::exit(self.token.take().unwrap()));
        drop(self.preemption.take());
    }
}

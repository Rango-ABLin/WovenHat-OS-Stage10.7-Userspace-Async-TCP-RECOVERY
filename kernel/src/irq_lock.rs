//! Spin locks shared by preemptible workers and syscall/teardown paths.
//! Disable local interrupts before acquisition so a timer cannot deschedule a
//! lock holder and run a second task that spins on the same CPU. Other CPUs are
//! serialized by the underlying mutex. Never sleep or switch tasks under a guard.

use core::{marker::PhantomData, ops::{Deref, DerefMut}};
use x86_64::instructions::interrupts;

pub struct IrqMutex<T>(spin::Mutex<T>);

impl<T> IrqMutex<T> {
    pub const fn new(value: T) -> Self { Self(spin::Mutex::new(value)) }

    pub fn lock(&self) -> IrqMutexGuard<'_, T> {
        let restore_interrupts = interrupts::are_enabled();
        interrupts::disable();
        IrqMutexGuard {
            guard: Some(self.0.lock()),
            restore_interrupts,
            // Interrupt state belongs to the acquiring CPU, not another thread.
            _not_send: PhantomData,
        }
    }
}

pub struct IrqMutexGuard<'a, T> {
    guard: Option<spin::MutexGuard<'a, T>>,
    restore_interrupts: bool,
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
        if self.restore_interrupts { interrupts::enable(); }
    }
}

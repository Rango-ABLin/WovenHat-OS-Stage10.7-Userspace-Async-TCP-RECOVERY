//! Test the production guard with host interrupt-state and mutex doubles.
#![allow(dead_code)]
extern crate self as spin;
extern crate self as x86_64;

pub type MutexGuard<'a, T> = std::sync::MutexGuard<'a, T>;
pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
    pub fn lock(&self) -> MutexGuard<'_, T> {
        assert!(!instructions::interrupts::are_enabled());
        self.0.lock().unwrap()
    }
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        assert!(!instructions::interrupts::are_enabled());
        self.0.try_lock().ok()
    }
}
pub mod instructions {
    pub mod interrupts {
        use std::cell::Cell;
        thread_local! { static ENABLED: Cell<bool> = const { Cell::new(true) }; }
        pub fn are_enabled() -> bool { ENABLED.with(Cell::get) }
        pub fn disable() { ENABLED.with(|state| state.set(false)); }
        pub fn enable() { ENABLED.with(|state| state.set(true)); }
    }
}
#[path = "../kernel/src/irq_lock.rs"]
mod irq_lock;
use instructions::interrupts;
use irq_lock::IrqMutex;

#[test]
fn nested_guards_restore_the_original_interrupt_state() {
    let outer = IrqMutex::new(0);
    let inner = IrqMutex::new(0);
    {
        let mut a = outer.lock();
        assert!(!interrupts::are_enabled());
        {
            let mut b = inner.lock();
            *b = 2;
        }
        assert!(!interrupts::are_enabled());
        *a = 1;
    }
    assert!(interrupts::are_enabled());
    assert_eq!(*outer.lock(), 1);
    assert_eq!(*inner.lock(), 2);
    assert!(interrupts::are_enabled());
}

#[test]
fn syscall_context_keeps_interrupts_disabled() {
    interrupts::disable();
    let mutex = IrqMutex::new(1);
    assert_eq!(*mutex.lock(), 1);
    assert!(!interrupts::are_enabled());
    interrupts::enable();
}

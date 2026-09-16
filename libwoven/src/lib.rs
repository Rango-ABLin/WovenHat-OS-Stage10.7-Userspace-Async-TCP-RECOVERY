#![no_std]
//! Stable userspace API boundary. Syscall-backed implementations are added as
//! kernel ABI contracts mature; these handles keep application code typed now.

pub mod process { #[derive(Clone, Copy, PartialEq, Eq)] pub struct Id(pub u64); }
pub mod thread { #[derive(Clone, Copy, PartialEq, Eq)] pub struct Id(pub u64); }
pub mod fs { #[derive(Clone, Copy, PartialEq, Eq)] pub struct Handle(pub u64); }
pub mod net { #[derive(Clone, Copy, PartialEq, Eq)] pub struct Socket(pub u64); }
pub mod ipc { #[derive(Clone, Copy, PartialEq, Eq)] pub struct Handle(pub u64); }
pub mod async_io { #[derive(Clone, Copy, PartialEq, Eq)] pub struct Operation(pub u64); }
pub mod security { #[derive(Clone, Copy, PartialEq, Eq)] pub struct Capability(pub u64); }
pub mod time { pub type Ticks = u64; }
pub mod graphics { #[derive(Clone, Copy, PartialEq, Eq)] pub struct Surface(pub u64); }
pub mod notifications { #[derive(Clone, Copy, PartialEq, Eq)] pub struct Record { pub kind: u64, pub source: u64, pub payload: u64 } }
pub mod syscall {
    pub const NOTIFICATION_POLL: u64 = 95;
    pub const NOTIFICATION_POST: u64 = 96;
    /// Invoke the WovenHat Ring-3 ABI. Callers must pass validated user pointers.
    /// # Safety
    /// Arguments and pointers must satisfy the WovenHat syscall ABI.
    pub unsafe fn invoke(number: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
        let result: u64;
        // SAFETY: The caller is executing in WovenHat userspace with the ABI
        // pointer contract satisfied; int 0x80 is the DPL3 syscall gate.
        unsafe { core::arch::asm!("int 0x80", inlateout("rax") number => result, in("rdi") arg0, in("rsi") arg1, in("rdx") arg2, options(preserves_flags)); }
        result
    }
    /// # Safety
    /// The caller must be executing in WovenHat userspace.
    pub unsafe fn notification_post(kind: u64, payload: u64) -> u64 { unsafe { invoke(NOTIFICATION_POST, kind, payload, 0) } }
    /// # Safety
    /// `record` must be a valid writable userspace pointer.
    pub unsafe fn notification_poll(record: *mut super::notifications::Record) -> u64 { unsafe { invoke(NOTIFICATION_POLL, record as u64, 0, 0) } }
}

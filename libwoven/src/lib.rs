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

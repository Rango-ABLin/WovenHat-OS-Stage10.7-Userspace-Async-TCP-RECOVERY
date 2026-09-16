use core::arch::asm;
use core::fmt;

use x86_64::registers::control::Cr2;

use crate::serial;

#[repr(C)]
pub struct PanicContext {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
    pub cr2: u64,
}

impl PanicContext {
    pub fn capture() -> Self {
        let mut ctx = Self {
            rax: 0,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            rbp: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: 0,
            rsp: 0,
            rflags: 0,
            cr2: Cr2::read().map_or(0, |address| address.as_u64()),
        };

        unsafe {
            asm!(
                "mov {rax}, rax",
                "mov {rbx}, rbx",
                "mov {rcx}, rcx",
                "mov {rdx}, rdx",
                "mov {rsi}, rsi",
                "mov {rdi}, rdi",
                "mov {rbp}, rbp",
                "mov {r8}, r8",
                "mov {r9}, r9",
                "mov {r10}, r10",
                "mov {r11}, r11",
                "mov {r12}, r12",
                "mov {r13}, r13",
                "mov {r14}, r14",
                "mov {r15}, r15",
                rax = out(reg) ctx.rax,
                rbx = out(reg) ctx.rbx,
                rcx = out(reg) ctx.rcx,
                rdx = out(reg) ctx.rdx,
                rsi = out(reg) ctx.rsi,
                rdi = out(reg) ctx.rdi,
                rbp = out(reg) ctx.rbp,
                r8 = out(reg) ctx.r8,
                r9 = out(reg) ctx.r9,
                r10 = out(reg) ctx.r10,
                r11 = out(reg) ctx.r11,
                r12 = out(reg) ctx.r12,
                r13 = out(reg) ctx.r13,
                r14 = out(reg) ctx.r14,
                r15 = out(reg) ctx.r15,
                options(nomem, nostack, preserves_flags)
            );
        }

        let rsp: u64;
        unsafe {
            asm!("mov {0}, rsp", out(reg) rsp, options(nomem, nostack, preserves_flags));
        }
        ctx.rsp = rsp;

        unsafe {
            asm!("lea {0}, [rip]", out(reg) ctx.rip, options(nomem, nostack, preserves_flags));
        }
        ctx.rflags = x86_64::registers::rflags::read().bits();
        ctx.cr2 = Cr2::read().map_or(0, |address| address.as_u64());

        ctx
    }

    pub fn print(&self, message: &dyn fmt::Display) {
        serial::write_fmt(format_args!("\nKERNEL PANIC: {message}\n"));
        serial::write_fmt(format_args!(
            "RAX={:016x} RBX={:016x} RCX={:016x} RDX={:016x}\n",
            self.rax, self.rbx, self.rcx, self.rdx
        ));
        serial::write_fmt(format_args!(
            "RSI={:016x} RDI={:016x} RBP={:016x} RSP={:016x}\n",
            self.rsi, self.rdi, self.rbp, self.rsp
        ));
        serial::write_fmt(format_args!(
            "R8 ={:016x} R9 ={:016x} R10={:016x} R11={:016x}\n",
            self.r8, self.r9, self.r10, self.r11
        ));
        serial::write_fmt(format_args!(
            "R12={:016x} R13={:016x} R14={:016x} R15={:016x}\n",
            self.r12, self.r13, self.r14, self.r15
        ));
        serial::write_fmt(format_args!(
            "RIP={:016x} RFLAGS={:016x} CR2={:016x}\n",
            self.rip, self.rflags, self.cr2
        ));
    }
}

pub fn kernel_panic(message: &dyn fmt::Display) -> ! {
    x86_64::instructions::interrupts::disable();
    let context = PanicContext::capture();
    context.print(message);

    loop {
        x86_64::instructions::hlt();
    }
}

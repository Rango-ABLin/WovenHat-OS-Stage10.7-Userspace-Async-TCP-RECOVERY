use core::{
    arch::asm,
    sync::atomic::{AtomicBool, Ordering},
};

use spin::Once;

use x86_64::{
    registers::control::Cr2,
    structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode},
    PrivilegeLevel, VirtAddr,
};

use crate::{gdt, keyboard, pic, serial, syscall, task, timer};

static BREAKPOINT_REACHED: AtomicBool = AtomicBool::new(false);

static IDT: Once<InterruptDescriptorTable> = Once::new();

/// First WovenHat-owned PCI device vector reserved for WovenWiFi.
/// Kept below the LAPIC timer/IPI range (0xe0+) and away from syscall 0x80.
pub const WIFI_DEVICE_VECTOR: u8 = 0xd0;
static WIFI_DEVICE_IRQS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static WIFI_DEVICE_WORK: AtomicBool = AtomicBool::new(false);

pub fn wifi_device_irq_count() -> u64 {
    WIFI_DEVICE_IRQS.load(Ordering::Acquire)
}

pub fn take_wifi_device_work() -> bool {
    WIFI_DEVICE_WORK.swap(false, Ordering::AcqRel)
}

#[cfg(feature = "stage13-9-test")]
pub fn publish_wifi_device_work_for_test() {
    WIFI_DEVICE_WORK.store(true, Ordering::Release);
}

pub fn stage13_10z_vector_self_test() -> bool {
    WIFI_DEVICE_VECTOR >= 0x20
        && WIFI_DEVICE_VECTOR != 0x80
        && WIFI_DEVICE_VECTOR != crate::smp::TIMER_VECTOR
        && WIFI_DEVICE_VECTOR != crate::smp::RESCHEDULE_VECTOR
        && WIFI_DEVICE_VECTOR != crate::smp::SPURIOUS_VECTOR
}

pub fn init() {
    let idt = IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();

        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.non_maskable_interrupt.set_handler_fn(tlb_nmi_handler);
        idt[crate::smp::TIMER_VECTOR].set_handler_fn(lapic_timer_handler);
        idt[crate::smp::RESCHEDULE_VECTOR].set_handler_fn(reschedule_ipi_handler);
        idt[crate::smp::SPURIOUS_VECTOR].set_handler_fn(spurious_handler);
        idt[WIFI_DEVICE_VECTOR].set_handler_fn(wifi_device_interrupt_handler);
        idt.divide_error.set_handler_fn(divide_error_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        // SAFETY: The selected IST entry is initialized with a dedicated,
        // statically allocated stack by `gdt::init` before this IDT is loaded.
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt.general_protection_fault
            .set_handler_fn(general_protection_fault_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        // SAFETY: The assembly entry preserves all GPRs, calls the Rust
        // dispatcher with a SysV-aligned stack, and returns through iretq.
        unsafe {
            idt[0x80]
                .set_handler_addr(VirtAddr::new(syscall::entry_address()))
                .set_privilege_level(PrivilegeLevel::Ring3);
        }
        idt[pic::MASTER_OFFSET + timer::IRQ].set_handler_fn(timer_interrupt_handler);
        idt[pic::MASTER_OFFSET + keyboard::IRQ].set_handler_fn(keyboard_interrupt_handler);

        idt
    });

    idt.load();
}

pub fn breakpoint_reached() -> bool {
    BREAKPOINT_REACHED.load(Ordering::SeqCst)
}

extern "x86-interrupt" fn breakpoint_handler(_stack_frame: InterruptStackFrame) {
    BREAKPOINT_REACHED.store(true, Ordering::SeqCst);
}

fn dump_exception_context(label: &str, stack_frame: &InterruptStackFrame, error_code: Option<u64>) {
    serial::write_fmt(format_args!("\nEXCEPTION: {label}\n"));
    if let Some(code) = error_code {
        serial::write_fmt(format_args!("ERROR CODE: {code:#x}\n"));
    }

    serial::write_fmt(format_args!(
        "RIP: {:#x}\nCS: {:#x}\nRFLAGS: {:#x}\nRSP: {:#x}\nSS: {:#x}\n",
        stack_frame.instruction_pointer.as_u64(),
        stack_frame.code_segment.0,
        stack_frame.cpu_flags,
        stack_frame.stack_pointer.as_u64(),
        stack_frame.stack_segment.0,
    ));
    dump_cpu_state();
}

pub fn dump_cpu_state() {
    let cr0: u64;
    let cr2: u64;
    let cr3: u64;
    let cr4: u64;
    let rsp: u64;
    let rbp: u64;
    let rflags: u64;

    // SAFETY: Reading control registers and the current stack/frame pointers
    // has no side effects and is valid while executing in ring 0.
    unsafe {
        asm!(
            "mov {cr0}, cr0",
            "mov {cr2}, cr2",
            "mov {cr3}, cr3",
            "mov {cr4}, cr4",
            "mov {rsp}, rsp",
            "mov {rbp}, rbp",
            "pushfq",
            "pop {rflags}",
            cr0 = out(reg) cr0,
            cr2 = out(reg) cr2,
            cr3 = out(reg) cr3,
            cr4 = out(reg) cr4,
            rsp = out(reg) rsp,
            rbp = out(reg) rbp,
            rflags = out(reg) rflags,
            options(preserves_flags),
        );
    }

    serial::write_fmt(format_args!(
        "CPU: CR0={cr0:#x} CR2={cr2:#x} CR3={cr3:#x} CR4={cr4:#x}\nSTACK: RSP={rsp:#x} RBP={rbp:#x} RFLAGS={rflags:#x}\nTICKS: {}\n",
        timer::ticks(),
    ));
}

extern "x86-interrupt" fn divide_error_handler(stack_frame: InterruptStackFrame) {
    dump_exception_context("DIVIDE ERROR", &stack_frame, None);
    recover_user_fault(&stack_frame, 0, -8);
    halt();
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    dump_exception_context("INVALID OPCODE", &stack_frame, None);
    recover_user_fault(&stack_frame, 6, -4);
    halt();
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    dump_exception_context("DOUBLE FAULT", &stack_frame, Some(error_code));
    halt();
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    dump_exception_context("GENERAL PROTECTION FAULT", &stack_frame, Some(error_code));
    recover_user_fault(&stack_frame, 13, -13);
    halt();
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    let address = Cr2::read();
    let fault_address = address.ok().map(|addr| addr.as_u64());

    if error_code.contains(PageFaultErrorCode::USER_MODE)
        && !error_code.intersects(
            PageFaultErrorCode::PROTECTION_VIOLATION
                | PageFaultErrorCode::INSTRUCTION_FETCH
                | PageFaultErrorCode::MALFORMED_TABLE,
        )
    {
        if let Some(addr) = fault_address {
            if task::try_handle_file_fault(
                addr,
                error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE),
            ) {
                return;
            }
        }
    }

    // Copy-on-write: user write to a present read-only page that is
    // logically writable in the process mapping tables.
    if error_code.contains(PageFaultErrorCode::USER_MODE)
        && error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE)
        && error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION)
    {
        if let Some(addr) = fault_address {
            if task::try_handle_cow_fault(addr) {
                return;
            }
        }
    }

    dump_exception_context("PAGE FAULT", &stack_frame, Some(error_code.bits()));
    if let Some(addr) = fault_address {
        serial::write_fmt(format_args!("FAULT ADDRESS: {:#x}\n", addr));
    } else {
        serial::write_fmt(format_args!("FAULT ADDRESS: UNAVAILABLE\n"));
    }
    serial::write_fmt(format_args!(
        "PAGE FAULT DETAILS: PROTECTION_VIOLATION={} WRITE={} USER={} RESERVED={} INSTRUCTION_FETCH={}\n",
        error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION),
        error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE),
        error_code.contains(PageFaultErrorCode::USER_MODE),
        error_code.contains(PageFaultErrorCode::MALFORMED_TABLE),
        error_code.contains(PageFaultErrorCode::INSTRUCTION_FETCH),
    ));
    if error_code.contains(PageFaultErrorCode::USER_MODE) {
        audit_user_fault(14);
        task::exit_current_process(-11);
    }
    halt();
}

fn recover_user_fault(stack_frame: &InterruptStackFrame, vector: u64, exit_code: i32) {
    if selector_is_user(stack_frame.code_segment.0) {
        audit_user_fault(vector);
        task::exit_current_process(exit_code);
    }
}

fn audit_user_fault(vector: u64) {
    crate::audit::record(
        task::current_process_id(),
        crate::audit::Action::ProcessFault,
        vector,
        false,
    );
}

pub fn fault_policy_self_test() -> bool {
    selector_is_user(0x23) && selector_is_user(0x1b) && !selector_is_user(0x08)
}

const fn selector_is_user(selector: u16) -> bool {
    selector & 3 == 3
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    timer::record_tick();
    task::tick();
    pic::notify_end_of_interrupt(timer::IRQ);
    task::preempt_from_interrupt();
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    keyboard::handle_interrupt();
    if crate::smp::routed_irq() {
        crate::smp::eoi();
    } else {
        pic::notify_end_of_interrupt(keyboard::IRQ);
    }
}

fn halt() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}

extern "x86-interrupt" fn tlb_nmi_handler(_frame: InterruptStackFrame) {
    crate::smp::acknowledge_tlb();
}
extern "x86-interrupt" fn spurious_handler(_frame: InterruptStackFrame) {}

/// Minimal hard-IRQ entry for a PCI Wi-Fi vector.
///
/// The hard interrupt path does no allocation, locking, firmware parsing, or
/// scheduler work. It only publishes deferred work and acknowledges the local
/// APIC. Stage Y remains responsible for reading/acknowledging Intel CSR causes.
extern "x86-interrupt" fn wifi_device_interrupt_handler(_frame: InterruptStackFrame) {
    WIFI_DEVICE_IRQS.fetch_add(1, Ordering::Relaxed);
    WIFI_DEVICE_WORK.store(true, Ordering::Release);
    crate::smp::eoi();
}
extern "x86-interrupt" fn lapic_timer_handler(_frame: InterruptStackFrame) {
    if crate::smp::cpu_index() == 0 {
        timer::record_tick();
    }
    task::tick();
    task::rebalance_tick();
    crate::smp::eoi();
    task::preempt_from_interrupt();
}

extern "x86-interrupt" fn reschedule_ipi_handler(_frame: InterruptStackFrame) {
    task::request_reschedule();
    crate::smp::eoi();
    task::preempt_from_interrupt();
}

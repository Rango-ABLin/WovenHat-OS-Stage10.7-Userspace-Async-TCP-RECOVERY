use core::cell::UnsafeCell;

use spin::Once;
use x86_64::{
    instructions::{
        segmentation::{Segment, CS, DS, ES, SS},
        tables::load_tss,
    },
    structures::{
        gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector},
        tss::TaskStateSegment,
    },
    VirtAddr,
};

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

const DOUBLE_FAULT_STACK_SIZE: usize = 4096 * 5;
const PRIVILEGE_STACK_SIZE: usize = 4096 * 8;

#[repr(align(16))]
struct KernelStack(UnsafeCell<[u8; DOUBLE_FAULT_STACK_SIZE]>);

// SAFETY: The stack is never accessed through Rust references after startup.
// The CPU exclusively writes to it while handling a double fault on this core.
unsafe impl Sync for KernelStack {}

static DOUBLE_FAULT_STACK: [KernelStack; crate::smp::MAX_CPUS] =
    [const { KernelStack(UnsafeCell::new([0; DOUBLE_FAULT_STACK_SIZE])) }; crate::smp::MAX_CPUS];

#[repr(align(16))]
struct PrivilegeStack(UnsafeCell<[u8; PRIVILEGE_STACK_SIZE]>);

// SAFETY: Each CPU has an exclusive privilege stack and TSS slot.
unsafe impl Sync for PrivilegeStack {}

static PRIVILEGE_STACK: [PrivilegeStack; crate::smp::MAX_CPUS] =
    [const { PrivilegeStack(UnsafeCell::new([0; PRIVILEGE_STACK_SIZE])) }; crate::smp::MAX_CPUS];
struct RuntimeTss(UnsafeCell<TaskStateSegment>);
// Each CPU updates only its own TSS, with local interrupts disabled.
unsafe impl Sync for RuntimeTss {}
static TSS: [Once<RuntimeTss>; crate::smp::MAX_CPUS] =
    [const { Once::new() }; crate::smp::MAX_CPUS];
static GDT: [Once<(GlobalDescriptorTable, Selectors)>; crate::smp::MAX_CPUS] =
    [const { Once::new() }; crate::smp::MAX_CPUS];

struct Selectors {
    code: SegmentSelector,
    data: SegmentSelector,
    user_code: SegmentSelector,
    user_data: SegmentSelector,
    tss: SegmentSelector,
}

pub fn init() {
    let cpu = crate::smp::cpu_index();
    let tss = TSS[cpu].call_once(|| {
        let mut tss = TaskStateSegment::new();
        let stack_start = VirtAddr::from_ptr(DOUBLE_FAULT_STACK[cpu].0.get());
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
            stack_start + DOUBLE_FAULT_STACK_SIZE as u64;

        let privilege_stack_start = VirtAddr::from_ptr(PRIVILEGE_STACK[cpu].0.get());
        tss.privilege_stack_table[0] = privilege_stack_start + PRIVILEGE_STACK_SIZE as u64;
        RuntimeTss(UnsafeCell::new(tss))
    });

    let (gdt, selectors) = GDT[cpu].call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let selectors = Selectors {
            code: gdt.append(Descriptor::kernel_code_segment()),
            data: gdt.append(Descriptor::kernel_data_segment()),
            user_code: gdt.append(Descriptor::user_code_segment()),
            user_data: gdt.append(Descriptor::user_data_segment()),
            tss: gdt.append(Descriptor::tss_segment(unsafe { &*tss.0.get() })),
        };
        (gdt, selectors)
    });

    gdt.load();

    // SAFETY: All selectors refer to live entries in the static GDT. The TSS
    // and its double-fault stack also have static storage and are initialized
    // before the task register is loaded.
    unsafe {
        CS::set_reg(selectors.code);
        SS::set_reg(selectors.data);
        DS::set_reg(selectors.data);
        ES::set_reg(selectors.data);
        load_tss(selectors.tss);
    }
}

pub fn user_segments() -> (SegmentSelector, SegmentSelector) {
    let (_, selectors) = GDT[crate::smp::cpu_index()]
        .get()
        .expect("GDT must be initialized before user mode is configured");
    (selectors.user_code, selectors.user_data)
}

/// Select the next task's syscall/interrupt entry stack before returning to it.
pub fn set_privilege_stack(top: u64) {
    assert!(!x86_64::instructions::interrupts::are_enabled());
    let tss = TSS[crate::smp::cpu_index()]
        .get()
        .expect("TSS must be initialized before scheduling");
    // TSS fields are packed; never form an aligned mutable reference. The CPU
    // reads RSP0 on the next ring transition, after this interrupt-masked switch.
    unsafe {
        core::ptr::addr_of_mut!((*tss.0.get()).privilege_stack_table)
            .cast::<VirtAddr>()
            .write_unaligned(VirtAddr::new(top));
    }
}

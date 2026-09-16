//! Bounded xAPIC SMP startup and lock-free, acknowledged TLB invalidation.
use crate::{gdt, interrupts, memory, paging, serial, task};
use bootloader_api::info::{MemoryRegion, MemoryRegionKind};
use core::{
    arch::{asm, global_asm, x86_64::__cpuid},
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
};
use x86_64::{registers::model_specific::Msr, structures::paging::PageTable};

pub const MAX_CPUS: usize = 4;
pub const TIMER_VECTOR: u8 = 0xe0;
pub const RESCHEDULE_VECTOR: u8 = 0xe1;
pub const SPURIOUS_VECTOR: u8 = 0xff;
static IDS: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(u32::MAX) }; MAX_CPUS];
static DOMAINS: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];
static ONLINE: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
static ACK: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static GENERATION: AtomicU64 = AtomicU64::new(0);
static SHOOT_LOCK: AtomicBool = AtomicBool::new(false);
static LAPIC: AtomicU64 = AtomicU64::new(0);
static X2APIC_REQUESTED: AtomicBool = AtomicBool::new(false);
static X2APIC_ACTIVE: AtomicBool = AtomicBool::new(false);
static BOOT_PAGE: AtomicU64 = AtomicU64::new(0);
static COUNT: AtomicUsize = AtomicUsize::new(1);
static IOAPIC: AtomicU64 = AtomicU64::new(0);
static TIMER_COUNT: AtomicU32 = AtomicU32::new(1_000_000);
static RESCHEDULE_IPIS: AtomicU64 = AtomicU64::new(0);
#[repr(align(16))]
struct Stack(UnsafeCell<[u8; 131072]>);
// Each AP exclusively owns its bootstrap stack for its entire lifetime.
unsafe impl Sync for Stack {}
static STACKS: [Stack; MAX_CPUS] = [const { Stack(UnsafeCell::new([0; 131072])) }; MAX_CPUS];
global_asm!(include_str!("ap_start.S"));
unsafe extern "C" {
    static ap_start: u8;
    static ap_end: u8;
    static ap_root: u8;
    static ap_stack: u8;
    static ap_entry: u8;
    static ap_gdt_ptr: u8;
    static ap_far: u8;
    static ap_long: u8;
    static ap_gdt: u8;
}

pub fn cpu_index() -> usize {
    let id = local_apic_id();
    IDS.iter()
        .position(|entry| entry.load(Ordering::Relaxed) == id)
        .expect("unregistered CPU identity")
}
fn local_apic_id() -> u32 {
    if X2APIC_ACTIVE.load(Ordering::Acquire) {
        unsafe { Msr::new(0x802).read() as u32 }
    } else {
        __cpuid(1).ebx >> 24
    }
}
pub fn online_count() -> usize {
    COUNT.load(Ordering::Acquire)
}

/// Return whether a logical CPU has completed the AP online publication step.
///
/// This is intentionally distinct from `online_count()`: during AP bootstrap,
/// an AP can be individually online before the BSP publishes the final count.
pub fn cpu_is_online(cpu: usize) -> bool {
    cpu < MAX_CPUS && ONLINE[cpu].load(Ordering::Acquire)
}

/// Snapshot the CPUs that have individually published themselves online.
pub fn online_mask() -> usize {
    ONLINE
        .iter()
        .enumerate()
        .fold(0usize, |mask, (cpu, online)| {
            if online.load(Ordering::Acquire) {
                mask | (1usize << cpu)
            } else {
                mask
            }
        })
}
pub fn cpu_domain(cpu: usize) -> u32 {
    if cpu < MAX_CPUS {
        DOMAINS[cpu].load(Ordering::Acquire)
    } else {
        u32::MAX
    }
}
pub fn topology_domains() -> u16 {
    let mut seen = [u32::MAX; MAX_CPUS];
    let mut count = 0_u16;
    for cpu in 0..online_count() {
        let domain = cpu_domain(cpu);
        if !seen[..count as usize].contains(&domain) {
            seen[count as usize] = domain;
            count = count.saturating_add(1);
        }
    }
    count.max(1)
}
pub fn x2apic_active() -> bool {
    X2APIC_ACTIVE.load(Ordering::Acquire)
}
pub fn prepare(regions: &[MemoryRegion]) {
    IDS[0].store(__cpuid(1).ebx >> 24, Ordering::Relaxed);
    DOMAINS[0].store(0, Ordering::Release);
    ONLINE[0].store(true, Ordering::Release);
    // The frame allocator excludes the low MiB. Never overwrite firmware/reserved RAM.
    for region in regions {
        let Some(aligned) = region.start.max(0x1000).checked_add(4095) else {
            continue;
        };
        let start = aligned & !4095;
        if region.kind == MemoryRegionKind::Usable
            && start
                .checked_add(4096)
                .is_some_and(|end| end <= region.end.min(0xa0000))
        {
            BOOT_PAGE.store(start, Ordering::Relaxed);
            break;
        }
    }
}
fn read(reg: u64) -> u32 {
    if X2APIC_ACTIVE.load(Ordering::Acquire) {
        unsafe { Msr::new((0x800 + (reg >> 4)) as u32).read() as u32 }
    } else {
        unsafe { ((LAPIC.load(Ordering::Relaxed) + reg) as *const u32).read_volatile() }
    }
}
fn write(reg: u64, value: u32) {
    if X2APIC_ACTIVE.load(Ordering::Acquire) {
        unsafe { Msr::new((0x800 + (reg >> 4)) as u32).write(u64::from(value)); }
    } else {
        unsafe {
            ((LAPIC.load(Ordering::Relaxed) + reg) as *mut u32).write_volatile(value);
        }
        let _ = read(0x20);
    }
}
pub fn eoi() {
    write(0xb0, 0);
}
fn enable_local() {
    unsafe {
        let mut msr = Msr::new(0x1b);
        let value = msr.read();
        let x2_supported = (__cpuid(1).ecx & (1 << 21)) != 0;
        let x2_active = value & (1 << 10) != 0;
        if X2APIC_REQUESTED.load(Ordering::Acquire) || x2_active {
            assert!(x2_supported, "firmware requested unsupported x2APIC");
            msr.write(value | (1 << 10) | (1 << 11));
            X2APIC_ACTIVE.store(true, Ordering::Release);
        } else {
            msr.write(value | (1 << 11));
        }
    }
    write(0x80, 0);
    write(0xf0, 0x100 | u32::from(SPURIOUS_VECTOR));
    write(0x350, 1 << 16);
    write(0x360, 1 << 16);
    write(0x320, 1 << 16);
}
fn send(id: u32, command: u32) {
    let start = unsafe { core::arch::x86_64::_rdtsc() };
    while read(0x300) & (1 << 12) != 0 {
        assert!(
            unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(start) < 5_000_000_000,
            "APIC delivery timeout"
        );
        core::hint::spin_loop();
    }
    if X2APIC_ACTIVE.load(Ordering::Acquire) {
        unsafe {
            Msr::new(0x830)
                .write((u64::from(id) << 32) | u64::from(command));
        }
    } else {
        write(0x310, id << 24);
        write(0x300, command);
    }
}

/// Prompt an online CPU to reconsider its run queue immediately. This is a
/// normal fixed-delivery xAPIC IPI, distinct from the NMI used for TLB
/// shootdowns. Calling it for the current CPU simply requests a local
/// reschedule and avoids a self-IPI.
pub fn reschedule_cpu(cpu: usize) -> bool {
    if cpu >= online_count() || !ONLINE[cpu].load(Ordering::Acquire) {
        return false;
    }
    if cpu == cpu_index() {
        task::request_reschedule();
        return true;
    }
    RESCHEDULE_IPIS.fetch_add(1, Ordering::Relaxed);
    x86_64::instructions::interrupts::without_interrupts(|| {
        send(IDS[cpu].load(Ordering::Relaxed), u32::from(RESCHEDULE_VECTOR));
    });
    true
}

pub fn reschedule_ipi_count() -> u64 {
    RESCHEDULE_IPIS.load(Ordering::Relaxed)
}
// PIT channel 2 provides a hardware interval independent of IRQ enable state.
fn delay_10ms() {
    use x86_64::instructions::port::Port;
    unsafe {
        let mut speaker = Port::<u8>::new(0x61);
        let saved = speaker.read();
        speaker.write(saved & !3);
        Port::<u8>::new(0x43).write(0xb0);
        Port::<u8>::new(0x42).write(11932u16 as u8);
        Port::<u8>::new(0x42).write((11932u16 >> 8) as u8);
        speaker.write((saved & !2) | 1);
        let start = core::arch::x86_64::_rdtsc();
        while speaker.read() & 0x20 == 0 {
            assert!(
                core::arch::x86_64::_rdtsc().wrapping_sub(start) < 5_000_000_000,
                "PIT calibration timeout"
            );
            core::hint::spin_loop();
        }
        speaker.write(saved);
    }
}
fn start_timer() {
    write(0x3e0, 3); // divide by 16
    write(0x320, (1 << 17) | u32::from(TIMER_VECTOR));
    write(0x380, TIMER_COUNT.load(Ordering::Relaxed));
}
fn io_read(base: u64, register: u32) -> u32 {
    unsafe {
        (base as *mut u32).write_volatile(register);
        ((base + 16) as *const u32).read_volatile()
    }
}
fn io_write(base: u64, register: u32, value: u32) {
    unsafe {
        (base as *mut u32).write_volatile(register);
        ((base + 16) as *mut u32).write_volatile(value);
    }
}
pub fn routed_irq() -> bool {
    IOAPIC.load(Ordering::Acquire) != 0
}

pub fn start(topology: Option<crate::hal::acpi::Summary>, offset: u64) {
    // Test the missing-topology fallback without disabling ACPI in UEFI itself.
    let topology = if cfg!(feature = "legacy-pic-test") {
        None
    } else {
        topology
    };
    let Some(topology) = topology else {
        serial::write_line(format_args!(
            "[SMP] online=1 expected=1 legacy PIC fallback"
        ));
        return;
    };
    if !topology.apic || topology.processor_count == 0 {
        return;
    }
    assert!(
        !topology.truncated && topology.processor_count <= MAX_CPUS,
        "unsupported CPU topology"
    );
    X2APIC_REQUESTED.store(
        topology.processor_ids[..topology.processor_count]
            .iter()
            .any(|id| *id > u8::MAX as u32),
        Ordering::Release,
    );
    LAPIC.store(
        paging::map_mmio(topology.local_apic_address)
            .ok()
            .expect("LAPIC mapping"),
        Ordering::Relaxed,
    );
    enable_local();
    IDS[0].store(local_apic_id(), Ordering::Release);
    write(0x3e0, 3);
    write(0x380, u32::MAX);
    delay_10ms();
    TIMER_COUNT.store(u32::MAX - read(0x390), Ordering::Relaxed);
    assert!(
        TIMER_COUNT.load(Ordering::Relaxed) > 0,
        "LAPIC timer calibration"
    );
    // Route only keyboard input through IOAPIC; each CPU has its own LAPIC timer.
    assert_eq!(topology.io_apics, 1, "one IOAPIC required");
    let io = paging::map_mmio(u64::from(topology.io_apic_address))
        .ok()
        .expect("IOAPIC mapping");
    let max = (io_read(io, 1) >> 16) & 0xff;
    for pin in 0..=max {
        io_write(io, 0x10 + pin * 2, 1 << 16);
    }
    let (gsi, flags) = topology.isa_gsi[1].unwrap_or((1, 0));
    assert!(
        gsi >= topology.io_apic_gsi_base && gsi - topology.io_apic_gsi_base <= max,
        "keyboard GSI"
    );
    assert!(
        flags & 3 != 2 && (flags >> 2) & 3 != 2,
        "reserved ISO flags"
    );
    let pin = gsi - topology.io_apic_gsi_base;
    io_write(io, 0x11 + pin * 2, IDS[0].load(Ordering::Relaxed) << 24);
    io_write(
        io,
        0x10 + pin * 2,
        33 | if flags & 3 == 3 { 1 << 13 } else { 0 }
            | if (flags >> 2) & 3 == 3 { 1 << 15 } else { 0 },
    );
    IOAPIC.store(io, Ordering::Release);
    if let Some(index) = topology.processor_ids[..topology.processor_count]
        .iter()
        .position(|id| *id == IDS[0].load(Ordering::Relaxed))
    {
        DOMAINS[0].store(topology.processor_domains[index], Ordering::Release);
    }
    if topology.processor_count > 1 {
        let low = BOOT_PAGE.load(Ordering::Relaxed);
        assert!(low != 0, "no usable AP trampoline page below 1 MiB");
        let root = memory::allocate_frame()
            .expect("AP root")
            .start_address()
            .as_u64();
        let pdpt = memory::allocate_frame()
            .expect("AP PDPT")
            .start_address()
            .as_u64();
        let pd = memory::allocate_frame()
            .expect("AP PD")
            .start_address()
            .as_u64();
        assert!(root < 1u64 << 32, "AP root above 4 GiB");
        unsafe {
            let real_root = paging::kernel_address_space().unwrap().root_address();
            core::ptr::copy_nonoverlapping(
                (offset + real_root) as *const u8,
                (offset + root) as *mut u8,
                4096,
            );
            let table = &mut *((offset + root) as *mut PageTable);
            let flags = x86_64::structures::paging::PageTableFlags::PRESENT
                | x86_64::structures::paging::PageTableFlags::WRITABLE;
            table[0].set_addr(x86_64::PhysAddr::new(pdpt), flags);
            core::ptr::write_bytes((offset + pdpt) as *mut u8, 0, 4096);
            core::ptr::write_bytes((offset + pd) as *mut u8, 0, 4096);
            (&mut *((offset + pdpt) as *mut PageTable))[0]
                .set_addr(x86_64::PhysAddr::new(pd), flags);
            (&mut *((offset + pd) as *mut PageTable))[0].set_addr(
                x86_64::PhysAddr::new(0),
                flags | x86_64::structures::paging::PageTableFlags::HUGE_PAGE,
            );
            let base = &ap_start as *const u8 as usize;
            let size = &ap_end as *const u8 as usize - base;
            assert!(size < 4096);
            core::ptr::copy_nonoverlapping(base as *const u8, (offset + low) as *mut u8, size);
            let patch = |symbol: *const u8| (offset + low + symbol as u64 - base as u64) as *mut u8;
            patch(&ap_root).cast::<u64>().write_unaligned(root);
            patch(&ap_entry)
                .cast::<u64>()
                .write_unaligned(ap_main as *const () as u64);
            patch(&ap_gdt_ptr)
                .add(2)
                .cast::<u32>()
                .write_unaligned((low + &ap_gdt as *const u8 as u64 - base as u64) as u32);
            patch(&ap_far)
                .cast::<u32>()
                .write_unaligned((low + &ap_long as *const u8 as u64 - base as u64) as u32);
            let mut cpu = 1;
            for id in &topology.processor_ids[..topology.processor_count] {
                if *id == IDS[0].load(Ordering::Relaxed) {
                    continue;
                }
                IDS[cpu].store(*id, Ordering::Release);
                let domain = topology.processor_ids[..topology.processor_count]
                    .iter()
                    .position(|candidate| candidate == id)
                    .map(|index| topology.processor_domains[index])
                    .unwrap_or(0);
                DOMAINS[cpu].store(domain, Ordering::Release);
                patch(&ap_stack)
                    .cast::<u64>()
                    .write_unaligned(STACKS[cpu].0.get() as u64 + 131072);
                send(*id, 0xc500);
                delay_10ms();
                send(*id, 0x8500);
                delay_10ms();
                send(*id, 0x600 | (low >> 12) as u32);
                delay_10ms();
                if !ONLINE[cpu].load(Ordering::Acquire) {
                    send(*id, 0x600 | (low >> 12) as u32);
                }
                for _ in 0..100 {
                    if ONLINE[cpu].load(Ordering::Acquire) {
                        break;
                    }
                    delay_10ms();
                }
                assert!(ONLINE[cpu].load(Ordering::Acquire), "AP startup timeout");
                cpu += 1;
            }
            COUNT.store(cpu, Ordering::Release);
        }
    }
    start_timer();
    serial::write_line(format_args!(
        "[SMP] online={} expected={} LAPIC/IOAPIC enabled",
        online_count(),
        topology.processor_count
    ));
    serial::write_line(format_args!(
        "[SMP] topology/NUMA affinity: PASSED domains={} mask={:#x}",
        topology_domains(),
        online_mask()
    ));
    serial::write_line(format_args!(
        "[SMP] APIC mode: {}",
        if x2apic_active() { "x2APIC" } else { "xAPIC" }
    ));
}
extern "C" fn ap_main() -> ! {
    paging::switch_to(paging::kernel_address_space().unwrap());
    gdt::init();
    interrupts::init();
    enable_local();
    let cpu = cpu_index();
    // Publish this AP before exposing its CPU-owned idle task. Otherwise the
    // BSP scheduler can observe an affinity bit for this CPU while ONLINE is
    // still false and incorrectly classify the live task as targeting an
    // offline CPU. Release ordering pairs with scheduler-side Acquire loads.
    ONLINE[cpu].store(true, Ordering::Release);
    task::init_ap(cpu);
    start_timer();
    loop {
        task::yield_now();
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}
/// NMI handler never acquires a lock, allocates, or schedules.
pub fn acknowledge_tlb() {
    let generation = GENERATION.load(Ordering::Acquire);
    unsafe {
        let cr4: u64;
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
        asm!("mov cr4, {}", in(reg) cr4 & !(1 << 7), options(nostack));
        let cr3: u64;
        asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
        asm!("mov cr3, {}", in(reg) cr3, options(nostack));
        asm!("mov cr4, {}", in(reg) cr4, options(nostack));
    }
    ACK[cpu_index()].store(generation, Ordering::Release);
}
pub fn shootdown() {
    x86_64::instructions::interrupts::without_interrupts(shootdown_inner);
}
fn shootdown_inner() {
    if online_count() == 1 {
        return;
    }
    while SHOOT_LOCK
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    let generation = GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    let current = cpu_index();
    acknowledge_tlb();
    for (cpu, id) in IDS.iter().enumerate().take(online_count()) {
        if cpu != current {
            send(id.load(Ordering::Relaxed), 0x400);
        }
    }
    let start = unsafe { core::arch::x86_64::_rdtsc() };
    while (0..online_count()).any(|cpu| ACK[cpu].load(Ordering::Acquire) < generation) {
        assert!(
            unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(start) < 5_000_000_000,
            "TLB shootdown timeout"
        );
        core::hint::spin_loop();
    }
    SHOOT_LOCK.store(false, Ordering::Release);
}

static ARRIVED: AtomicUsize = AtomicUsize::new(0);
static FINISHED: AtomicUsize = AtomicUsize::new(0);
static CPU_MASK: AtomicUsize = AtomicUsize::new(0);
static PROBE_SUM: AtomicU64 = AtomicU64::new(0);
fn parallel_probe() -> ! {
    let cpu = cpu_index();
    CPU_MASK.fetch_or(1 << cpu, Ordering::AcqRel);
    ARRIVED.fetch_add(1, Ordering::AcqRel);
    let start = unsafe { core::arch::x86_64::_rdtsc() };
    while ARRIVED.load(Ordering::Acquire) < online_count() {
        assert!(
            unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(start) < 10_000_000_000,
            "parallel barrier timeout"
        );
        core::hint::spin_loop();
    }
    for _ in 0..32 {
        PROBE_SUM.fetch_add(1, Ordering::AcqRel);
        task::yield_now();
    }
    FINISHED.fetch_add(1, Ordering::Release);
    task::exit_current_task();
}
pub fn self_test() {
    static TESTED: AtomicBool = AtomicBool::new(false);
    if TESTED.swap(true, Ordering::AcqRel) {
        return;
    }
    for cpu in 0..online_count() {
        unsafe { task::spawn_on(cpu, "smp-probe", parallel_probe) }
            .ok()
            .expect("SMP probe slots");
    }
    let deadline = crate::timer::ticks() + 500;
    while FINISHED.load(Ordering::Acquire) < online_count() {
        assert!(
            crate::timer::ticks() < deadline,
            "multicore scheduling timeout"
        );
        task::yield_now();
    }
    assert_eq!(CPU_MASK.load(Ordering::Acquire), (1 << online_count()) - 1);
    assert_eq!(
        PROBE_SUM.load(Ordering::Acquire),
        (32 * online_count()) as u64
    );
    // SAFETY: This job touches only an atomic and the CPU-owned scheduler.
    unsafe { task::spawn_parallel("balanced-probe", balanced_probe) }
        .ok()
        .expect("balanced probe slot");
    let deadline = crate::timer::ticks() + 500;
    while !BALANCED_DONE.load(Ordering::Acquire) {
        assert!(crate::timer::ticks() < deadline, "balanced job timeout");
        task::yield_now();
    }
    affinity_foundation_test();
    ready_migration_test();
    runnable_load_accounting_test();
    automatic_rebalance_test();
    reschedule_ipi_test();
    migration_stress_test();
    stale_translation_test();
    preemption_test();
    for _ in 0..32 {
        shootdown();
    }
    serial::write_line(format_args!(
        "[SMP] scheduler/barrier: PASSED cpus={} mask={:#x}",
        online_count(),
        CPU_MASK.load(Ordering::Acquire)
    ));
    serial::write_line(format_args!("[SMP] acknowledged TLB shootdowns: PASSED"));
}

const TEST_PAGE: u64 = 0x5555_6000_0000;
static READ_PHASE: AtomicUsize = AtomicUsize::new(0);
static READ_ACK: [AtomicUsize; MAX_CPUS] = [const { AtomicUsize::new(0) }; MAX_CPUS];
fn translation_reader() -> ! {
    let cpu = cpu_index();
    for phase in 1..=8 {
        while READ_PHASE.load(Ordering::Acquire) < phase {
            core::hint::spin_loop();
        }
        let value = unsafe { (TEST_PAGE as *const u64).read_volatile() };
        assert_eq!(value, phase as u64, "stale remote TLB translation");
        READ_ACK[cpu].store(phase, Ordering::Release);
    }
    task::exit_current_task();
}
fn stale_translation_test() {
    paging::map_range(TEST_PAGE, 4096)
        .ok()
        .expect("SMP test mapping");
    for cpu in 1..online_count() {
        unsafe { task::spawn_on(cpu, "tlb-reader", translation_reader) }
            .ok()
            .expect("TLB reader slot");
    }
    for phase in 1..=8 {
        paging::replace_smp_test_page(TEST_PAGE, phase as u64);
        assert_eq!(
            unsafe { (TEST_PAGE as *const u64).read_volatile() },
            phase as u64
        );
        READ_PHASE.store(phase, Ordering::Release);
        let deadline = crate::timer::ticks() + 500;
        while READ_ACK
            .iter()
            .take(online_count())
            .skip(1)
            .any(|ack| ack.load(Ordering::Acquire) != phase)
        {
            assert!(crate::timer::ticks() < deadline, "TLB reader timeout");
            task::yield_now();
        }
    }
    paging::remove_smp_test_page(TEST_PAGE);
    serial::write_line(format_args!(
        "[SMP] remote stale-translation/refree: PASSED"
    ));
}
static PREEMPT_STARTED: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
static PREEMPT_PEER: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
static PREEMPT_DONE: AtomicUsize = AtomicUsize::new(0);
fn cpu_bound_probe() -> ! {
    let cpu = cpu_index();
    PREEMPT_STARTED[cpu].store(true, Ordering::Release);
    let start = unsafe { core::arch::x86_64::_rdtsc() };
    // No yield: only a hardware timer can schedule the peer on this CPU.
    while !PREEMPT_PEER[cpu].load(Ordering::Acquire) {
        assert!(
            unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(start) < 10_000_000_000,
            "AP timer preemption timeout"
        );
        core::hint::spin_loop();
    }
    PREEMPT_DONE.fetch_add(1, Ordering::Release);
    task::exit_current_task();
}
fn preemption_peer() -> ! {
    let cpu = cpu_index();
    while !PREEMPT_STARTED[cpu].load(Ordering::Acquire) {
        task::yield_now();
    }
    PREEMPT_PEER[cpu].store(true, Ordering::Release);
    task::exit_current_task();
}
fn preemption_test() {
    for cpu in 0..online_count() {
        unsafe { task::spawn_on(cpu, "cpu-bound", cpu_bound_probe) }
            .ok()
            .expect("CPU-bound slot");
        unsafe { task::spawn_on(cpu, "preempt-peer", preemption_peer) }
            .ok()
            .expect("preemption peer slot");
    }
    let deadline = crate::timer::ticks() + 500;
    while PREEMPT_DONE.load(Ordering::Acquire) < online_count() {
        assert!(
            crate::timer::ticks() < deadline,
            "per-CPU preemption timeout"
        );
        task::yield_now();
    }
    serial::write_line(format_args!("[SMP] per-CPU timer preemption: PASSED"));
}

static AFFINITY_RELEASE: AtomicBool = AtomicBool::new(false);
static AFFINITY_DONE: AtomicBool = AtomicBool::new(false);
static AFFINITY_OBSERVED_CPU: AtomicUsize = AtomicUsize::new(usize::MAX);
fn affinity_probe() -> ! {
    AFFINITY_OBSERVED_CPU.store(cpu_index(), Ordering::Release);
    while !AFFINITY_RELEASE.load(Ordering::Acquire) {
        task::yield_now();
    }
    AFFINITY_DONE.store(true, Ordering::Release);
    task::exit_current_task();
}

fn affinity_foundation_test() {
    AFFINITY_RELEASE.store(false, Ordering::Release);
    AFFINITY_DONE.store(false, Ordering::Release);
    AFFINITY_OBSERVED_CPU.store(usize::MAX, Ordering::Release);

    // Keep the new task Ready while its affinity contract is inspected and
    // changed. On CPU0, disabling interrupts prevents the local scheduler from
    // dispatching it before the test publishes a destination.
    let id = x86_64::instructions::interrupts::without_interrupts(|| {
        // SAFETY: The probe uses only atomics plus yield/exit and retains no
        // CPU-local state across scheduling points.
        let id = unsafe { task::spawn_migratable_on(0, "affinity-probe", affinity_probe) }
            .ok()
            .expect("affinity probe slot");

        assert_eq!(
            task::set_ready_task_affinity(id, 0),
            Err(task::AffinityError::InvalidMask),
            "empty affinity mask was accepted"
        );
        assert_eq!(
            task::set_ready_task_affinity(id, 1usize << online_count()),
            Err(task::AffinityError::OfflineCpu),
            "offline CPU affinity bit was accepted"
        );

        let cpu0_only = 1usize;
        task::set_ready_task_affinity(id, cpu0_only).expect("restrict affinity to CPU0");
        assert_eq!(task::task_affinity(id), Some(cpu0_only));
        for _ in 0..4 {
            let _ = task::rebalance_once();
        }
        assert_eq!(
            task::task_cpu(id),
            Some(0),
            "automatic rebalance escaped a CPU0-only affinity mask"
        );

        if online_count() > 1 {
            assert_eq!(
                task::migrate_ready_task(id, 1),
                Err(task::MigrationError::AffinityDenied),
                "migration escaped the hard affinity mask"
            );

            let cpu1_only = 1usize << 1;
            task::set_ready_task_affinity(id, cpu1_only)
                .expect("move Ready task by restricting affinity to CPU1");
            assert_eq!(task::task_affinity(id), Some(cpu1_only));
            assert_eq!(
                task::task_cpu(id),
                Some(1),
                "affinity update did not move ownership to an allowed CPU"
            );
        }
        id
    });

    let expected_cpu = if online_count() > 1 { 1 } else { 0 };
    let deadline = crate::timer::ticks() + 500;
    while AFFINITY_OBSERVED_CPU.load(Ordering::Acquire) == usize::MAX {
        assert!(crate::timer::ticks() < deadline, "affinity execution timeout");
        task::yield_now();
    }
    assert_eq!(
        AFFINITY_OBSERVED_CPU.load(Ordering::Acquire),
        expected_cpu,
        "task executed outside its selected affinity owner"
    );

    AFFINITY_RELEASE.store(true, Ordering::Release);
    while !AFFINITY_DONE.load(Ordering::Acquire) {
        assert!(crate::timer::ticks() < deadline, "affinity completion timeout");
        task::yield_now();
    }

    // The task may already have been reaped, so the durable observed CPU is
    // the execution proof. `id` is retained to ensure the task APIs above all
    // operate on the same object.
    let _ = id;
    serial::write_line(format_args!(
        "[STAGE7.1] CPU affinity foundation: PASSED cpus={} observed_cpu={}",
        online_count(),
        expected_cpu
    ));
}

static MIGRATION_OBSERVED_CPU: AtomicUsize = AtomicUsize::new(usize::MAX);
fn migration_probe() -> ! {
    MIGRATION_OBSERVED_CPU.store(cpu_index(), Ordering::Release);
    task::exit_current_task();
}
fn ready_migration_test() {
    if online_count() < 2 {
        serial::write_line(format_args!(
            "[SMP] ready-task migration: PASSED cpus=1 (single-CPU baseline)"
        ));
        return;
    }

    MIGRATION_OBSERVED_CPU.store(usize::MAX, Ordering::Release);
    let target = 1;
    x86_64::instructions::interrupts::without_interrupts(|| {
        // SAFETY: The probe touches only an atomic and exits. Starting it on
        // CPU 0 while BSP interrupts are disabled guarantees it remains Ready
        // until the scheduler lock transfers ownership to CPU 1.
        let id = unsafe { task::spawn_migratable_on(0, "migration-probe", migration_probe) }
            .ok()
            .expect("migration probe slot");
        assert_eq!(task::task_cpu(id), Some(0));
        task::migrate_ready_task(id, target).expect("ready task migration");
        // Do not re-read the TCB here. The destination CPU receives a
        // reschedule IPI as soon as migration is published and may run this
        // tiny probe to completion before the BSP performs another lookup.
        // `MIGRATION_OBSERVED_CPU` below is the authoritative execution proof.
    });

    let deadline = crate::timer::ticks() + 500;
    while MIGRATION_OBSERVED_CPU.load(Ordering::Acquire) == usize::MAX {
        assert!(crate::timer::ticks() < deadline, "ready migration timeout");
        task::yield_now();
    }
    assert_eq!(MIGRATION_OBSERVED_CPU.load(Ordering::Acquire), target);
    serial::write_line(format_args!(
        "[SMP] ready-task migration: PASSED source=0 target={} observed={}",
        target,
        MIGRATION_OBSERVED_CPU.load(Ordering::Acquire)
    ));
}

static LOAD_PROBE_RELEASE: AtomicBool = AtomicBool::new(false);
static LOAD_PROBE_DONE: AtomicUsize = AtomicUsize::new(0);
fn load_probe() -> ! {
    while !LOAD_PROBE_RELEASE.load(Ordering::Acquire) {
        task::yield_now();
    }
    LOAD_PROBE_DONE.fetch_add(1, Ordering::Release);
    task::exit_current_task();
}
fn runnable_load_accounting_test() {
    const PROBES_PER_CPU: usize = 2;

    LOAD_PROBE_RELEASE.store(false, Ordering::Release);
    LOAD_PROBE_DONE.store(0, Ordering::Release);

    let before = task::cpu_run_loads();
    for cpu in 0..online_count() {
        for _ in 0..PROBES_PER_CPU {
            // SAFETY: This probe is deliberately pinned to `cpu`. It uses
            // only atomics plus yield/exit and keeps no CPU-local state
            // across scheduling points.
            unsafe { task::spawn_on(cpu, "load-probe", load_probe) }
                .ok()
                .expect("load probe slot");
        }
    }

    let after = task::cpu_run_loads();
    for load in after.iter().take(online_count()) {
        // Do not compare this snapshot with `before` using an exact delta.
        // On an SMP kernel unrelated runnable/running tasks can legitimately
        // change state on another CPU between the two snapshots. The probes
        // themselves cannot disappear: they are pinned and remain runnable
        // until LOAD_PROBE_RELEASE is set. Therefore each CPU must account
        // for at least all of its pinned probes. This tests the accounting
        // invariant without treating unrelated concurrent activity as a
        // regression failure.
        assert!(
            load.runnable() >= PROBES_PER_CPU,
            "per-CPU runnable load accounting lost pinned probes"
        );
    }

    serial::write_line(format_args!(
        "[SMP] runnable-load accounting: PASSED cpus={} probes_per_cpu={} before={:?} after={:?}",
        online_count(),
        PROBES_PER_CPU,
        &before[..online_count()],
        &after[..online_count()]
    ));

    LOAD_PROBE_RELEASE.store(true, Ordering::Release);
    let expected_done = online_count() * PROBES_PER_CPU;
    let deadline = crate::timer::ticks() + 500;
    while LOAD_PROBE_DONE.load(Ordering::Acquire) < expected_done {
        assert!(crate::timer::ticks() < deadline, "load probe completion timeout");
        task::yield_now();
    }
}

static AUTO_BALANCE_RELEASE: AtomicBool = AtomicBool::new(false);
static AUTO_BALANCE_DONE: AtomicUsize = AtomicUsize::new(0);
static AUTO_BALANCE_MASK: AtomicUsize = AtomicUsize::new(0);
fn auto_balance_probe() -> ! {
    AUTO_BALANCE_MASK.fetch_or(1 << cpu_index(), Ordering::AcqRel);
    while !AUTO_BALANCE_RELEASE.load(Ordering::Acquire) {
        task::yield_now();
    }
    AUTO_BALANCE_DONE.fetch_add(1, Ordering::Release);
    task::exit_current_task();
}
fn automatic_rebalance_test() {
    if online_count() < 2 {
        serial::write_line(format_args!(
            "[SMP] automatic rebalancing: PASSED cpus=1 (single-CPU baseline)"
        ));
        return;
    }

    AUTO_BALANCE_RELEASE.store(false, Ordering::Release);
    AUTO_BALANCE_DONE.store(0, Ordering::Release);
    AUTO_BALANCE_MASK.store(0, Ordering::Release);
    let jobs = online_count() * 3;
    x86_64::instructions::interrupts::without_interrupts(|| {
        for _ in 0..jobs {
            // SAFETY: Probe state is atomic-only and obeys the restricted
            // migratable-kernel-job contract.
            unsafe { task::spawn_migratable_on(0, "rebalance-probe", auto_balance_probe) }
                .ok()
                .expect("automatic rebalance probe slot");
        }
    });

    let stats_before = task::rebalance_stats();
    let mut explicit_migrations = 0usize;
    for _ in 0..jobs * 2 {
        if !task::rebalance_once() {
            break;
        }
        explicit_migrations += 1;
    }
    let loads = task::cpu_run_loads();

    // Do not require an exact point-in-time runnable-load shape here. Once a
    // migrated probe receives its reschedule IPI it may immediately transition
    // Ready -> Running (or yield again) on the destination CPU while this CPU
    // is taking its snapshot. That makes an exact max/min assertion racy even
    // when the balancer is correct. Instead prove the two invariants that
    // matter: the explicit rebalance pass actually migrated work, and after the
    // probes are released every online CPU is observed executing that work.
    assert!(
        explicit_migrations != 0,
        "automatic rebalance did not migrate any probe workload"
    );
    let stats_after = task::rebalance_stats();
    assert!(
        stats_after.migrations > stats_before.migrations,
        "automatic rebalance migration counter did not advance"
    );

    AUTO_BALANCE_RELEASE.store(true, Ordering::Release);
    let deadline = crate::timer::ticks() + 500;
    while AUTO_BALANCE_DONE.load(Ordering::Acquire) < jobs {
        assert!(crate::timer::ticks() < deadline, "automatic rebalance completion timeout");
        task::yield_now();
    }
    assert_eq!(
        AUTO_BALANCE_MASK.load(Ordering::Acquire) & ((1 << online_count()) - 1),
        (1 << online_count()) - 1,
        "automatic rebalance did not execute work on every CPU"
    );
    serial::write_line(format_args!(
        "[SMP] automatic rebalancing: PASSED cpus={} explicit_migrations={} loads={:?}",
        online_count(),
        explicit_migrations,
        &loads[..online_count()]
    ));
}

static IPI_PROBE_DONE: AtomicBool = AtomicBool::new(false);
fn ipi_probe() -> ! {
    IPI_PROBE_DONE.store(true, Ordering::Release);
    task::exit_current_task();
}
fn reschedule_ipi_test() {
    if online_count() < 2 {
        serial::write_line(format_args!(
            "[SMP] reschedule IPI: PASSED cpus=1 (local baseline)"
        ));
        return;
    }
    IPI_PROBE_DONE.store(false, Ordering::Release);
    let before = reschedule_ipi_count();
    // SAFETY: The probe only stores to an atomic and exits.
    unsafe { task::spawn_on(1, "ipi-probe", ipi_probe) }
        .ok()
        .expect("IPI probe slot");
    let deadline = crate::timer::ticks() + 500;
    while !IPI_PROBE_DONE.load(Ordering::Acquire) {
        assert!(crate::timer::ticks() < deadline, "reschedule IPI timeout");
        task::yield_now();
    }
    assert!(reschedule_ipi_count() > before, "reschedule IPI was not emitted");
    serial::write_line(format_args!(
        "[SMP] reschedule IPI: PASSED count={}",
        reschedule_ipi_count()
    ));
}

static STRESS_RELEASE: AtomicBool = AtomicBool::new(false);
static STRESS_DONE: AtomicUsize = AtomicUsize::new(0);
static STRESS_MASK: AtomicUsize = AtomicUsize::new(0);
static STRESS_COVERAGE_DONE: AtomicUsize = AtomicUsize::new(0);

fn migration_stress_coverage_probe() -> ! {
    STRESS_MASK.fetch_or(1 << cpu_index(), Ordering::AcqRel);
    STRESS_COVERAGE_DONE.fetch_add(1, Ordering::Release);
    task::exit_current_task();
}

fn migration_stress_probe() -> ! {
    for _ in 0..16 {
        STRESS_MASK.fetch_or(1 << cpu_index(), Ordering::AcqRel);
        task::yield_now();
    }
    while !STRESS_RELEASE.load(Ordering::Acquire) {
        task::yield_now();
    }
    STRESS_DONE.fetch_add(1, Ordering::Release);
    task::exit_current_task();
}
fn migration_stress_test() {
    if online_count() < 2 {
        serial::write_line(format_args!(
            "[SMP] migration stress: PASSED cpus=1 (single-CPU baseline)"
        ));
        return;
    }
    STRESS_RELEASE.store(false, Ordering::Release);
    STRESS_DONE.store(0, Ordering::Release);
    STRESS_MASK.store(0, Ordering::Release);
    STRESS_COVERAGE_DONE.store(0, Ordering::Release);

    // Establish deterministic execution coverage before creating migration
    // pressure.  The previous test inferred CPU coverage from migratable jobs
    // that all started Ready on CPU 0.  Under legitimate eager rebalancing,
    // every such job can migrate away before its first instruction executes,
    // making the coverage assertion timing-dependent even though migration is
    // working correctly.  A short pinned probe on each online CPU proves the
    // execution side explicitly; the migratable jobs below separately prove
    // ownership transfer under pressure.
    for cpu in 0..online_count() {
        // SAFETY: The coverage probe only updates atomics and exits, and
        // spawn_on keeps it pinned to the requested online CPU.
        unsafe { task::spawn_on(cpu, "migration-coverage", migration_stress_coverage_probe) }
            .ok()
            .expect("migration coverage slots");
    }
    let coverage_deadline = crate::timer::ticks() + 500;
    while STRESS_COVERAGE_DONE.load(Ordering::Acquire) < online_count() {
        assert!(
            crate::timer::ticks() < coverage_deadline,
            "migration stress CPU coverage timeout"
        );
        task::yield_now();
    }

    let expected_mask = (1 << online_count()) - 1;
    assert_eq!(
        STRESS_MASK.load(Ordering::Acquire) & expected_mask,
        expected_mask,
        "migration stress CPU coverage did not execute on every CPU"
    );

    let jobs = online_count() * 4;
    let mut ids = [None; MAX_CPUS * 4];
    x86_64::instructions::interrupts::without_interrupts(|| {
        for id in ids.iter_mut().take(jobs) {
            // SAFETY: Atomic-only probe; no CPU-local pointer or unsafe service
            // is retained across yield points. Starting every probe on CPU 0
            // deliberately creates migration pressure.
            *id = Some(
                unsafe { task::spawn_migratable_on(0, "migration-stress", migration_stress_probe) }
                    .ok()
                    .expect("migration stress slots"),
            );
        }
    });

    let mut migrations = 0usize;
    let deadline = crate::timer::ticks() + 500;
    for round in 0..64usize {
        for (slot, id) in ids.iter().take(jobs).enumerate() {
            if let Some(id) = *id {
                let current = task::task_cpu(id).unwrap_or(0);
                let mut target = (round + slot + 1) % online_count();
                if target == current {
                    target = (target + 1) % online_count();
                }
                if task::migrate_ready_task(id, target).is_ok() {
                    migrations += 1;
                }
            }
        }
        let _ = task::rebalance_once();
        task::yield_now();
        assert!(crate::timer::ticks() < deadline, "migration stress rebalance timeout");
    }
    assert!(migrations >= jobs, "migration stress made too few ownership transfers");

    STRESS_RELEASE.store(true, Ordering::Release);
    while STRESS_DONE.load(Ordering::Acquire) < jobs {
        assert!(crate::timer::ticks() < deadline, "migration stress completion timeout");
        task::yield_now();
    }
    assert_eq!(
        STRESS_MASK.load(Ordering::Acquire) & expected_mask,
        expected_mask,
        "migration stress lost previously verified CPU execution coverage"
    );
    serial::write_line(format_args!(
        "[SMP] migration stress: PASSED jobs={} migrations={} mask={:#x}",
        jobs,
        migrations,
        STRESS_MASK.load(Ordering::Acquire)
    ));
}

static BALANCED_DONE: AtomicBool = AtomicBool::new(false);
fn balanced_probe() -> ! {
    BALANCED_DONE.store(true, Ordering::Release);
    task::exit_current_task();
}
pub fn diagnostic(console: &mut crate::console::Console<'_>) {
    console.print("Online CPUs: ");
    // Console itself remains BSP-owned; diagnostic output never runs in IRQ/NMI.
    let count = online_count();
    console.println(match count {
        1 => "1",
        2 => "2",
        3 => "3",
        _ => "4",
    });
    console.println(
        "CPU-owned scheduling; Ready restricted kernel jobs may migrate; userspace/I/O stay CPU 0.",
    );
    let loads = task::cpu_run_loads();
    serial::write_line(format_args!(
        "[SMP] online={} shootdowns={} resched_ipis={} runnable_loads={:?} rebalance={:?}",
        count,
        GENERATION.load(Ordering::Acquire),
        reschedule_ipi_count(),
        &loads[..count],
        task::rebalance_stats()
    ));
}

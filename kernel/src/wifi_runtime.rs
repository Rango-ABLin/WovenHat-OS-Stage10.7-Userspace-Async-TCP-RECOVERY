//! Event-driven AC receive-cause worker for the opt-in Wi-Fi build.
//!
//! One kernel-owned subscription; the slot lock serializes service and detach.
//! Hardware attachment is deliberately not inferred from PCI discovery. The
//! probe owns synthetic CSR storage and exercises the real IDT/scheduler path.
//! A physical binding must first own initialized RX DMA and quiesce MSI before
//! retiring its vector; software epochs cannot identify late untagged PCI MSIs.
use crate::{irq_lock::IrqMutex, irq_mailbox::Mailbox, task, timer, wifi_hw::*};
use core::sync::atomic::{AtomicU64, Ordering};

static MAILBOX: Mailbox = Mailbox::new();
static WORKER: AtomicU64 = AtomicU64::new(0);
static RESULT: AtomicU64 = AtomicU64::new(0);
static PRODUCER_CPU: AtomicU64 = AtomicU64::new(0);

struct Binding {
    owner: task::TaskId,
    epoch: u64,
    controller: IntelRxInterruptController,
    page: DmaPage,
    completion: Option<Result<Ax200DeferredServiceEvent, Ax200DeferredServiceError>>,
    faulted: bool,
}
struct State {
    next_epoch: u64,
    binding: Option<Binding>,
}
static STATE: IrqMutex<State> = IrqMutex::with_rank(
    State {
        next_epoch: 1,
        binding: None,
    },
    20,
);

/// Called after LAPIC EOI, with no device lock held. Scheduler notification
/// latches a permit, covering an interrupt immediately before worker blocking.
/// All IRQ-mutex holders mask local IRQs; the sole IRQ-enabled PreemptMutex
/// (terminal, rank 10) may nest the rank-10 scheduler notification safely.
pub fn notify_from_irq() {
    let epoch = MAILBOX.epoch();
    if MAILBOX.publish(epoch) {
        let id = WORKER.load(Ordering::Acquire);
        if id != 0 {
            let _ = task::signal_event(task::TaskId::from_u64(id));
        }
    }
}

fn worker() -> ! {
    loop {
        if let Some(epoch) = MAILBOX.claim() {
            let owner = {
                let mut state = STATE.lock();
                if let Some(binding) = state
                    .binding
                    .as_mut()
                    .filter(|b| b.epoch == epoch && !b.faulted)
                {
                    let result = service_deferred_ax200_interrupt(&mut binding.controller);
                    if result.is_err() {
                        binding.faulted = true;
                        MAILBOX.retire(epoch);
                        let _ = binding.controller.disable();
                    }
                    // One bounded coalescing completion. Errors are sticky;
                    // successful snapshots retain all observed cause bits.
                    binding.completion = Some(match (binding.completion.take(), result) {
                        (Some(Err(error)), _) => Err(error),
                        (_, Err(error)) => Err(error),
                        (Some(Ok(previous)), Ok(mut event)) => {
                            event.work_was_pending |= previous.work_was_pending;
                            event.rx_pending |= previous.rx_pending;
                            event.raw_status |= previous.raw_status;
                            event.acknowledged |= previous.acknowledged;
                            Ok(event)
                        }
                        (None, event) => event,
                    });
                    Some(binding.owner)
                } else {
                    None
                }
            };
            if let Some(owner) = owner {
                let _ = task::signal_event(owner);
            }
        }
        // No slot or IRQ guard crosses this scheduling boundary.
        task::wait_for_event();
    }
}

/// Teardown is also called by the task exit/termination paths. Service cannot
/// retain a page across unlock, so removing under the same guard is a complete
/// in-flight barrier. The backing frame is dropped only after guard release.
pub fn release_owner(owner: task::TaskId) {
    let retired = {
        let mut state = STATE.lock();
        if !state
            .binding
            .as_ref()
            .is_some_and(|binding| binding.owner == owner)
        {
            return;
        }
        let binding = state.binding.as_mut().unwrap();
        MAILBOX.retire(binding.epoch);
        let _ = binding.controller.disable();
        state.binding.take()
    };
    drop(retired);
}

fn attach_synthetic() -> Option<u64> {
    let page = DmaPage::allocate_zeroed().ok()?;
    let region = MmioRegion::new(page.physical_address(), DMA_PAGE_SIZE).ok()?;
    // SAFETY: the owned page moves into Binding together with the controller;
    // STATE serializes all accesses and teardown drops it after service ends.
    let mmio = unsafe { VolatileMmio32::from_mapped(region, page.virtual_address()) };
    let csr = IntelCsrBank::from_supported(stage13_10i_supported_function()?, mmio).ok()?;
    let mut controller = IntelRxInterruptController::new(csr);
    controller.enable_rx().ok()?;
    let owner = task::current_task_id();
    let mut state = STATE.lock();
    if state.binding.is_some() || state.next_epoch > u64::MAX >> 1 {
        return None;
    }
    let epoch = state.next_epoch;
    state.next_epoch += 1;
    state.binding = Some(Binding {
        owner,
        epoch,
        controller,
        page,
        completion: None,
        faulted: false,
    });
    assert!(MAILBOX.activate(epoch));
    Some(epoch)
}

fn seed(epoch: u64, cause: u32) -> bool {
    let owner = task::current_task_id();
    let mut state = STATE.lock();
    state
        .binding
        .as_mut()
        .filter(|b| b.owner == owner && b.epoch == epoch && !b.faulted)
        .is_some_and(|b| b.page.write_u32(IntelCsr::Int.offset(), cause).is_ok())
}

fn interrupt_producer() -> ! {
    PRODUCER_CPU.store(crate::smp::cpu_index() as u64 + 1, Ordering::Release);
    // SAFETY: initialized IDT vector 0xd0 has an x86-interrupt handler. This
    // software interrupt tests dispatch/wakeup, not hardware MSI delivery.
    unsafe {
        core::arch::asm!("int 0xd0");
    }
    task::exit_current_task()
}

fn exchange(
    epoch: u64,
    cause: u32,
) -> Option<Result<Ax200DeferredServiceEvent, Ax200DeferredServiceError>> {
    PRODUCER_CPU.store(0, Ordering::Release);
    if !seed(epoch, cause) {
        return None;
    }
    // SAFETY: the producer only enters an IRQ handler that uses atomics and
    // scheduler wakeup, then exits. It holds no CPU-local mutable state.
    unsafe {
        task::spawn_on(
            crate::smp::online_count() - 1,
            "wifi-irq-producer",
            interrupt_producer,
        )
    }
    .ok()?;
    let deadline = timer::ticks().saturating_add(500);
    loop {
        let result = {
            let mut state = STATE.lock();
            state
                .binding
                .as_mut()
                .filter(|b| b.epoch == epoch)?
                .completion
                .take()
        };
        if result.is_some() {
            return if PRODUCER_CPU.load(Ordering::Acquire) == crate::smp::online_count() as u64 {
                result
            } else {
                None
            };
        }
        if timer::ticks() >= deadline {
            return None;
        }
        task::wait_for_event_until(deadline);
    }
}

fn consumer() -> ! {
    let ok = (|| {
        let owner = task::current_task_id();
        let epoch = attach_synthetic()?;
        if attach_synthetic().is_some() {
            return None;
        }
        let before = crate::interrupts::wifi_device_irq_count();
        let event = exchange(epoch, INTEL_CSR_INT_BIT_FH_RX)?.ok()?;
        if !event.rx_pending
            || event.acknowledged != INTEL_CSR_INT_BIT_FH_RX
            || crate::interrupts::wifi_device_irq_count() <= before
        {
            return None;
        }
        if exchange(epoch, INTEL_CSR_INT_BIT_SW_ERR)?
            != Err(Ax200DeferredServiceError::Csr(
                IntelRxInterruptError::FatalFirmware,
            ))
        {
            return None;
        }
        if MAILBOX.publish(epoch) || seed(epoch, INTEL_CSR_INT_BIT_FH_RX) {
            return None;
        }
        release_owner(owner);
        let next = attach_synthetic()?;
        if next <= epoch || MAILBOX.publish(epoch) || seed(epoch, 0) {
            return None;
        }
        let event = exchange(next, INTEL_CSR_INT_BIT_SW_RX)?.ok()?;
        if !event.rx_pending || event.acknowledged != INTEL_CSR_INT_BIT_SW_RX {
            return None;
        }
        // Leave queued work and its owned page behind deliberately: ordinary
        // task exit must retire the subscription before reclaiming the frame.
        x86_64::instructions::interrupts::disable();
        crate::interrupts::publish_wifi_device_work_for_test();
        notify_from_irq();
        Some(())
    })()
    .is_some();
    RESULT.store(if ok { 1 } else { 2 }, Ordering::Release);
    task::exit_current_task()
}

pub fn self_test() -> bool {
    // Boot invokes this once after SMP and scheduler initialization.
    let Ok(worker_id) = task::spawn("wifi-rx", worker) else {
        return false;
    };
    WORKER.store(worker_id.as_u64(), Ordering::Release);
    if task::spawn("wifi-rx-check", consumer).is_err() {
        return false;
    }
    let deadline = timer::ticks().saturating_add(500);
    while RESULT.load(Ordering::Acquire) == 0 || STATE.lock().binding.is_some() {
        if timer::ticks() >= deadline {
            return false;
        }
        x86_64::instructions::hlt();
    }
    RESULT.load(Ordering::Acquire) == 1 && MAILBOX.epoch() == 0
}

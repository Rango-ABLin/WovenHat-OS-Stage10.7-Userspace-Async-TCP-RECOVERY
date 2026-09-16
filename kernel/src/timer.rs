use core::{
    arch::asm,
    sync::atomic::{AtomicU64, Ordering},
};

pub const IRQ: u8 = 0;
pub const FREQUENCY_HZ: u32 = 100;

const PIT_INPUT_HZ: u32 = 1_193_182;
const CHANNEL_0_DATA: u16 = 0x40;
const COMMAND: u16 = 0x43;
const CHANNEL_0_RATE_GENERATOR: u8 = 0x34;

static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    let divisor = (PIT_INPUT_HZ / FREQUENCY_HZ) as u16;

    // SAFETY: WovenHat owns the PIT, interrupts are still disabled, and IRQ0
    // remains masked while channel 0 is configured as a rate generator.
    unsafe {
        outb(COMMAND, CHANNEL_0_RATE_GENERATOR);
        outb(CHANNEL_0_DATA, divisor as u8);
        outb(CHANNEL_0_DATA, (divisor >> 8) as u8);
    }
}

pub fn record_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Write one byte to an x86 I/O port.
///
/// # Safety
///
/// The caller must ensure that `port` is valid and that WovenHat owns the
/// associated device.
unsafe fn outb(port: u16, value: u8) {
    // SAFETY: The caller upholds the port-I/O requirements above.
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

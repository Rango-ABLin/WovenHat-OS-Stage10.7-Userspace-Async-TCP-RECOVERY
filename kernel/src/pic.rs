use core::arch::asm;

pub const MASTER_OFFSET: u8 = 32;
pub const SLAVE_OFFSET: u8 = MASTER_OFFSET + 8;

const MASTER_COMMAND: u16 = 0x20;
const MASTER_DATA: u16 = 0x21;
const SLAVE_COMMAND: u16 = 0xa0;
const SLAVE_DATA: u16 = 0xa1;

const INITIALIZE: u8 = 0x11;
const MODE_8086: u8 = 0x01;
const ALL_IRQS_MASKED: u8 = 0xff;
const END_OF_INTERRUPT: u8 = 0x20;

/// Remap both legacy 8259 PICs and leave every hardware IRQ masked.
///
/// This controller is a bootstrap path for QEMU. WovenHat will move to an
/// APIC/x2APIC implementation as the interrupt architecture matures.
pub fn init() {
    // SAFETY: Interrupts remain disabled and the IDT is already loaded. The
    // constants below are the standard 8259 command/data ports, and masking
    // every line prevents delivery before individual IRQ handlers are ready.
    unsafe {
        outb(MASTER_COMMAND, INITIALIZE);
        io_wait();
        outb(SLAVE_COMMAND, INITIALIZE);
        io_wait();

        outb(MASTER_DATA, MASTER_OFFSET);
        io_wait();
        outb(SLAVE_DATA, SLAVE_OFFSET);
        io_wait();

        outb(MASTER_DATA, 1 << 2);
        io_wait();
        outb(SLAVE_DATA, 2);
        io_wait();

        outb(MASTER_DATA, MODE_8086);
        io_wait();
        outb(SLAVE_DATA, MODE_8086);
        io_wait();

        outb(MASTER_DATA, ALL_IRQS_MASKED);
        outb(SLAVE_DATA, ALL_IRQS_MASKED);
    }
}

pub fn unmask(irq: u8) {
    assert!(irq < 16, "PIC IRQ must be in the range 0..16");

    let (data_port, line) = if irq < 8 {
        (MASTER_DATA, irq)
    } else {
        (SLAVE_DATA, irq - 8)
    };

    // SAFETY: The caller installs the corresponding IDT handler before
    // unmasking. PIC data ports contain the IRQ mask and `line` is in 0..8.
    unsafe {
        let mask = inb(data_port) & !(1 << line);
        outb(data_port, mask);

        if irq >= 8 {
            let master_mask = inb(MASTER_DATA) & !(1 << 2);
            outb(MASTER_DATA, master_mask);
        }
    }
}

pub fn notify_end_of_interrupt(irq: u8) {
    // SAFETY: These are the standard PIC command ports. Cascaded IRQs must be
    // acknowledged at the slave first, followed by the master.
    unsafe {
        if irq >= 8 {
            outb(SLAVE_COMMAND, END_OF_INTERRUPT);
        }

        outb(MASTER_COMMAND, END_OF_INTERRUPT);
    }
}

/// Read one byte from an x86 I/O port.
///
/// # Safety
///
/// The caller must ensure that `port` is valid and that WovenHat owns the
/// associated device.
unsafe fn inb(port: u16) -> u8 {
    let value: u8;

    // SAFETY: The caller upholds the port-I/O requirements above.
    unsafe {
        asm!(
            "in al, dx",
            out("al") value,
            in("dx") port,
            options(nomem, nostack, preserves_flags)
        );
    }

    value
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

/// Delay long enough for legacy hardware to observe the previous port write.
unsafe fn io_wait() {
    // SAFETY: Port 0x80 is the conventional POST delay port and the written
    // value has no effect on the PIC or other kernel state.
    unsafe { outb(0x80, 0) };
}

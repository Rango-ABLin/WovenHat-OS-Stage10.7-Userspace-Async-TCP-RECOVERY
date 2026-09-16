use core::arch::asm;
use core::fmt::{self, Write};

const COM1: u16 = 0x3f8;
const LINE_STATUS_TRANSMITTER_EMPTY: u8 = 1 << 5;

pub fn init() {
    // SAFETY: WovenHat owns the machine at this point in kernel startup. These
    // are the standard 16550-compatible COM1 registers exposed by QEMU.
    unsafe {
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x80);
        outb(COM1, 0x03);
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03);
        outb(COM1 + 2, 0xc7);
        outb(COM1 + 4, 0x0b);
    }
}

pub fn write_fmt(arguments: fmt::Arguments<'_>) {
    let _ = SerialPort.write_fmt(arguments);
}

pub fn write_line(arguments: fmt::Arguments<'_>) {
    let _ = SerialPort.write_fmt(arguments);
    let _ = SerialPort.write_str("\n");
}

pub fn write_bytes(bytes: &[u8]) {
    let mut port = SerialPort;
    for &byte in bytes {
        if byte == b'\n' {
            port.write_byte(b'\r');
        }
        port.write_byte(byte);
    }
}
struct SerialPort;

impl Write for SerialPort {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }

            self.write_byte(byte);
        }

        Ok(())
    }
}

impl SerialPort {
    fn write_byte(&mut self, byte: u8) {
        while unsafe { inb(COM1 + 5) } & LINE_STATUS_TRANSMITTER_EMPTY == 0 {
            core::hint::spin_loop();
        }

        // SAFETY: COM1 was initialized during kernel startup, and this writes
        // one byte to its transmit register after it reports readiness.
        unsafe { outb(COM1, byte) };
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

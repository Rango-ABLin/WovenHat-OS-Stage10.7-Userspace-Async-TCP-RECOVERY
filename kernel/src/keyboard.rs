use spin::Mutex;

use core::{
    arch::asm,
    cell::UnsafeCell,
    sync::atomic::{AtomicUsize, Ordering},
};

pub const IRQ: u8 = 1;

const DATA_PORT: u16 = 0x60;
const STATUS_PORT: u16 = 0x64;
const SCANCODE_QUEUE_CAPACITY: usize = 64;

static SCANCODES: ScancodeQueue = ScancodeQueue::new();
static DECODER: Mutex<Keyboard> = Mutex::new(Keyboard::new());

struct ScancodeQueue {
    buffer: UnsafeCell<[u8; SCANCODE_QUEUE_CAPACITY]>,
    read_index: AtomicUsize,
    write_index: AtomicUsize,
}

// SAFETY: This is a single-producer/single-consumer queue. IRQ1 is the only
// producer and the kernel main loop is the only consumer. Release/acquire
// ordering publishes each byte before the consumer observes its index.
unsafe impl Sync for ScancodeQueue {}

impl ScancodeQueue {
    const fn new() -> Self {
        Self {
            buffer: UnsafeCell::new([0; SCANCODE_QUEUE_CAPACITY]),
            read_index: AtomicUsize::new(0),
            write_index: AtomicUsize::new(0),
        }
    }

    fn push(&self, scancode: u8) {
        let write_index = self.write_index.load(Ordering::Relaxed);
        let next_index = (write_index + 1) % SCANCODE_QUEUE_CAPACITY;

        if next_index == self.read_index.load(Ordering::Acquire) {
            return;
        }

        // SAFETY: Only IRQ1 writes queue slots. A slot is not reused until the
        // consumer advances `read_index`, so this write cannot alias a read.
        unsafe { (*self.buffer.get())[write_index] = scancode };
        self.write_index.store(next_index, Ordering::Release);
    }

    fn pop(&self) -> Option<u8> {
        let read_index = self.read_index.load(Ordering::Relaxed);

        if read_index == self.write_index.load(Ordering::Acquire) {
            return None;
        }

        // SAFETY: The producer published this slot with a release store and
        // cannot reuse it until `read_index` is advanced below.
        let scancode = unsafe { (*self.buffer.get())[read_index] };
        self.read_index.store(
            (read_index + 1) % SCANCODE_QUEUE_CAPACITY,
            Ordering::Release,
        );
        Some(scancode)
    }
}

#[derive(Clone, Copy)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Tab,
    F1,
}

pub struct Keyboard {
    shift: bool,
}

impl Keyboard {
    pub const fn new() -> Self {
        Self { shift: false }
    }

    pub fn poll(&mut self) -> Option<Key> {
        if let Some(scancode) = SCANCODES.pop() {
            return self.decode(scancode);
        }

        if !x86_64::instructions::interrupts::are_enabled() {
            return self.poll_legacy();
        }

        None
    }

    /// Temporary fallback for use while CPU interrupts are disabled.
    pub fn poll_legacy(&mut self) -> Option<Key> {
        // PS/2 controller status register:
        // bit 0 == output buffer contains keyboard data.
        let status = unsafe { inb(STATUS_PORT) };

        if status & 1 == 0 {
            return None;
        }

        let scancode = unsafe { inb(DATA_PORT) };

        self.decode(scancode)
    }

    fn decode(&mut self, scancode: u8) -> Option<Key> {
        // PS/2 Set 1: break codes have bit 7 set (make | 0x80).
        // Filter all break codes first; only shift release is meaningful.
        if scancode & 0x80 != 0 {
            return match scancode {
                0xAA | 0xB6 => {
                    self.shift = false;
                    None
                }
                _ => None,
            };
        }

        match scancode {
            // Shift pressed
            0x2A | 0x36 => {
                self.shift = true;
                None
            }

            // Enter
            0x1C => Some(Key::Enter),

            // Backspace
            0x0E => Some(Key::Backspace),

            // Tab
            0x0F => Some(Key::Tab),

            // F1
            0x3B => Some(Key::F1),

            code => decode_scancode(code, self.shift).map(Key::Char),
        }
    }
}

pub fn poll() -> Option<Key> {
    DECODER.lock().poll()
}

pub fn read_bytes(buffer: &mut [u8]) -> usize {
    let mut decoder = DECODER.lock();
    let mut count = 0;
    while count < buffer.len() {
        let Some(key) = decoder.poll() else {
            break;
        };
        let byte = match key {
            Key::Char(character) if character.is_ascii() => character as u8,
            Key::Enter => b'\n',
            Key::Backspace => 8,
            Key::Tab => b'\t',
            Key::F1 | Key::Char(_) => continue,
        };
        buffer[count] = byte;
        count += 1;
    }
    count
}

pub fn inject_validation_input(count: usize) {
    for _ in 0..count {
        SCANCODES.push(0x1e);
    }
}

pub fn self_test() -> bool {
    let mut keyboard = Keyboard::new();
    keyboard
        .decode(0x1e)
        .is_some_and(|key| matches!(key, Key::Char('a')))
        && keyboard.decode(0x2a).is_none()
        && keyboard
            .decode(0x1e)
            .is_some_and(|key| matches!(key, Key::Char('A')))
        && keyboard.decode(0xaa).is_none()
        && keyboard
            .decode(0x1c)
            .is_some_and(|key| matches!(key, Key::Enter))
}
pub fn handle_interrupt() {
    // Confirm that the PS/2 controller really has a fresh keyboard byte before
    // consuming port 0x60. This filters duplicate/spurious IRQ1 delivery and
    // ignores auxiliary-device bytes.
    let status = unsafe { inb(STATUS_PORT) };
    if status & 0x01 == 0 || status & 0x20 != 0 {
        return;
    }

    let scancode = unsafe { inb(DATA_PORT) };
    SCANCODES.push(scancode);
}

/// Read one byte from an x86 I/O port.
///
/// # Safety
///
/// Port I/O is privileged hardware access. This function must only be used
/// after WovenHat OS owns the machine and only with known-valid device ports.
unsafe fn inb(port: u16) -> u8 {
    let value: u8;

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

fn decode_scancode(code: u8, shift: bool) -> Option<char> {
    let normal = match code {
        0x02 => '1',
        0x03 => '2',
        0x04 => '3',
        0x05 => '4',
        0x06 => '5',
        0x07 => '6',
        0x08 => '7',
        0x09 => '8',
        0x0A => '9',
        0x0B => '0',

        0x10 => 'q',
        0x11 => 'w',
        0x12 => 'e',
        0x13 => 'r',
        0x14 => 't',
        0x15 => 'y',
        0x16 => 'u',
        0x17 => 'i',
        0x18 => 'o',
        0x19 => 'p',

        0x1E => 'a',
        0x1F => 's',
        0x20 => 'd',
        0x21 => 'f',
        0x22 => 'g',
        0x23 => 'h',
        0x24 => 'j',
        0x25 => 'k',
        0x26 => 'l',

        0x2C => 'z',
        0x2D => 'x',
        0x2E => 'c',
        0x2F => 'v',
        0x30 => 'b',
        0x31 => 'n',
        0x32 => 'm',

        0x39 => ' ',

        0x0C => '-',
        0x0D => '=',
        0x33 => ',',
        0x34 => '.',
        0x35 => '/',

        _ => return None,
    };

    if !shift {
        return Some(normal);
    }

    Some(match normal {
        'a'..='z' => normal.to_ascii_uppercase(),

        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',

        '-' => '_',
        '=' => '+',
        ',' => '<',
        '.' => '>',
        '/' => '?',

        other => other,
    })
}

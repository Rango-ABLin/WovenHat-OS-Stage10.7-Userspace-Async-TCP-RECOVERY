//! Global framebuffer terminal used by userspace standard streams.
//!
//! The boot/kernel debug console remains a local `Console`, but user processes
//! need a system-wide stdout/stderr sink. This module keeps only raw framebuffer
//! metadata and serializes rendering through a spin mutex. Foreground ownership
//! also prevents the kernel debug shell from stealing PS/2 input while `/bin/sh`
//! is running.

use bootloader_api::info::{FrameBufferInfo, PixelFormat};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::console::glyph;

const BG_R: u8 = 4;
const BG_G: u8 = 12;
const BG_B: u8 = 28;
const FG_R: u8 = 40;
const FG_G: u8 = 220;
const FG_B: u8 = 230;
const NO_FOREGROUND: u64 = 0;

#[derive(Clone, Copy)]
enum Format {
    Rgb,
    Bgr,
    Other,
}

struct State {
    ptr: usize,
    len: usize,
    width: usize,
    height: usize,
    stride: usize,
    bytes_per_pixel: usize,
    format: Format,
    cursor_x: usize,
    cursor_y: usize,
    start_x: usize,
    scale: usize,
    initialized: bool,
}

impl State {
    const fn empty() -> Self {
        Self {
            ptr: 0,
            len: 0,
            width: 0,
            height: 0,
            stride: 0,
            bytes_per_pixel: 0,
            format: Format::Other,
            cursor_x: 40,
            cursor_y: 40,
            start_x: 40,
            scale: 2,
            initialized: false,
        }
    }

    fn init(&mut self, buffer: &mut [u8], info: FrameBufferInfo) {
        self.ptr = buffer.as_mut_ptr() as usize;
        self.len = buffer.len();
        self.width = info.width;
        self.height = info.height;
        self.stride = info.stride;
        self.bytes_per_pixel = info.bytes_per_pixel;
        self.format = match info.pixel_format {
            PixelFormat::Rgb => Format::Rgb,
            PixelFormat::Bgr => Format::Bgr,
            _ => Format::Other,
        };
        self.cursor_x = 40;
        self.cursor_y = 40;
        self.start_x = 40;
        self.scale = 2;
        self.initialized = true;
    }

    fn clear(&mut self) {
        if !self.initialized {
            return;
        }
        for y in 0..self.height {
            for x in 0..self.width {
                self.pixel(x, y, BG_R, BG_G, BG_B);
            }
        }
        self.cursor_x = self.start_x;
        self.cursor_y = 40;
    }

    fn write(&mut self, bytes: &[u8]) {
        if !self.initialized {
            return;
        }
        let mut i = 0usize;
        while i < bytes.len() {
            // Minimal ANSI support used by `/bin/sh`: clear screen + home.
            if bytes[i..].starts_with(b"\x1b[2J") {
                self.clear();
                i += 4;
                continue;
            }
            if bytes[i..].starts_with(b"\x1b[H") {
                self.cursor_x = self.start_x;
                self.cursor_y = 40;
                i += 3;
                continue;
            }
            match bytes[i] {
                b'\n' => self.newline(),
                b'\r' => self.cursor_x = self.start_x,
                8 => self.backspace(),
                b'\t' => {
                    for _ in 0..4 {
                        self.put_char(' ');
                    }
                }
                value if value.is_ascii_graphic() || value == b' ' => self.put_char(value as char),
                _ => {}
            }
            i += 1;
        }
    }

    fn put_char(&mut self, c: char) {
        self.ensure_room();
        let shape = glyph(c);
        for (row, bits) in shape.iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) == 0 {
                    continue;
                }
                for sy in 0..self.scale {
                    for sx in 0..self.scale {
                        self.pixel(
                            self.cursor_x + col * self.scale + sx,
                            self.cursor_y + row * self.scale + sy,
                            FG_R,
                            FG_G,
                            FG_B,
                        );
                    }
                }
            }
        }
        self.cursor_x += 6 * self.scale;
        if self.cursor_x + 6 * self.scale >= self.width {
            self.newline();
        }
    }

    fn newline(&mut self) {
        self.cursor_x = self.start_x;
        self.cursor_y = self.cursor_y.saturating_add(9 * self.scale);
        self.ensure_room();
    }

    fn backspace(&mut self) {
        let char_width = 6 * self.scale;
        if self.cursor_x <= self.start_x {
            return;
        }
        self.cursor_x -= char_width;
        for y in self.cursor_y..core::cmp::min(self.cursor_y + 8 * self.scale, self.height) {
            for x in self.cursor_x..core::cmp::min(self.cursor_x + char_width, self.width) {
                self.pixel(x, y, BG_R, BG_G, BG_B);
            }
        }
    }

    fn ensure_room(&mut self) {
        let line_height = 9 * self.scale;
        if self.cursor_y + 8 * self.scale < self.height {
            return;
        }
        self.scroll(line_height);
        self.cursor_y = self.cursor_y.saturating_sub(line_height);
    }

    fn scroll(&mut self, rows: usize) {
        if rows == 0 || rows >= self.height || self.bytes_per_pixel == 0 {
            self.clear();
            return;
        }
        let row_bytes = self.stride.saturating_mul(self.bytes_per_pixel);
        let move_bytes = (self.height - rows).saturating_mul(row_bytes);
        let src_offset = rows.saturating_mul(row_bytes);
        if src_offset + move_bytes > self.len {
            return;
        }
        unsafe {
            core::ptr::copy(
                (self.ptr as *const u8).add(src_offset),
                self.ptr as *mut u8,
                move_bytes,
            );
        }
        for y in self.height - rows..self.height {
            for x in 0..self.width {
                self.pixel(x, y, BG_R, BG_G, BG_B);
            }
        }
    }

    fn pixel(&mut self, x: usize, y: usize, r: u8, g: u8, b: u8) {
        if x >= self.width || y >= self.height || self.bytes_per_pixel == 0 {
            return;
        }
        let Some(offset) = (y * self.stride + x).checked_mul(self.bytes_per_pixel) else {
            return;
        };
        if offset + self.bytes_per_pixel > self.len {
            return;
        }
        let p = (self.ptr + offset) as *mut u8;
        unsafe {
            match self.format {
                Format::Rgb => {
                    *p = r;
                    if self.bytes_per_pixel > 1 {
                        *p.add(1) = g;
                    }
                    if self.bytes_per_pixel > 2 {
                        *p.add(2) = b;
                    }
                }
                Format::Bgr => {
                    *p = b;
                    if self.bytes_per_pixel > 1 {
                        *p.add(1) = g;
                    }
                    if self.bytes_per_pixel > 2 {
                        *p.add(2) = r;
                    }
                }
                Format::Other => {}
            }
        }
    }
}

static TERMINAL: Mutex<State> = Mutex::new(State::empty());
static FOREGROUND_PID: AtomicU64 = AtomicU64::new(NO_FOREGROUND);

pub fn init(buffer: &mut [u8], info: FrameBufferInfo) {
    TERMINAL.lock().init(buffer, info);
}

pub fn write_bytes(bytes: &[u8]) {
    TERMINAL.lock().write(bytes);
}

pub fn set_cursor_position(x: usize, y: usize) {
    let mut terminal = TERMINAL.lock();
    if !terminal.initialized {
        return;
    }
    terminal.cursor_x = core::cmp::min(x, terminal.width.saturating_sub(1));
    terminal.cursor_y = core::cmp::min(y, terminal.height.saturating_sub(1));
}

pub fn cursor_position() -> (usize, usize) {
    let terminal = TERMINAL.lock();
    (terminal.cursor_x, terminal.cursor_y)
}

#[allow(dead_code)]
pub fn clear() {
    TERMINAL.lock().clear();
}

pub fn set_foreground(pid: u64) {
    FOREGROUND_PID.store(pid, Ordering::Release);
}

pub fn release_foreground(pid: u64) {
    let _ =
        FOREGROUND_PID.compare_exchange(pid, NO_FOREGROUND, Ordering::AcqRel, Ordering::Acquire);
}

pub fn foreground_pid() -> Option<u64> {
    match FOREGROUND_PID.load(Ordering::Acquire) {
        NO_FOREGROUND => None,
        pid => Some(pid),
    }
}

pub fn foreground_active() -> bool {
    foreground_pid().is_some()
}

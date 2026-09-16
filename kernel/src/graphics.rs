use bootloader_api::info::{FrameBufferInfo, PixelFormat};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const BLACK: Self = Self { r: 0, g: 0, b: 0 };
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
    };
    pub const CYAN: Self = Self {
        r: 40,
        g: 220,
        b: 230,
    };
    pub const DARK_BLUE: Self = Self { r: 4, g: 12, b: 28 };
}

pub struct Graphics<'a> {
    buffer: &'a mut [u8],
    info: FrameBufferInfo,
}

impl<'a> Graphics<'a> {
    pub fn new(buffer: &'a mut [u8], info: FrameBufferInfo) -> Self {
        Self { buffer, info }
    }

    pub fn width(&self) -> usize {
        self.info.width
    }

    pub fn height(&self) -> usize {
        self.info.height
    }

    pub fn clear(&mut self, color: Color) {
        self.fill_rect(0, 0, self.width(), self.height(), color);
    }

    pub fn set_pixel(&mut self, x: i32, y: i32, color: Color) {
        let Ok(x) = usize::try_from(x) else {
            return;
        };
        let Ok(y) = usize::try_from(y) else {
            return;
        };
        if x >= self.info.width || y >= self.info.height {
            return;
        }

        let offset = (y * self.info.stride + x) * self.info.bytes_per_pixel;
        if offset + self.info.bytes_per_pixel > self.buffer.len() {
            return;
        }

        match self.info.pixel_format {
            PixelFormat::Rgb => self.write_rgb(offset, color),
            PixelFormat::Bgr => self.write_bgr(offset, color),
            _ => {}
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, width: usize, height: usize, color: Color) {
        let end_x = x.saturating_add(i32::try_from(width).unwrap_or(i32::MAX));
        let end_y = y.saturating_add(i32::try_from(height).unwrap_or(i32::MAX));
        for pixel_y in y..end_y {
            for pixel_x in x..end_x {
                self.set_pixel(pixel_x, pixel_y, color);
            }
        }
    }

    pub fn stroke_rect(&mut self, x: i32, y: i32, width: usize, height: usize, color: Color) {
        if width == 0 || height == 0 {
            return;
        }
        let right = x.saturating_add(width as i32 - 1);
        let bottom = y.saturating_add(height as i32 - 1);
        self.draw_line(x, y, right, y, color);
        self.draw_line(x, bottom, right, bottom, color);
        self.draw_line(x, y, x, bottom, color);
        self.draw_line(right, y, right, bottom, color);
    }

    pub fn draw_line(&mut self, start_x: i32, start_y: i32, end_x: i32, end_y: i32, color: Color) {
        let mut x = start_x;
        let mut y = start_y;
        let delta_x = (end_x - start_x).abs();
        let step_x = if start_x < end_x { 1 } else { -1 };
        let delta_y = -(end_y - start_y).abs();
        let step_y = if start_y < end_y { 1 } else { -1 };
        let mut error = delta_x + delta_y;

        loop {
            self.set_pixel(x, y, color);
            if x == end_x && y == end_y {
                break;
            }
            let double_error = 2 * error;
            if double_error >= delta_y {
                error += delta_y;
                x += step_x;
            }
            if double_error <= delta_x {
                error += delta_x;
                y += step_y;
            }
        }
    }

    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, color: Color) {
        let mut cursor_x = x;
        for character in text.chars() {
            if character == '\n' {
                cursor_x = x;
                continue;
            }
            let glyph = glyph(character);
            for (row, bits) in glyph.iter().enumerate() {
                for column in 0..5 {
                    if bits & (1 << (4 - column)) != 0 {
                        self.set_pixel(cursor_x + column, y + row as i32, color);
                    }
                }
            }
            cursor_x += 6;
        }
    }

    fn write_rgb(&mut self, offset: usize, color: Color) {
        self.buffer[offset] = color.r;
        if self.info.bytes_per_pixel > 1 {
            self.buffer[offset + 1] = color.g;
        }
        if self.info.bytes_per_pixel > 2 {
            self.buffer[offset + 2] = color.b;
        }
    }

    fn write_bgr(&mut self, offset: usize, color: Color) {
        self.buffer[offset] = color.b;
        if self.info.bytes_per_pixel > 1 {
            self.buffer[offset + 1] = color.g;
        }
        if self.info.bytes_per_pixel > 2 {
            self.buffer[offset + 2] = color.r;
        }
    }
}

fn glyph(character: char) -> [u8; 7] {
    match character.to_ascii_uppercase() {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0F, 0x10, 0x10, 0x10, 0x10, 0x10, 0x0F],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x1F],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        ' ' => [0; 7],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        _ => [0x1F, 0x11, 0x02, 0x04, 0x04, 0x00, 0x04],
    }
}

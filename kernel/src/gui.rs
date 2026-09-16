use alloc::vec::Vec;

use crate::graphics::{Color, Graphics};

#[derive(Clone, Copy)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: usize, height: usize) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains(self, x: i32, y: i32) -> bool {
        let Ok(width) = i32::try_from(self.width) else {
            return false;
        };
        let Ok(height) = i32::try_from(self.height) else {
            return false;
        };
        x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(width)
            && y < self.y.saturating_add(height)
    }
}

pub enum InputEvent {
    PointerDown { x: i32, y: i32 },
    Key(char),
}

pub struct Button {
    pub bounds: Rect,
    pub label: &'static str,
    pub color: Color,
    pub pressed: bool,
    pub focused: bool,
}

impl Button {
    pub const fn new(bounds: Rect, label: &'static str, color: Color) -> Self {
        Self {
            bounds,
            label,
            color,
            pressed: false,
            focused: false,
        }
    }

    fn render(&self, graphics: &mut Graphics<'_>) {
        let color = if self.pressed || self.focused {
            Color::WHITE
        } else {
            self.color
        };
        graphics.fill_rect(
            self.bounds.x,
            self.bounds.y,
            self.bounds.width,
            self.bounds.height,
            color,
        );
        graphics.draw_text(
            self.bounds.x + 12,
            self.bounds.y + 18,
            self.label,
            Color::DARK_BLUE,
        );
        if self.focused {
            graphics.stroke_rect(
                self.bounds.x - 2,
                self.bounds.y - 2,
                self.bounds.width + 4,
                self.bounds.height + 4,
                Color::DARK_BLUE,
            );
        }
    }

    fn handle(&mut self, event: &InputEvent) -> bool {
        match event {
            InputEvent::PointerDown { x, y } if self.bounds.contains(*x, *y) => {
                self.pressed = !self.pressed;
                true
            }
            _ => false,
        }
    }
}

pub struct Window {
    pub bounds: Rect,
    pub title: &'static str,
    pub title_bar_height: usize,
    pub title_color: Color,
    pub body_color: Color,
    pub buttons: Vec<Button>,
}

impl Window {
    pub fn new(bounds: Rect, title: &'static str) -> Self {
        Self {
            bounds,
            title,
            title_bar_height: 24,
            title_color: Color::CYAN,
            body_color: Color::WHITE,
            buttons: Vec::new(),
        }
    }

    pub fn add_button(&mut self, button: Button) {
        self.buttons.push(button);
    }

    fn contains(&self, x: i32, y: i32) -> bool {
        self.bounds.contains(x, y)
    }

    fn clear_focus(&mut self) {
        for button in &mut self.buttons {
            button.focused = false;
        }
    }

    fn render(&self, graphics: &mut Graphics<'_>) {
        graphics.fill_rect(
            self.bounds.x,
            self.bounds.y,
            self.bounds.width,
            self.title_bar_height,
            self.title_color,
        );
        graphics.draw_text(
            self.bounds.x + 10,
            self.bounds.y + 8,
            self.title,
            Color::DARK_BLUE,
        );
        graphics.fill_rect(
            self.bounds.x,
            self.bounds.y + self.title_bar_height as i32,
            self.bounds.width,
            self.bounds.height.saturating_sub(self.title_bar_height),
            self.body_color,
        );
        graphics.stroke_rect(
            self.bounds.x,
            self.bounds.y,
            self.bounds.width,
            self.bounds.height,
            Color::BLACK,
        );
        for button in &self.buttons {
            button.render(graphics);
        }
    }

    fn handle(&mut self, event: &InputEvent) -> bool {
        if let InputEvent::Key('\t') = event {
            if self.buttons.is_empty() {
                return false;
            }
            let current = self.buttons.iter().position(|button| button.focused);
            let next = current.map_or(0, |index| (index + 1) % self.buttons.len());
            for (index, button) in self.buttons.iter_mut().enumerate() {
                button.focused = index == next;
            }
            return true;
        }
        if let InputEvent::Key('\n') = event {
            if let Some(button) = self.buttons.iter_mut().find(|button| button.focused) {
                button.pressed = !button.pressed;
                return true;
            }
        }
        if let InputEvent::PointerDown { x, y } = event {
            let mut handled = false;
            for button in &mut self.buttons {
                let hit = button.bounds.contains(*x, *y);
                button.focused = hit;
                if hit {
                    handled = button.handle(event);
                }
            }
            return handled;
        }
        false
    }
}

pub struct Desktop {
    pub background: Color,
    pub windows: Vec<Window>,
}

impl Desktop {
    pub fn new(background: Color) -> Self {
        Self {
            background,
            windows: Vec::new(),
        }
    }

    pub fn add_window(&mut self, window: Window) {
        self.windows.push(window);
    }

    pub fn render(&self, graphics: &mut Graphics<'_>) {
        graphics.clear(self.background);
        for window in &self.windows {
            window.render(graphics);
        }
    }

    pub fn handle(&mut self, event: &InputEvent) -> bool {
        if let InputEvent::PointerDown { x, y } = event {
            let Some(active_index) = self
                .windows
                .iter()
                .rposition(|window| window.contains(*x, *y))
            else {
                for window in &mut self.windows {
                    window.clear_focus();
                }
                return false;
            };

            for (index, window) in self.windows.iter_mut().enumerate() {
                if index != active_index {
                    window.clear_focus();
                }
            }

            let mut active = self.windows.remove(active_index);
            active.handle(event);
            self.windows.push(active);
            return true;
        }

        self.windows
            .last_mut()
            .is_some_and(|window| window.handle(event))
    }
}

pub fn self_test() -> bool {
    let bounds = Rect::new(10, 10, 80, 30);
    let mut desktop = Desktop::new(Color::DARK_BLUE);
    let mut window = Window::new(Rect::new(0, 0, 160, 120), "TEST");
    window.add_button(Button::new(bounds, "ACTIVATE", Color::CYAN));
    window.add_button(Button::new(
        Rect::new(10, 50, 80, 30),
        "SECOND",
        Color::CYAN,
    ));
    desktop.add_window(window);

    let first_focused =
        desktop.handle(&InputEvent::Key('\t')) && desktop.windows[0].buttons[0].focused;
    let second_focused =
        desktop.handle(&InputEvent::Key('\t')) && desktop.windows[0].buttons[1].focused;
    let activated = desktop.handle(&InputEvent::Key('\n'));
    let pointer_activation = desktop.handle(&InputEvent::PointerDown { x: 20, y: 20 });
    first_focused
        && second_focused
        && activated
        && pointer_activation
        && desktop.windows[0].buttons[0].pressed
        && desktop.windows[0].buttons[1].pressed
}

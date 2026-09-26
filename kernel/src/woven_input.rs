// Keyboard input is production; other event producers belong to the Stage 13.7 probe.
use crate::irq_lock::IrqMutex as Mutex;

const INPUT_QUEUE_CAPACITY: usize = 128;

#[cfg(feature = "stage13-7-test")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Keyboard,
    Mouse,
    Touchpad,
    Touchscreen,
    Pen,
    GameController,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Tab,
    F1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Key(Key),
    #[cfg(feature = "stage13-7-test")]
    PointerMove {
        dx: i16,
        dy: i16,
    },
    #[cfg(feature = "stage13-7-test")]
    PointerButton {
        button: u8,
        pressed: bool,
    },
    #[cfg(feature = "stage13-7-test")]
    Scroll {
        vertical: i16,
    },
    #[cfg(feature = "stage13-7-test")]
    Touch {
        contact: u8,
        x: u16,
        y: u16,
        active: bool,
    },
    #[cfg(feature = "stage13-7-test")]
    Pen {
        x: u16,
        y: u16,
        pressure: u16,
        touching: bool,
    },
    #[cfg(feature = "stage13-7-test")]
    GameController {
        control: u16,
        value: i16,
    },
}

#[derive(Clone, Copy)]
struct Queue {
    events: [Option<Event>; INPUT_QUEUE_CAPACITY],
    read: usize,
    write: usize,
    count: usize,
    dropped: u64,
}

impl Queue {
    const fn new() -> Self {
        Self {
            events: [None; INPUT_QUEUE_CAPACITY],
            read: 0,
            write: 0,
            count: 0,
            dropped: 0,
        }
    }

    fn push(&mut self, event: Event) -> bool {
        if self.count == INPUT_QUEUE_CAPACITY {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        self.events[self.write] = Some(event);
        self.write = (self.write + 1) % INPUT_QUEUE_CAPACITY;
        self.count += 1;
        true
    }

    fn pop(&mut self) -> Option<Event> {
        if self.count == 0 {
            return None;
        }
        let event = self.events[self.read].take();
        self.read = (self.read + 1) % INPUT_QUEUE_CAPACITY;
        self.count -= 1;
        event
    }
}

static QUEUE: Mutex<Queue> = Mutex::with_rank(Queue::new(), 10);

pub fn publish(event: Event) -> bool {
    QUEUE.lock().push(event)
}

pub fn publish_key(key: Key) -> bool {
    publish(Event::Key(key))
}

pub fn poll() -> Option<Event> {
    QUEUE.lock().pop()
}

#[cfg(feature = "stage13-7-test")]
pub fn poll_key() -> Option<Key> {
    #[cfg(not(feature = "stage13-7-test"))]
    {
        let Event::Key(key) = poll()?;
        Some(key)
    }
    #[cfg(feature = "stage13-7-test")]
    loop {
        if let Event::Key(key) = poll()? {
            return Some(key);
        }
    }
}

#[cfg(not(feature = "stage13-7-test"))]
pub fn poll_key() -> Option<Key> {
    let Event::Key(key) = poll()?;
    Some(key)
}

pub fn read_bytes(buffer: &mut [u8]) -> usize {
    let mut count = 0;
    while count < buffer.len() {
        let Some(key) = poll_key() else { break };
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

#[cfg(feature = "stage13-7-test")]
pub fn dropped_events() -> u64 {
    QUEUE.lock().dropped
}

#[cfg(feature = "stage13-7-test")]
pub fn self_test() -> bool {
    {
        let mut q = QUEUE.lock();
        *q = Queue::new();
    }
    let ok = publish_key(Key::Char('a'))
        && publish(Event::PointerMove { dx: 3, dy: -2 })
        && poll() == Some(Event::Key(Key::Char('a')))
        && poll() == Some(Event::PointerMove { dx: 3, dy: -2 })
        && poll().is_none();
    {
        let mut q = QUEUE.lock();
        *q = Queue::new();
    }
    ok
}

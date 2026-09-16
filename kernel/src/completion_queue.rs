//! Allocation-free completion-port state; shared by kernel and host tests.

pub const CAPACITY: usize = 32;
pub const BATCH: usize = 8;
pub const CANCELLED: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Event {
    pub operation: u64,
    pub cookie: u64,
    pub class: u32,
    pub flags: u32,
    pub status: i32,
    pub value: u64,
}

impl Event {
    pub fn encode(self, out: &mut [u8]) {
        out[..40].fill(0);
        out[0..8].copy_from_slice(&self.operation.to_le_bytes());
        out[8..16].copy_from_slice(&self.cookie.to_le_bytes());
        out[16..20].copy_from_slice(&self.class.to_le_bytes());
        out[20..24].copy_from_slice(&self.flags.to_le_bytes());
        out[24..28].copy_from_slice(&self.status.to_le_bytes());
        out[32..40].copy_from_slice(&self.value.to_le_bytes());
    }
}

#[derive(Clone, Copy)]
struct Entry {
    event: Event,
    sequence: Option<u64>,
}

pub struct Queue {
    entries: [Option<Entry>; CAPACITY],
    next_sequence: u64,
}

impl Queue {
    pub const fn new() -> Self {
        Self { entries: [None; CAPACITY], next_sequence: 1 }
    }

    /// Reserve before associating: every accepted producer owns one slot.
    pub fn reserve(&mut self, operation: u64, cookie: u64) -> bool {
        if self.entries.iter().flatten().any(|e| e.event.operation == operation) {
            return false;
        }
        let Some(slot) = self.entries.iter_mut().find(|s| s.is_none()) else { return false; };
        *slot = Some(Entry {
            event: Event { operation, cookie, class: 0, flags: 0, status: 0, value: 0 },
            sequence: None,
        });
        true
    }

    pub fn publish(&mut self, operation: u64, class: u32, flags: u32, status: i32, value: u64) -> bool {
        let Some(entry) = self.entries.iter_mut().flatten().find(|e| e.event.operation == operation) else { return false; };
        if entry.sequence.is_some() { return false; }
        // At most CAPACITY live records: renumbering preserves FIFO order and
        // avoids either dropping completions or wrapping ordering at u64::MAX.
        if self.next_sequence == u64::MAX { self.renumber(); }
        let entry = self.entries.iter_mut().flatten().find(|e| e.event.operation == operation).unwrap();
        entry.event.class = class;
        entry.event.flags = flags;
        entry.event.status = status;
        entry.event.value = value;
        entry.sequence = Some(self.next_sequence);
        self.next_sequence += 1;
        true
    }

    fn renumber(&mut self) {
        let mut ordered = [0u64; CAPACITY];
        let mut count = 0;
        for entry in self.entries.iter().flatten() {
            if let Some(seq) = entry.sequence { ordered[count] = seq; count += 1; }
        }
        ordered[..count].sort_unstable();
        for entry in self.entries.iter_mut().flatten() {
            if let Some(seq) = entry.sequence {
                entry.sequence = Some(ordered[..count].binary_search(&seq).unwrap() as u64 + 1);
            }
        }
        self.next_sequence = count as u64 + 1;
    }

    pub fn snapshot(&self, out: &mut [Event]) -> usize {
        let mut after = 0;
        let mut count = 0;
        for target in out {
            let next = self.entries.iter().flatten()
                .filter(|e| e.sequence.is_some_and(|s| s > after))
                .min_by_key(|e| e.sequence);
            let Some(entry) = next else { break; };
            *target = entry.event;
            after = entry.sequence.unwrap();
            count += 1;
        }
        count
    }

    pub fn remove(&mut self, operation: u64) -> bool {
        let Some(slot) = self.entries.iter_mut().find(|s| s.is_some_and(|e| e.event.operation == operation)) else { return false; };
        *slot = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reserve_backpressure_fifo_retry_and_reuse() {
        let mut q = Queue::new();
        for op in 1..=CAPACITY as u64 { assert!(q.reserve(op, op + 100)); }
        assert!(!q.reserve(1000, 0));
        assert!(q.publish(2, 1, 0, 0, 22));
        assert!(q.publish(1, 1, CANCELLED, -2, 0));
        assert!(!q.publish(1, 1, 0, 0, 99));
        let mut out = [Event::default(); BATCH];
        assert_eq!(q.snapshot(&mut out), 2);
        assert_eq!((out[0].operation, out[1].operation), (2, 1));
        assert_eq!(out[1].flags, CANCELLED);
        let mut bytes = [0xff; 40];
        out[1].encode(&mut bytes);
        assert_eq!(&bytes[28..32], &[0; 4]);
        let saved = out;
        assert_eq!(q.snapshot(&mut out), 2); // failed copyout consumes nothing
        assert_eq!(saved, out);
        assert!(q.remove(2));
        assert!(q.reserve(1000, 5));
        assert_eq!(q.snapshot(&mut out), 1);
    }
    #[test]
    fn order_survives_sequence_exhaustion() {
        let mut q = Queue::new();
        q.next_sequence = u64::MAX - 1;
        assert!(q.reserve(9, 0) && q.reserve(8, 0));
        assert!(q.publish(9, 0, 0, 0, 0));
        assert!(q.publish(8, 0, 0, 0, 0));
        let mut out = [Event::default(); 2];
        assert_eq!(q.snapshot(&mut out), 2);
        assert_eq!((out[0].operation, out[1].operation), (9, 8));
    }
}

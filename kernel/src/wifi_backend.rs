//! WovenWiFi Stage 13.9K — transport/backend acceptance foundation.
//!
//! Defines the contract between the protocol/network stack and a Wi-Fi device
//! backend. This stage does not claim support for a real chipset. It proves a
//! bounded virtual backend with TX/RX queues, link state, drop accounting and
//! frame integrity so later PCI/USB Wi-Fi drivers can implement one reviewed
//! transport interface.

pub const MAX_80211_FRAME: usize = 1600;
pub const QUEUE_DEPTH: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendError {
    Down,
    FrameTooLarge,
    TxFull,
    RxFull,
    BufferTooSmall,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackendStats {
    pub up: bool,
    pub tx_frames: u64,
    pub rx_frames: u64,
    pub tx_dropped: u64,
    pub rx_dropped: u64,
}

#[derive(Clone, Copy)]
struct Slot {
    len: usize,
    data: [u8; MAX_80211_FRAME],
}
impl Slot {
    const fn empty() -> Self { Self { len: 0, data: [0; MAX_80211_FRAME] } }
}

pub trait WifiBackend {
    fn set_up(&mut self, up: bool);
    fn is_up(&self) -> bool;
    fn transmit(&mut self, frame: &[u8]) -> Result<(), BackendError>;
    fn receive_into(&mut self, out: &mut [u8]) -> Result<Option<usize>, BackendError>;
    fn stats(&self) -> BackendStats;
}

/// Deterministic in-kernel backend used to validate the transport contract.
/// A later chipset driver can implement `WifiBackend` without changing RSN,
/// CCMP, or WovenNet integration.
pub struct VirtualBackend {
    up: bool,
    tx: [Slot; QUEUE_DEPTH],
    tx_head: usize,
    tx_tail: usize,
    tx_count: usize,
    rx: [Slot; QUEUE_DEPTH],
    rx_head: usize,
    rx_tail: usize,
    rx_count: usize,
    tx_frames: u64,
    rx_frames: u64,
    tx_dropped: u64,
    rx_dropped: u64,
}
impl VirtualBackend {
    pub const fn new() -> Self {
        Self {
            up: false,
            tx: [Slot::empty(); QUEUE_DEPTH],
            tx_head: 0, tx_tail: 0, tx_count: 0,
            rx: [Slot::empty(); QUEUE_DEPTH],
            rx_head: 0, rx_tail: 0, rx_count: 0,
            tx_frames: 0, rx_frames: 0, tx_dropped: 0, rx_dropped: 0,
        }
    }

    pub fn dequeue_tx(&mut self, out: &mut [u8]) -> Result<Option<usize>, BackendError> {
        if self.tx_count == 0 { return Ok(None); }
        let slot = &self.tx[self.tx_head];
        if out.len() < slot.len { return Err(BackendError::BufferTooSmall); }
        out[..slot.len].copy_from_slice(&slot.data[..slot.len]);
        let len = slot.len;
        self.tx[self.tx_head].len = 0;
        self.tx_head = (self.tx_head + 1) % QUEUE_DEPTH;
        self.tx_count -= 1;
        Ok(Some(len))
    }

    /// Inject a received 802.11 frame as if delivered by a chipset/firmware.
    pub fn inject_rx(&mut self, frame: &[u8]) -> Result<(), BackendError> {
        if !self.up { return Err(BackendError::Down); }
        if frame.len() > MAX_80211_FRAME {
            self.rx_dropped = self.rx_dropped.saturating_add(1);
            return Err(BackendError::FrameTooLarge);
        }
        if self.rx_count == QUEUE_DEPTH {
            self.rx_dropped = self.rx_dropped.saturating_add(1);
            return Err(BackendError::RxFull);
        }
        let slot = &mut self.rx[self.rx_tail];
        slot.data[..frame.len()].copy_from_slice(frame);
        slot.len = frame.len();
        self.rx_tail = (self.rx_tail + 1) % QUEUE_DEPTH;
        self.rx_count += 1;
        Ok(())
    }
}

impl WifiBackend for VirtualBackend {
    fn set_up(&mut self, up: bool) {
        self.up = up;
        if !up {
            self.tx_head = 0; self.tx_tail = 0; self.tx_count = 0;
            self.rx_head = 0; self.rx_tail = 0; self.rx_count = 0;
            for slot in &mut self.tx { slot.len = 0; }
            for slot in &mut self.rx { slot.len = 0; }
        }
    }
    fn is_up(&self) -> bool { self.up }

    fn transmit(&mut self, frame: &[u8]) -> Result<(), BackendError> {
        if !self.up { return Err(BackendError::Down); }
        if frame.len() > MAX_80211_FRAME {
            self.tx_dropped = self.tx_dropped.saturating_add(1);
            return Err(BackendError::FrameTooLarge);
        }
        if self.tx_count == QUEUE_DEPTH {
            self.tx_dropped = self.tx_dropped.saturating_add(1);
            return Err(BackendError::TxFull);
        }
        let slot = &mut self.tx[self.tx_tail];
        slot.data[..frame.len()].copy_from_slice(frame);
        slot.len = frame.len();
        self.tx_tail = (self.tx_tail + 1) % QUEUE_DEPTH;
        self.tx_count += 1;
        self.tx_frames = self.tx_frames.saturating_add(1);
        Ok(())
    }

    fn receive_into(&mut self, out: &mut [u8]) -> Result<Option<usize>, BackendError> {
        if !self.up { return Err(BackendError::Down); }
        if self.rx_count == 0 { return Ok(None); }
        let slot = &self.rx[self.rx_head];
        if out.len() < slot.len { return Err(BackendError::BufferTooSmall); }
        out[..slot.len].copy_from_slice(&slot.data[..slot.len]);
        let len = slot.len;
        self.rx[self.rx_head].len = 0;
        self.rx_head = (self.rx_head + 1) % QUEUE_DEPTH;
        self.rx_count -= 1;
        self.rx_frames = self.rx_frames.saturating_add(1);
        Ok(Some(len))
    }

    fn stats(&self) -> BackendStats {
        BackendStats {
            up: self.up,
            tx_frames: self.tx_frames,
            rx_frames: self.rx_frames,
            tx_dropped: self.tx_dropped,
            rx_dropped: self.rx_dropped,
        }
    }
}

pub fn self_test() -> bool {
    let mut backend = VirtualBackend::new();
    let frame = [0x5au8; 128];

    if backend.transmit(&frame) != Err(BackendError::Down) { return false; }
    backend.set_up(true);
    if !backend.is_up() { return false; }

    if backend.transmit(&frame).is_err() { return false; }
    let mut out = [0u8; MAX_80211_FRAME];
    let Ok(Some(tx_len)) = backend.dequeue_tx(&mut out) else { return false; };
    if tx_len != frame.len() || out[..tx_len] != frame { return false; }

    if backend.inject_rx(&frame).is_err() { return false; }
    out.fill(0);
    let Ok(Some(rx_len)) = backend.receive_into(&mut out) else { return false; };
    if rx_len != frame.len() || out[..rx_len] != frame { return false; }

    for _ in 0..QUEUE_DEPTH {
        if backend.transmit(&frame).is_err() { return false; }
    }
    if backend.transmit(&frame) != Err(BackendError::TxFull) { return false; }

    let stats = backend.stats();
    if !stats.up || stats.tx_frames != (QUEUE_DEPTH as u64 + 1)
        || stats.rx_frames != 1 || stats.tx_dropped != 1 || stats.rx_dropped != 0
    {
        return false;
    }

    backend.set_up(false);
    if backend.is_up() || backend.dequeue_tx(&mut out).ok().flatten().is_some() {
        return false;
    }
    true
}

//! WovenWiFi Stage 13.9A foundation.
//!
//! This layer deliberately separates Wi-Fi policy/state from chipset-specific
//! drivers. Stage 13.9A proves bounded scan state, association transitions,
//! security metadata, and PCI wireless-controller classification. It does not
//! claim that a physical 802.11 chipset can transmit frames yet.

use crate::hal::pci;

pub const MAX_SCAN_RESULTS: usize = 16;
pub const MAX_SSID_LEN: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadioState {
    Down,
    Idle,
    Scanning,
    Associating,
    Associated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Security {
    Open,
    Wpa2Personal,
    Wpa3Personal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccessPoint {
    pub ssid: [u8; MAX_SSID_LEN],
    pub ssid_len: u8,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub signal_dbm: i8,
    pub security: Security,
}

impl AccessPoint {
    pub const fn empty() -> Self {
        Self {
            ssid: [0; MAX_SSID_LEN],
            ssid_len: 0,
            bssid: [0; 6],
            channel: 0,
            signal_dbm: -127,
            security: Security::Open,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateError {
    RadioDown,
    Busy,
    NotScanning,
    NoCandidate,
}

pub struct Manager {
    state: RadioState,
    scan: [Option<AccessPoint>; MAX_SCAN_RESULTS],
    scan_count: usize,
    associated: Option<AccessPoint>,
}

impl Manager {
    pub const fn new() -> Self {
        Self {
            state: RadioState::Down,
            scan: [None; MAX_SCAN_RESULTS],
            scan_count: 0,
            associated: None,
        }
    }

    pub fn state(&self) -> RadioState {
        self.state
    }
    pub fn scan_count(&self) -> usize {
        self.scan_count
    }
    pub fn associated(&self) -> Option<AccessPoint> {
        self.associated
    }

    pub fn power_on(&mut self) {
        if self.state == RadioState::Down {
            self.state = RadioState::Idle;
        }
    }

    pub fn power_off(&mut self) {
        self.state = RadioState::Down;
        self.scan = [None; MAX_SCAN_RESULTS];
        self.scan_count = 0;
        self.associated = None;
    }

    pub fn begin_scan(&mut self) -> Result<(), StateError> {
        match self.state {
            RadioState::Down => Err(StateError::RadioDown),
            RadioState::Idle | RadioState::Associated => {
                self.scan = [None; MAX_SCAN_RESULTS];
                self.scan_count = 0;
                self.state = RadioState::Scanning;
                Ok(())
            }
            RadioState::Scanning | RadioState::Associating => Err(StateError::Busy),
        }
    }

    pub fn record_scan_result(&mut self, ap: AccessPoint) -> Result<(), StateError> {
        if self.state != RadioState::Scanning {
            return Err(StateError::NotScanning);
        }
        if ap.ssid_len as usize > MAX_SSID_LEN {
            return Err(StateError::NoCandidate);
        }

        if let Some(existing) = self.scan[..self.scan_count]
            .iter_mut()
            .flatten()
            .find(|existing| existing.bssid == ap.bssid)
        {
            *existing = ap;
            return Ok(());
        }

        if self.scan_count < MAX_SCAN_RESULTS {
            self.scan[self.scan_count] = Some(ap);
            self.scan_count += 1;
        }
        Ok(())
    }

    pub fn finish_scan(&mut self) -> Result<(), StateError> {
        if self.state != RadioState::Scanning {
            return Err(StateError::NotScanning);
        }
        self.state = RadioState::Idle;
        Ok(())
    }

    pub fn begin_association(&mut self, bssid: [u8; 6]) -> Result<AccessPoint, StateError> {
        if self.state == RadioState::Down {
            return Err(StateError::RadioDown);
        }
        if self.state != RadioState::Idle && self.state != RadioState::Associated {
            return Err(StateError::Busy);
        }
        let candidate = self.scan[..self.scan_count]
            .iter()
            .flatten()
            .copied()
            .find(|ap| ap.bssid == bssid)
            .ok_or(StateError::NoCandidate)?;
        self.state = RadioState::Associating;
        Ok(candidate)
    }

    pub fn association_complete(&mut self, ap: AccessPoint, success: bool) {
        if self.state != RadioState::Associating {
            return;
        }
        if success {
            self.associated = Some(ap);
            self.state = RadioState::Associated;
        } else {
            self.associated = None;
            self.state = RadioState::Idle;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PciDiscovery {
    pub network_controllers: u16,
    pub wireless_candidates: u16,
}

/// PCI base class 0x02 is Network Controller. Subclass 0x80 is the generic
/// "other network controller" bucket commonly used by PCI Wi-Fi adapters.
/// Chipset-specific matching is intentionally deferred to later Stage 13.9 work.
pub const fn is_wireless_class(class: u8, subclass: u8) -> bool {
    class == 0x02 && subclass == 0x80
}

pub fn discover_pci() -> PciDiscovery {
    let mut summary = PciDiscovery::default();
    for index in 0..64 {
        let Some(device) = pci::device(index) else {
            continue;
        };
        if device.class == 0x02 {
            summary.network_controllers = summary.network_controllers.saturating_add(1);
            if is_wireless_class(device.class, device.subclass) {
                summary.wireless_candidates = summary.wireless_candidates.saturating_add(1);
            }
        }
    }
    summary
}

pub fn self_test() -> bool {
    let mut manager = Manager::new();
    if manager.begin_scan() != Err(StateError::RadioDown) {
        return false;
    }
    manager.power_on();
    if manager.state() != RadioState::Idle || manager.begin_scan().is_err() {
        return false;
    }

    let mut ssid = [0u8; MAX_SSID_LEN];
    ssid[..8].copy_from_slice(b"WovenLab");
    let ap = AccessPoint {
        ssid,
        ssid_len: 8,
        bssid: [0x02, 0x57, 0x48, 0x13, 0x09, 0x01],
        channel: 6,
        signal_dbm: -42,
        security: Security::Wpa3Personal,
    };
    if manager.record_scan_result(ap).is_err()
        || manager.scan_count() != 1
        || manager.finish_scan().is_err()
    {
        return false;
    }
    let Ok(candidate) = manager.begin_association(ap.bssid) else {
        return false;
    };
    manager.association_complete(candidate, true);

    is_wireless_class(0x02, 0x80)
        && !is_wireless_class(0x02, 0x00)
        && !is_wireless_class(0x03, 0x80)
        && manager.state() == RadioState::Associated
        && manager.associated() == Some(ap)
}

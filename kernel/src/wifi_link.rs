//! Stage 13.9Q: WPA2 association + RSN integration.
//!
//! Open System authentication remains the 802.11 authentication algorithm for
//! WPA2-Personal. WPA2 is negotiated by carrying a validated RSN IE in the
//! association request. Association rejection, authentication rejection, and
//! timeout all roll Manager state back to Idle.

use crate::wifi::{AccessPoint, Manager, Security, StateError};
use crate::wifi80211::{self, ManagementSubtype, ParseError, MGMT_HEADER_LEN};
use crate::wifi_rsn::{self, Akm, Cipher, RsnProfile};

const AUTH_LEN: usize = 6;
const ASSOC_RESP_LEN: usize = 6;
const CAPABILITY_ESS: u16 = 1 << 0;
const CAPABILITY_PRIVACY: u16 = 1 << 4;
const LISTEN_INTERVAL: u16 = 10;
const RSN_BODY_LEN: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Idle,
    AuthenticationPending,
    AssociationPending,
    Associated,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkError {
    Manager(StateError),
    Protocol(ParseError),
    UnsupportedSecurity,
    WrongState,
    WrongPeer,
    WrongSubtype,
    Truncated,
    AuthenticationRejected(u16),
    AssociationRejected(u16),
    InvalidAuthentication,
    BufferTooSmall,
}
impl From<StateError> for LinkError {
    fn from(v: StateError) -> Self {
        Self::Manager(v)
    }
}
impl From<ParseError> for LinkError {
    fn from(v: ParseError) -> Self {
        Self::Protocol(v)
    }
}

pub struct LinkMachine {
    state: LinkState,
    station: [u8; 6],
    candidate: Option<AccessPoint>,
    aid: Option<u16>,
}
impl LinkMachine {
    pub const fn new(station: [u8; 6]) -> Self {
        Self {
            state: LinkState::Idle,
            station,
            candidate: None,
            aid: None,
        }
    }
    pub const fn state(&self) -> LinkState {
        self.state
    }
    pub const fn association_id(&self) -> Option<u16> {
        self.aid
    }

    pub fn begin(
        &mut self,
        manager: &mut Manager,
        bssid: [u8; 6],
        out: &mut [u8],
    ) -> Result<usize, LinkError> {
        if self.state != LinkState::Idle {
            return Err(LinkError::WrongState);
        }
        let ap = manager.begin_association(bssid)?;
        if !matches!(ap.security, Security::Open | Security::Wpa2Personal) {
            manager.association_complete(ap, false);
            return Err(LinkError::UnsupportedSecurity);
        }
        match build_auth_request(out, self.station, ap.bssid) {
            Ok(len) => {
                self.candidate = Some(ap);
                self.state = LinkState::AuthenticationPending;
                Ok(len)
            }
            Err(e) => {
                manager.association_complete(ap, false);
                Err(e)
            }
        }
    }

    pub fn authentication_response(
        &mut self,
        manager: &mut Manager,
        frame: &[u8],
        out: &mut [u8],
    ) -> Result<usize, LinkError> {
        if self.state != LinkState::AuthenticationPending {
            return Err(LinkError::WrongState);
        }
        let ap = self.candidate.ok_or(LinkError::WrongState)?;
        let h = wifi80211::parse_management_header(frame)?;
        if h.control.management_subtype() != Some(ManagementSubtype::Authentication) {
            return Err(LinkError::WrongSubtype);
        }
        if h.receiver != self.station || h.transmitter != ap.bssid || h.bssid != ap.bssid {
            return Err(LinkError::WrongPeer);
        }
        let b = frame
            .get(MGMT_HEADER_LEN..MGMT_HEADER_LEN + AUTH_LEN)
            .ok_or(LinkError::Truncated)?;
        let algorithm = u16::from_le_bytes([b[0], b[1]]);
        let sequence = u16::from_le_bytes([b[2], b[3]]);
        let status = u16::from_le_bytes([b[4], b[5]]);
        if algorithm != 0 || sequence != 2 {
            self.fail(manager);
            return Err(LinkError::InvalidAuthentication);
        }
        if status != 0 {
            self.fail(manager);
            return Err(LinkError::AuthenticationRejected(status));
        }
        match build_assoc_request(out, self.station, ap) {
            Ok(len) => {
                self.state = LinkState::AssociationPending;
                Ok(len)
            }
            Err(e) => {
                self.fail(manager);
                Err(e)
            }
        }
    }

    pub fn association_response(
        &mut self,
        manager: &mut Manager,
        frame: &[u8],
    ) -> Result<(), LinkError> {
        if self.state != LinkState::AssociationPending {
            return Err(LinkError::WrongState);
        }
        let ap = self.candidate.ok_or(LinkError::WrongState)?;
        let h = wifi80211::parse_management_header(frame)?;
        if h.control.management_subtype() != Some(ManagementSubtype::AssociationResponse) {
            return Err(LinkError::WrongSubtype);
        }
        if h.receiver != self.station || h.transmitter != ap.bssid || h.bssid != ap.bssid {
            return Err(LinkError::WrongPeer);
        }
        let b = frame
            .get(MGMT_HEADER_LEN..MGMT_HEADER_LEN + ASSOC_RESP_LEN)
            .ok_or(LinkError::Truncated)?;
        let status = u16::from_le_bytes([b[2], b[3]]);
        if status != 0 {
            self.fail(manager);
            return Err(LinkError::AssociationRejected(status));
        }
        self.aid = Some(u16::from_le_bytes([b[4], b[5]]) & 0x3fff);
        self.state = LinkState::Associated;
        manager.association_complete(ap, true);
        Ok(())
    }

    pub fn timeout(&mut self, manager: &mut Manager) -> Result<(), LinkError> {
        if !matches!(
            self.state,
            LinkState::AuthenticationPending | LinkState::AssociationPending
        ) {
            return Err(LinkError::WrongState);
        }
        self.fail(manager);
        Ok(())
    }

    fn fail(&mut self, manager: &mut Manager) {
        if let Some(ap) = self.candidate {
            manager.association_complete(ap, false)
        }
        self.aid = None;
        self.candidate = None;
        self.state = LinkState::Failed;
    }
}

fn build_auth_request(
    out: &mut [u8],
    station: [u8; 6],
    bssid: [u8; 6],
) -> Result<usize, LinkError> {
    let total = MGMT_HEADER_LEN + AUTH_LEN;
    if out.len() < total {
        return Err(LinkError::BufferTooSmall);
    }
    wifi80211::build_management_header(out, 11, bssid, station, bssid, 0)?;
    out[MGMT_HEADER_LEN..total].fill(0);
    out[MGMT_HEADER_LEN + 2..MGMT_HEADER_LEN + 4].copy_from_slice(&1u16.to_le_bytes());
    Ok(total)
}

fn wpa2_psk_rsn_body() -> [u8; RSN_BODY_LEN] {
    [
        1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
    ]
}

fn build_assoc_request(
    out: &mut [u8],
    station: [u8; 6],
    ap: AccessPoint,
) -> Result<usize, LinkError> {
    let n = usize::from(ap.ssid_len);
    let rsn_len = if ap.security == Security::Wpa2Personal {
        2 + RSN_BODY_LEN
    } else {
        0
    };
    let total = MGMT_HEADER_LEN + 4 + 2 + n + 4 + rsn_len;
    if out.len() < total {
        return Err(LinkError::BufferTooSmall);
    }
    wifi80211::build_management_header(out, 0, ap.bssid, station, ap.bssid, 0)?;
    let mut p = MGMT_HEADER_LEN;
    let mut capability = CAPABILITY_ESS;
    if ap.security == Security::Wpa2Personal {
        capability |= CAPABILITY_PRIVACY
    }
    out[p..p + 2].copy_from_slice(&capability.to_le_bytes());
    p += 2;
    out[p..p + 2].copy_from_slice(&LISTEN_INTERVAL.to_le_bytes());
    p += 2;
    out[p] = wifi80211::IE_SSID;
    out[p + 1] = ap.ssid_len;
    p += 2;
    out[p..p + n].copy_from_slice(&ap.ssid[..n]);
    p += n;
    out[p] = wifi80211::IE_SUPPORTED_RATES;
    out[p + 1] = 2;
    out[p + 2] = 0x82;
    out[p + 3] = 0x84;
    p += 4;
    if ap.security == Security::Wpa2Personal {
        let rsn = wpa2_psk_rsn_body();
        out[p] = wifi80211::IE_RSN;
        out[p + 1] = RSN_BODY_LEN as u8;
        p += 2;
        out[p..p + RSN_BODY_LEN].copy_from_slice(&rsn);
        p += RSN_BODY_LEN;
    }
    Ok(p)
}

fn response(
    out: &mut [u8],
    subtype: u8,
    station: [u8; 6],
    bssid: [u8; 6],
    body: &[u8],
) -> Option<usize> {
    let total = MGMT_HEADER_LEN + body.len();
    if out.len() < total {
        return None;
    }
    wifi80211::build_management_header(out, subtype, station, bssid, bssid, 0).ok()?;
    out[MGMT_HEADER_LEN..total].copy_from_slice(body);
    Some(total)
}

pub fn self_test() -> bool {
    let station = [0x02, 0x57, 0x48, 0x13, 0x09, 0x44];
    let bssid = [0x02, 0x57, 0x48, 0x13, 0x09, 0x55];
    let mut ssid = [0u8; crate::wifi::MAX_SSID_LEN];
    ssid[..9].copy_from_slice(b"WovenWPA2");
    let ap = AccessPoint {
        ssid,
        ssid_len: 9,
        bssid,
        channel: 6,
        signal_dbm: -35,
        security: Security::Wpa2Personal,
    };
    let mut manager = Manager::new();
    manager.power_on();
    if manager.begin_scan().is_err()
        || manager.record_scan_result(ap).is_err()
        || manager.finish_scan().is_err()
    {
        return false;
    }
    let mut link = LinkMachine::new(station);
    let mut tx = [0u8; 160];
    if link.begin(&mut manager, bssid, &mut tx).is_err()
        || link.state() != LinkState::AuthenticationPending
    {
        return false;
    }
    let mut rx = [0u8; 128];
    let Some(n) = response(&mut rx, 11, station, bssid, &[0, 0, 2, 0, 0, 0]) else {
        return false;
    };
    let Ok(assoc_len) = link.authentication_response(&mut manager, &rx[..n], &mut tx) else {
        return false;
    };
    if link.state() != LinkState::AssociationPending {
        return false;
    }
    let body = &tx[MGMT_HEADER_LEN..assoc_len];
    let capability = u16::from_le_bytes([body[0], body[1]]);
    if capability & (CAPABILITY_ESS | CAPABILITY_PRIVACY) != (CAPABILITY_ESS | CAPABILITY_PRIVACY) {
        return false;
    }
    let ies = &body[4..];
    let Ok(elements) = wifi80211::parse_elements(ies) else {
        return false;
    };
    if !elements.rsn_present {
        return false;
    }
    let mut p = 0usize;
    let mut parsed: Option<RsnProfile> = None;
    while p + 2 <= ies.len() {
        let id = ies[p];
        let len = usize::from(ies[p + 1]);
        p += 2;

        if p + len > ies.len() {
            return false;
        }

        if id == wifi80211::IE_RSN {
            parsed = wifi_rsn::parse_rsn(&ies[p..p + len]).ok();
            break;
        }

        p += len;
    }
    let Some(profile) = parsed else { return false };
    if profile.akm != Akm::Psk
        || profile.pairwise_cipher != Cipher::Ccmp128
        || profile.group_cipher != Cipher::Ccmp128
    {
        return false;
    }
    let Some(n) = response(&mut rx, 1, station, bssid, &[0, 0, 0, 0, 0x2a, 0xc0]) else {
        return false;
    };
    if link.association_response(&mut manager, &rx[..n]).is_err()
        || link.association_id() != Some(42)
        || manager.associated() != Some(ap)
    {
        return false;
    }

    // Authentication rejection must roll Manager out of Associating.
    let mut m2 = Manager::new();
    m2.power_on();
    if m2.begin_scan().is_err() || m2.record_scan_result(ap).is_err() || m2.finish_scan().is_err() {
        return false;
    }
    let mut l2 = LinkMachine::new(station);
    if l2.begin(&mut m2, bssid, &mut tx).is_err() {
        return false;
    }
    let Some(n) = response(&mut rx, 11, station, bssid, &[0, 0, 2, 0, 13, 0]) else {
        return false;
    };
    if l2.authentication_response(&mut m2, &rx[..n], &mut tx)
        != Err(LinkError::AuthenticationRejected(13))
        || m2.state() != crate::wifi::RadioState::Idle
    {
        return false;
    }

    // WPA3 remains rejected at this WPA2-only milestone, also with rollback.
    let mut wpa3 = ap;
    wpa3.security = Security::Wpa3Personal;
    wpa3.bssid[5] ^= 1;
    let mut m3 = Manager::new();
    m3.power_on();
    if m3.begin_scan().is_err() || m3.record_scan_result(wpa3).is_err() || m3.finish_scan().is_err()
    {
        return false;
    }
    let mut l3 = LinkMachine::new(station);
    l3.begin(&mut m3, wpa3.bssid, &mut tx) == Err(LinkError::UnsupportedSecurity)
        && m3.state() == crate::wifi::RadioState::Idle
}

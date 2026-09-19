//! Stage 13.9D: hardware-neutral IEEE 802.11 Open System authentication/association.
use crate::wifi::{AccessPoint, Manager, Security, StateError};
use crate::wifi80211::{self, ManagementSubtype, ParseError, MGMT_HEADER_LEN};

const AUTH_LEN: usize = 6;
const ASSOC_RESP_LEN: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState { Idle, AuthenticationPending, AssociationPending, Associated, Failed }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkError {
    Manager(StateError), Protocol(ParseError), UnsupportedSecurity, WrongState,
    WrongPeer, WrongSubtype, Truncated, AuthenticationRejected(u16),
    AssociationRejected(u16), InvalidAuthentication, BufferTooSmall,
}
impl From<StateError> for LinkError { fn from(v: StateError) -> Self { Self::Manager(v) } }
impl From<ParseError> for LinkError { fn from(v: ParseError) -> Self { Self::Protocol(v) } }

pub struct LinkMachine {
    state: LinkState,
    station: [u8; 6],
    candidate: Option<AccessPoint>,
    aid: Option<u16>,
}
impl LinkMachine {
    pub const fn new(station: [u8; 6]) -> Self {
        Self { state: LinkState::Idle, station, candidate: None, aid: None }
    }
    pub const fn state(&self) -> LinkState { self.state }
    pub const fn association_id(&self) -> Option<u16> { self.aid }

    pub fn begin(&mut self, manager: &mut Manager, bssid: [u8; 6], out: &mut [u8])
        -> Result<usize, LinkError>
    {
        if self.state != LinkState::Idle { return Err(LinkError::WrongState); }
        let ap = manager.begin_association(bssid)?;
        if ap.security != Security::Open {
            manager.association_complete(ap, false);
            return Err(LinkError::UnsupportedSecurity);
        }
        let len = build_auth_request(out, self.station, ap.bssid)?;
        self.candidate = Some(ap);
        self.state = LinkState::AuthenticationPending;
        Ok(len)
    }

    pub fn authentication_response(&mut self, frame: &[u8], out: &mut [u8])
        -> Result<usize, LinkError>
    {
        if self.state != LinkState::AuthenticationPending { return Err(LinkError::WrongState); }
        let ap = self.candidate.ok_or(LinkError::WrongState)?;
        let h = wifi80211::parse_management_header(frame)?;
        if h.control.management_subtype() != Some(ManagementSubtype::Authentication) {
            return Err(LinkError::WrongSubtype);
        }
        if h.receiver != self.station || h.transmitter != ap.bssid || h.bssid != ap.bssid {
            return Err(LinkError::WrongPeer);
        }
        let b = frame.get(MGMT_HEADER_LEN..MGMT_HEADER_LEN + AUTH_LEN).ok_or(LinkError::Truncated)?;
        let algorithm = u16::from_le_bytes([b[0], b[1]]);
        let sequence = u16::from_le_bytes([b[2], b[3]]);
        let status = u16::from_le_bytes([b[4], b[5]]);
        if algorithm != 0 || sequence != 2 {
            self.state = LinkState::Failed;
            return Err(LinkError::InvalidAuthentication);
        }
        if status != 0 {
            self.state = LinkState::Failed;
            return Err(LinkError::AuthenticationRejected(status));
        }
        let len = build_assoc_request(out, self.station, ap)?;
        self.state = LinkState::AssociationPending;
        Ok(len)
    }

    pub fn association_response(&mut self, manager: &mut Manager, frame: &[u8])
        -> Result<(), LinkError>
    {
        if self.state != LinkState::AssociationPending { return Err(LinkError::WrongState); }
        let ap = self.candidate.ok_or(LinkError::WrongState)?;
        let h = wifi80211::parse_management_header(frame)?;
        if h.control.management_subtype() != Some(ManagementSubtype::AssociationResponse) {
            return Err(LinkError::WrongSubtype);
        }
        if h.receiver != self.station || h.transmitter != ap.bssid || h.bssid != ap.bssid {
            return Err(LinkError::WrongPeer);
        }
        let b = frame.get(MGMT_HEADER_LEN..MGMT_HEADER_LEN + ASSOC_RESP_LEN).ok_or(LinkError::Truncated)?;
        let status = u16::from_le_bytes([b[2], b[3]]);
        if status != 0 {
            self.state = LinkState::Failed;
            manager.association_complete(ap, false);
            return Err(LinkError::AssociationRejected(status));
        }
        self.aid = Some(u16::from_le_bytes([b[4], b[5]]) & 0x3fff);
        self.state = LinkState::Associated;
        manager.association_complete(ap, true);
        Ok(())
    }

    pub fn timeout(&mut self, manager: &mut Manager) -> Result<(), LinkError> {
        if !matches!(self.state, LinkState::AuthenticationPending | LinkState::AssociationPending) {
            return Err(LinkError::WrongState);
        }
        if let Some(ap) = self.candidate { manager.association_complete(ap, false); }
        self.aid = None;
        self.state = LinkState::Failed;
        Ok(())
    }
}

fn build_auth_request(out: &mut [u8], station: [u8; 6], bssid: [u8; 6])
    -> Result<usize, LinkError>
{
    let total = MGMT_HEADER_LEN + AUTH_LEN;
    if out.len() < total { return Err(LinkError::BufferTooSmall); }
    wifi80211::build_management_header(out, 11, bssid, station, bssid, 0)?;
    out[MGMT_HEADER_LEN..total].fill(0);
    out[MGMT_HEADER_LEN + 2..MGMT_HEADER_LEN + 4].copy_from_slice(&1u16.to_le_bytes());
    Ok(total)
}

fn build_assoc_request(out: &mut [u8], station: [u8; 6], ap: AccessPoint)
    -> Result<usize, LinkError>
{
    let n = usize::from(ap.ssid_len);
    let total = MGMT_HEADER_LEN + 4 + 2 + n + 4;
    if out.len() < total { return Err(LinkError::BufferTooSmall); }
    wifi80211::build_management_header(out, 0, ap.bssid, station, ap.bssid, 0)?;
    let mut p = MGMT_HEADER_LEN;
    out[p..p+2].copy_from_slice(&0u16.to_le_bytes()); p += 2;
    out[p..p+2].copy_from_slice(&10u16.to_le_bytes()); p += 2;
    out[p] = wifi80211::IE_SSID; out[p+1] = ap.ssid_len; p += 2;
    out[p..p+n].copy_from_slice(&ap.ssid[..n]); p += n;
    out[p] = wifi80211::IE_SUPPORTED_RATES; out[p+1] = 2; out[p+2] = 0x82; out[p+3] = 0x84;
    Ok(p + 4)
}

fn response(out: &mut [u8], subtype: u8, station: [u8; 6], bssid: [u8; 6], body: &[u8])
    -> Option<usize>
{
    let total = MGMT_HEADER_LEN + body.len();
    if out.len() < total { return None; }
    wifi80211::build_management_header(out, subtype, station, bssid, bssid, 0).ok()?;
    out[MGMT_HEADER_LEN..total].copy_from_slice(body);
    Some(total)
}

pub fn self_test() -> bool {
    let station = [0x02,0x57,0x48,0x13,0x09,0x44];
    let bssid = [0x02,0x57,0x48,0x13,0x09,0x55];
    let mut ssid = [0u8; crate::wifi::MAX_SSID_LEN];
    ssid[..9].copy_from_slice(b"WovenOpen");
    let ap = AccessPoint { ssid, ssid_len: 9, bssid, channel: 6, signal_dbm: -35, security: Security::Open };

    let mut manager = Manager::new();
    manager.power_on();
    if manager.begin_scan().is_err() || manager.record_scan_result(ap).is_err() || manager.finish_scan().is_err() { return false; }

    let mut link = LinkMachine::new(station);
    let mut tx = [0u8; 128];
    if link.begin(&mut manager, bssid, &mut tx).is_err() || link.state() != LinkState::AuthenticationPending { return false; }

    let mut rx = [0u8; 128];
    let Some(n) = response(&mut rx, 11, station, bssid, &[0,0,2,0,0,0]) else { return false; };
    if link.authentication_response(&rx[..n], &mut tx).is_err() || link.state() != LinkState::AssociationPending { return false; }

    let Some(n) = response(&mut rx, 1, station, bssid, &[0,0,0,0,0x2a,0xc0]) else { return false; };
    if link.association_response(&mut manager, &rx[..n]).is_err()
        || link.state() != LinkState::Associated || link.association_id() != Some(42)
        || manager.associated() != Some(ap) { return false; }

    let mut manager2 = Manager::new();
    manager2.power_on();
    if manager2.begin_scan().is_err() || manager2.record_scan_result(ap).is_err() || manager2.finish_scan().is_err() { return false; }
    let mut link2 = LinkMachine::new(station);
    link2.begin(&mut manager2, bssid, &mut tx).is_ok()
        && link2.timeout(&mut manager2).is_ok()
        && link2.state() == LinkState::Failed
        && manager2.associated().is_none()
}

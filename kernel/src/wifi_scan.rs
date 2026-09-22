//! WovenWiFi Stage 13.9C — beacon/probe scanning pipeline.
//!
//! Converts validated IEEE 802.11 beacon and probe-response frames into the
//! bounded WovenWiFi scan table. This remains hardware-neutral: frames are
//! supplied by a backend in later stages.

use crate::wifi::{AccessPoint, Manager, Security};
use crate::wifi80211::{self, ManagementSubtype, ParseError, MGMT_HEADER_LEN};

const FIXED_BEACON_FIELDS: usize = 12;
const CAPABILITY_PRIVACY: u16 = 1 << 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanParseError {
    Protocol(ParseError),
    WrongSubtype,
    TruncatedFixedFields,
    MissingChannel,
    InvalidChannel,
    ManagerRejected,
}

impl From<ParseError> for ScanParseError {
    fn from(value: ParseError) -> Self {
        Self::Protocol(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanObservation {
    pub ap: AccessPoint,
    pub beacon_interval_tu: u16,
    pub capability_info: u16,
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Result<u16, ScanParseError> {
    let chunk = bytes
        .get(offset..offset + 2)
        .ok_or(ScanParseError::TruncatedFixedFields)?;
    Ok(u16::from_le_bytes([chunk[0], chunk[1]]))
}

fn classify_security(capability_info: u16, rsn_present: bool) -> Security {
    if rsn_present || capability_info & CAPABILITY_PRIVACY != 0 {
        Security::Wpa2Personal
    } else {
        Security::Open
    }
}

pub fn parse_scan_frame(frame: &[u8], signal_dbm: i8) -> Result<ScanObservation, ScanParseError> {
    let header = wifi80211::parse_management_header(frame)?;
    match header.control.management_subtype() {
        Some(ManagementSubtype::Beacon | ManagementSubtype::ProbeResponse) => {}
        _ => return Err(ScanParseError::WrongSubtype),
    }

    let fixed_start = MGMT_HEADER_LEN;
    let fixed_end = fixed_start + FIXED_BEACON_FIELDS;
    if frame.len() < fixed_end {
        return Err(ScanParseError::TruncatedFixedFields);
    }

    let beacon_interval_tu = read_u16_le(frame, fixed_start + 8)?;
    let capability_info = read_u16_le(frame, fixed_start + 10)?;
    let elements = wifi80211::parse_elements(&frame[fixed_end..])?;
    let channel = elements.channel.ok_or(ScanParseError::MissingChannel)?;
    if channel == 0 {
        return Err(ScanParseError::InvalidChannel);
    }

    let mut ssid = [0u8; crate::wifi::MAX_SSID_LEN];
    let ssid_len = usize::from(elements.ssid_len);
    ssid[..ssid_len].copy_from_slice(&elements.ssid[..ssid_len]);

    Ok(ScanObservation {
        ap: AccessPoint {
            ssid,
            ssid_len: elements.ssid_len,
            bssid: header.bssid,
            channel,
            signal_dbm,
            security: classify_security(capability_info, elements.rsn_present),
        },
        beacon_interval_tu,
        capability_info,
    })
}

pub fn record_frame(
    manager: &mut Manager,
    frame: &[u8],
    signal_dbm: i8,
) -> Result<ScanObservation, ScanParseError> {
    let observation = parse_scan_frame(frame, signal_dbm)?;
    manager
        .record_scan_result(observation.ap)
        .map_err(|_| ScanParseError::ManagerRejected)?;
    Ok(observation)
}

fn build_scan_frame(
    output: &mut [u8],
    subtype: u8,
    bssid: [u8; 6],
    ssid: &[u8],
    channel: u8,
    rsn: bool,
) -> Option<usize> {
    let fixed = MGMT_HEADER_LEN + FIXED_BEACON_FIELDS;
    let ie_len = 2 + ssid.len() + 3 + if rsn { 4 } else { 0 };
    let total = fixed + ie_len;
    if ssid.len() > crate::wifi::MAX_SSID_LEN || output.len() < total {
        return None;
    }

    wifi80211::build_management_header(output, subtype, [0xff; 6], bssid, bssid, 0x0010).ok()?;

    output[MGMT_HEADER_LEN..fixed].fill(0);
    output[MGMT_HEADER_LEN + 8..MGMT_HEADER_LEN + 10].copy_from_slice(&100u16.to_le_bytes());
    let capability = if rsn { CAPABILITY_PRIVACY } else { 0 };
    output[MGMT_HEADER_LEN + 10..MGMT_HEADER_LEN + 12].copy_from_slice(&capability.to_le_bytes());

    let mut p = fixed;
    output[p] = wifi80211::IE_SSID;
    output[p + 1] = ssid.len() as u8;
    p += 2;
    output[p..p + ssid.len()].copy_from_slice(ssid);
    p += ssid.len();

    output[p] = wifi80211::IE_DS_PARAMETER_SET;
    output[p + 1] = 1;
    output[p + 2] = channel;
    p += 3;

    if rsn {
        output[p] = wifi80211::IE_RSN;
        output[p + 1] = 2;
        output[p + 2] = 1;
        output[p + 3] = 0;
        p += 4;
    }

    Some(p)
}

pub fn self_test() -> bool {
    let bssid = [0x02, 0x57, 0x48, 0x13, 0x09, 0x03];
    let mut frame = [0u8; 128];
    let Some(len) = build_scan_frame(&mut frame, 8, bssid, b"WovenScan", 11, true) else {
        return false;
    };

    let Ok(obs) = parse_scan_frame(&frame[..len], -38) else {
        return false;
    };
    if obs.ap.bssid != bssid
        || obs.ap.ssid_len != 9
        || &obs.ap.ssid[..9] != b"WovenScan"
        || obs.ap.channel != 11
        || obs.ap.signal_dbm != -38
        || obs.ap.security != Security::Wpa2Personal
        || obs.beacon_interval_tu != 100
    {
        return false;
    }

    let mut manager = Manager::new();
    manager.power_on();
    if manager.begin_scan().is_err() {
        return false;
    }
    if record_frame(&mut manager, &frame[..len], -38).is_err() || manager.scan_count() != 1 {
        return false;
    }

    let Some(len2) = build_scan_frame(&mut frame, 5, bssid, b"WovenScan", 6, true) else {
        return false;
    };
    if record_frame(&mut manager, &frame[..len2], -25).is_err() || manager.scan_count() != 1 {
        return false;
    }

    let mut bad = [0u8; 64];
    let Some(bad_len) = build_scan_frame(&mut bad, 8, bssid, b"Bad", 0, false) else {
        return false;
    };

    manager.finish_scan().is_ok()
        && parse_scan_frame(&bad[..bad_len], -70) == Err(ScanParseError::InvalidChannel)
        && parse_scan_frame(&frame[..MGMT_HEADER_LEN], -70)
            == Err(ScanParseError::TruncatedFixedFields)
}

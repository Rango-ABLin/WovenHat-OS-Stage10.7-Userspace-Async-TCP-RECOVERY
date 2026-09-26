//! WovenWiFi Stage 13.9B — IEEE 802.11 core parsing/building.
//!
//! This module is intentionally hardware-neutral. It provides bounded parsing
//! primitives for management/data frames and information elements so later
//! scan/authentication/association logic can share one audited protocol core.
//!
//! Stage 13.9B does NOT implement RF transmission, WPA handshakes, firmware
//! loading, or a physical Wi-Fi chipset driver.

pub const MAC_LEN: usize = 6;
pub const MGMT_HEADER_LEN: usize = 24;
pub const DATA_HEADER_LEN: usize = 24;
pub const MAX_INFORMATION_ELEMENTS: usize = 32;
pub const MAX_SSID_LEN: usize = 32;

pub const IE_SSID: u8 = 0;
pub const IE_SUPPORTED_RATES: u8 = 1;
pub const IE_DS_PARAMETER_SET: u8 = 3;
pub const IE_EXTENDED_SUPPORTED_RATES: u8 = 50;
pub const IE_RSN: u8 = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameType {
    Management,
    Control,
    Data,
    Extension,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagementSubtype {
    AssociationRequest,
    AssociationResponse,
    ReassociationRequest,
    ReassociationResponse,
    ProbeRequest,
    ProbeResponse,
    Beacon,
    Atim,
    Disassociation,
    Authentication,
    Deauthentication,
    Action,
    Unknown(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameControl {
    raw: u16,
}

impl FrameControl {
    pub const fn from_raw(raw: u16) -> Self {
        Self { raw }
    }

    pub const fn raw(self) -> u16 {
        self.raw
    }

    pub const fn protocol_version(self) -> u8 {
        (self.raw & 0x0003) as u8
    }

    pub const fn frame_type(self) -> FrameType {
        match (self.raw >> 2) & 0x3 {
            0 => FrameType::Management,
            1 => FrameType::Control,
            2 => FrameType::Data,
            _ => FrameType::Extension,
        }
    }

    pub const fn subtype(self) -> u8 {
        ((self.raw >> 4) & 0x0f) as u8
    }

    pub const fn to_ds(self) -> bool {
        self.raw & (1 << 8) != 0
    }

    pub const fn is_from_ds(self) -> bool {
        self.raw & (1 << 9) != 0
    }

    pub const fn more_fragments(self) -> bool {
        self.raw & (1 << 10) != 0
    }

    pub const fn retry(self) -> bool {
        self.raw & (1 << 11) != 0
    }

    pub const fn protected(self) -> bool {
        self.raw & (1 << 14) != 0
    }

    pub const fn management_subtype(self) -> Option<ManagementSubtype> {
        if !matches!(self.frame_type(), FrameType::Management) {
            return None;
        }
        Some(match self.subtype() {
            0 => ManagementSubtype::AssociationRequest,
            1 => ManagementSubtype::AssociationResponse,
            2 => ManagementSubtype::ReassociationRequest,
            3 => ManagementSubtype::ReassociationResponse,
            4 => ManagementSubtype::ProbeRequest,
            5 => ManagementSubtype::ProbeResponse,
            8 => ManagementSubtype::Beacon,
            9 => ManagementSubtype::Atim,
            10 => ManagementSubtype::Disassociation,
            11 => ManagementSubtype::Authentication,
            12 => ManagementSubtype::Deauthentication,
            13 => ManagementSubtype::Action,
            other => ManagementSubtype::Unknown(other),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ManagementHeader {
    pub control: FrameControl,
    pub duration: u16,
    pub receiver: [u8; MAC_LEN],
    pub transmitter: [u8; MAC_LEN],
    pub bssid: [u8; MAC_LEN],
    pub sequence_control: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataHeader {
    pub control: FrameControl,
    pub duration: u16,
    pub address1: [u8; MAC_LEN],
    pub address2: [u8; MAC_LEN],
    pub address3: [u8; MAC_LEN],
    pub sequence_control: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    Truncated,
    InvalidProtocolVersion,
    WrongFrameType,
    TooManyInformationElements,
    InvalidSsid,
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Result<u16, ParseError> {
    let chunk = bytes.get(offset..offset + 2).ok_or(ParseError::Truncated)?;
    Ok(u16::from_le_bytes([chunk[0], chunk[1]]))
}

fn read_mac(bytes: &[u8], offset: usize) -> Result<[u8; MAC_LEN], ParseError> {
    let chunk = bytes
        .get(offset..offset + MAC_LEN)
        .ok_or(ParseError::Truncated)?;
    let mut mac = [0u8; MAC_LEN];
    mac.copy_from_slice(chunk);
    Ok(mac)
}

pub fn parse_management_header(bytes: &[u8]) -> Result<ManagementHeader, ParseError> {
    if bytes.len() < MGMT_HEADER_LEN {
        return Err(ParseError::Truncated);
    }
    let control = FrameControl::from_raw(read_u16_le(bytes, 0)?);
    if control.protocol_version() != 0 {
        return Err(ParseError::InvalidProtocolVersion);
    }
    if control.frame_type() != FrameType::Management {
        return Err(ParseError::WrongFrameType);
    }
    Ok(ManagementHeader {
        control,
        duration: read_u16_le(bytes, 2)?,
        receiver: read_mac(bytes, 4)?,
        transmitter: read_mac(bytes, 10)?,
        bssid: read_mac(bytes, 16)?,
        sequence_control: read_u16_le(bytes, 22)?,
    })
}

pub fn parse_data_header(bytes: &[u8]) -> Result<DataHeader, ParseError> {
    if bytes.len() < DATA_HEADER_LEN {
        return Err(ParseError::Truncated);
    }
    let control = FrameControl::from_raw(read_u16_le(bytes, 0)?);
    if control.protocol_version() != 0 {
        return Err(ParseError::InvalidProtocolVersion);
    }
    if control.frame_type() != FrameType::Data {
        return Err(ParseError::WrongFrameType);
    }
    Ok(DataHeader {
        control,
        duration: read_u16_le(bytes, 2)?,
        address1: read_mac(bytes, 4)?,
        address2: read_mac(bytes, 10)?,
        address3: read_mac(bytes, 16)?,
        sequence_control: read_u16_le(bytes, 22)?,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InformationElement<'a> {
    pub id: u8,
    pub body: &'a [u8],
}

pub struct InformationElements<'a> {
    bytes: &'a [u8],
    offset: usize,
    count: usize,
    malformed: bool,
}

impl<'a> InformationElements<'a> {
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            count: 0,
            malformed: false,
        }
    }

    pub const fn malformed(&self) -> bool {
        self.malformed
    }
}

impl<'a> Iterator for InformationElements<'a> {
    type Item = InformationElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.malformed || self.offset == self.bytes.len() {
            return None;
        }
        if self.count >= MAX_INFORMATION_ELEMENTS {
            self.malformed = true;
            return None;
        }
        let header = match self.bytes.get(self.offset..self.offset + 2) {
            Some(header) => header,
            None => {
                self.malformed = true;
                return None;
            }
        };
        let id = header[0];
        let len = header[1] as usize;
        let body_start = self.offset + 2;
        let body_end = match body_start.checked_add(len) {
            Some(end) => end,
            None => {
                self.malformed = true;
                return None;
            }
        };
        let body = match self.bytes.get(body_start..body_end) {
            Some(body) => body,
            None => {
                self.malformed = true;
                return None;
            }
        };
        self.offset = body_end;
        self.count += 1;
        Some(InformationElement { id, body })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParsedElements {
    pub ssid: [u8; MAX_SSID_LEN],
    pub ssid_len: u8,
    pub channel: Option<u8>,
    pub rsn_present: bool,
    pub supported_rate_elements: u8,
}

impl ParsedElements {
    pub const fn empty() -> Self {
        Self {
            ssid: [0; MAX_SSID_LEN],
            ssid_len: 0,
            channel: None,
            rsn_present: false,
            supported_rate_elements: 0,
        }
    }
}

pub fn parse_elements(bytes: &[u8]) -> Result<ParsedElements, ParseError> {
    let mut parsed = ParsedElements::empty();
    let mut elements = InformationElements::new(bytes);

    for element in elements.by_ref() {
        match element.id {
            IE_SSID => {
                if element.body.len() > MAX_SSID_LEN {
                    return Err(ParseError::InvalidSsid);
                }
                parsed.ssid = [0; MAX_SSID_LEN];
                parsed.ssid[..element.body.len()].copy_from_slice(element.body);
                parsed.ssid_len = element.body.len() as u8;
            }
            IE_DS_PARAMETER_SET if element.body.len() == 1 => {
                parsed.channel = Some(element.body[0]);
            }
            IE_RSN => {
                parsed.rsn_present = true;
            }
            IE_SUPPORTED_RATES | IE_EXTENDED_SUPPORTED_RATES => {
                parsed.supported_rate_elements = parsed.supported_rate_elements.saturating_add(1);
            }
            _ => {}
        }
    }

    if elements.malformed() {
        return Err(ParseError::TooManyInformationElements);
    }
    Ok(parsed)
}

/// Build a minimal management header. Body/FCS generation is handled by
/// higher-level protocol or chipset layers.
pub fn build_management_header(
    output: &mut [u8],
    subtype: u8,
    receiver: [u8; MAC_LEN],
    transmitter: [u8; MAC_LEN],
    bssid: [u8; MAC_LEN],
    sequence_control: u16,
) -> Result<usize, ParseError> {
    if output.len() < MGMT_HEADER_LEN {
        return Err(ParseError::Truncated);
    }
    let frame_control = u16::from(subtype & 0x0f) << 4;
    output[..MGMT_HEADER_LEN].fill(0);
    output[0..2].copy_from_slice(&frame_control.to_le_bytes());
    output[4..10].copy_from_slice(&receiver);
    output[10..16].copy_from_slice(&transmitter);
    output[16..22].copy_from_slice(&bssid);
    output[22..24].copy_from_slice(&sequence_control.to_le_bytes());
    Ok(MGMT_HEADER_LEN)
}

pub fn self_test() -> bool {
    let receiver = [0xff; 6];
    let transmitter = [0x02, 0x57, 0x48, 0x13, 0x09, 0x02];
    let bssid = [0x02, 0x57, 0x48, 0x13, 0x09, 0x01];

    let mut header = [0u8; MGMT_HEADER_LEN];
    if build_management_header(&mut header, 8, receiver, transmitter, bssid, 0x1230)
        != Ok(MGMT_HEADER_LEN)
    {
        return false;
    }

    let Ok(parsed_header) = parse_management_header(&header) else {
        return false;
    };
    if parsed_header.control.management_subtype() != Some(ManagementSubtype::Beacon)
        || parsed_header.receiver != receiver
        || parsed_header.transmitter != transmitter
        || parsed_header.bssid != bssid
        || parsed_header.sequence_control != 0x1230
    {
        return false;
    }

    let ies = [
        IE_SSID,
        8,
        b'W',
        b'o',
        b'v',
        b'e',
        b'n',
        b'L',
        b'a',
        b'b',
        IE_SUPPORTED_RATES,
        2,
        0x82,
        0x84,
        IE_DS_PARAMETER_SET,
        1,
        6,
        IE_RSN,
        2,
        1,
        0,
    ];
    let Ok(parsed_ies) = parse_elements(&ies) else {
        return false;
    };

    let mut data_header = [0u8; DATA_HEADER_LEN];
    data_header[0] = 0x08;
    data_header[1] = 0x01;
    if parse_data_header(&data_header)
        .map(|h| h.control.frame_type() == FrameType::Data && h.control.to_ds())
        != Ok(true)
    {
        return false;
    }

    let malformed = [IE_SSID, 4, b'a', b'b'];
    let invalid_version = [0x01u8, 0x00];

    parsed_ies.ssid_len == 8
        && &parsed_ies.ssid[..8] == b"WovenLab"
        && parsed_ies.channel == Some(6)
        && parsed_ies.rsn_present
        && parsed_ies.supported_rate_elements == 1
        && parse_elements(&malformed) == Err(ParseError::TooManyInformationElements)
        && parse_management_header(&invalid_version) == Err(ParseError::Truncated)
}

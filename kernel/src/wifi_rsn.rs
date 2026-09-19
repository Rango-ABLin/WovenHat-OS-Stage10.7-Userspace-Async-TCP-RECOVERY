//! WovenWiFi Stage 13.9E — WPA2/RSN protocol foundation.
//!
//! This stage parses RSN information elements and EAPOL-Key message metadata,
//! and tracks a bounded WPA2 4-way-handshake state machine. Cryptographic
//! primitives are intentionally represented as verification/derivation
//! boundaries and are not implemented here. A later security-reviewed stage
//! must supply PBKDF2/PRF/HMAC/AES key-wrap/CCMP primitives.

pub const RSN_OUI: [u8; 3] = [0x00, 0x0f, 0xac];
pub const RSN_VERSION: u16 = 1;
pub const CIPHER_CCMP_128: u8 = 4;
pub const AKM_PSK: u8 = 2;
pub const AKM_SAE: u8 = 8;

pub const EAPOL_TYPE_KEY: u8 = 3;
pub const EAPOL_KEY_DESCRIPTOR_RSN: u8 = 2;
pub const EAPOL_KEY_FIXED_LEN: usize = 95;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RsnError {
    Truncated,
    UnsupportedVersion,
    InvalidSuite,
    UnsupportedCipher,
    UnsupportedAkm,
    UnsupportedPairwiseCount,
    UnsupportedAkmCount,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cipher {
    Ccmp128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Akm {
    Psk,
    Sae,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RsnProfile {
    pub group_cipher: Cipher,
    pub pairwise_cipher: Cipher,
    pub akm: Akm,
    pub capabilities: u16,
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Result<u16, RsnError> {
    let b = bytes.get(offset..offset + 2).ok_or(RsnError::Truncated)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn parse_suite(bytes: &[u8], offset: usize) -> Result<([u8; 3], u8), RsnError> {
    let s = bytes.get(offset..offset + 4).ok_or(RsnError::Truncated)?;
    Ok(([s[0], s[1], s[2]], s[3]))
}

fn parse_cipher(bytes: &[u8], offset: usize) -> Result<Cipher, RsnError> {
    let (oui, suite) = parse_suite(bytes, offset)?;
    if oui != RSN_OUI { return Err(RsnError::InvalidSuite); }
    match suite {
        CIPHER_CCMP_128 => Ok(Cipher::Ccmp128),
        _ => Err(RsnError::UnsupportedCipher),
    }
}

fn parse_akm(bytes: &[u8], offset: usize) -> Result<Akm, RsnError> {
    let (oui, suite) = parse_suite(bytes, offset)?;
    if oui != RSN_OUI { return Err(RsnError::InvalidSuite); }
    match suite {
        AKM_PSK => Ok(Akm::Psk),
        AKM_SAE => Ok(Akm::Sae),
        _ => Err(RsnError::UnsupportedAkm),
    }
}

pub fn parse_rsn(body: &[u8]) -> Result<RsnProfile, RsnError> {
    if read_u16_le(body, 0)? != RSN_VERSION {
        return Err(RsnError::UnsupportedVersion);
    }
    let group_cipher = parse_cipher(body, 2)?;
    let pairwise_count = read_u16_le(body, 6)?;
    if pairwise_count != 1 { return Err(RsnError::UnsupportedPairwiseCount); }
    let pairwise_cipher = parse_cipher(body, 8)?;
    let akm_count = read_u16_le(body, 12)?;
    if akm_count != 1 { return Err(RsnError::UnsupportedAkmCount); }
    let akm = parse_akm(body, 14)?;
    let capabilities = if body.len() >= 20 { read_u16_le(body, 18)? } else { 0 };
    Ok(RsnProfile { group_cipher, pairwise_cipher, akm, capabilities })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EapolError {
    Truncated,
    WrongPacketType,
    WrongDescriptorType,
    InvalidBodyLength,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyInfo(pub u16);

impl KeyInfo {
    pub const fn descriptor_version(self) -> u8 { (self.0 & 0x0007) as u8 }
    pub const fn key_type_pairwise(self) -> bool { self.0 & (1 << 3) != 0 }
    pub const fn install(self) -> bool { self.0 & (1 << 6) != 0 }
    pub const fn ack(self) -> bool { self.0 & (1 << 7) != 0 }
    pub const fn mic(self) -> bool { self.0 & (1 << 8) != 0 }
    pub const fn secure(self) -> bool { self.0 & (1 << 9) != 0 }
    pub const fn encrypted_key_data(self) -> bool { self.0 & (1 << 12) != 0 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EapolKey<'a> {
    pub protocol_version: u8,
    pub descriptor_type: u8,
    pub key_info: KeyInfo,
    pub key_length: u16,
    pub replay_counter: u64,
    pub nonce: [u8; 32],
    pub mic: [u8; 16],
    pub key_data: &'a [u8],
}

fn read_u16_be(bytes: &[u8], offset: usize) -> Result<u16, EapolError> {
    let b = bytes.get(offset..offset + 2).ok_or(EapolError::Truncated)?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}
fn read_u64_be(bytes: &[u8], offset: usize) -> Result<u64, EapolError> {
    let b = bytes.get(offset..offset + 8).ok_or(EapolError::Truncated)?;
    Ok(u64::from_be_bytes([b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7]]))
}

pub fn parse_eapol_key(frame: &[u8]) -> Result<EapolKey<'_>, EapolError> {
    if frame.len() < 4 + EAPOL_KEY_FIXED_LEN { return Err(EapolError::Truncated); }
    if frame[1] != EAPOL_TYPE_KEY { return Err(EapolError::WrongPacketType); }
    let body_len = read_u16_be(frame, 2)? as usize;
    if body_len < EAPOL_KEY_FIXED_LEN || frame.len() < 4 + body_len {
        return Err(EapolError::InvalidBodyLength);
    }
    let b = &frame[4..4 + body_len];
    if b[0] != EAPOL_KEY_DESCRIPTOR_RSN { return Err(EapolError::WrongDescriptorType); }
    let key_info = KeyInfo(read_u16_be(b, 1)?);
    let key_length = read_u16_be(b, 3)?;
    let replay_counter = read_u64_be(b, 5)?;
    let mut nonce = [0u8; 32];
    nonce.copy_from_slice(&b[13..45]);
    let mut mic = [0u8; 16];
    mic.copy_from_slice(&b[77..93]);
    let key_data_len = read_u16_be(b, 93)? as usize;
    let key_data_end = 95usize.checked_add(key_data_len).ok_or(EapolError::InvalidBodyLength)?;
    let key_data = b.get(95..key_data_end).ok_or(EapolError::InvalidBodyLength)?;
    Ok(EapolKey {
        protocol_version: frame[0],
        descriptor_type: b[0],
        key_info,
        key_length,
        replay_counter,
        nonce,
        mic,
        key_data,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FourWayState {
    Idle,
    AwaitingMessage1,
    AwaitingMessage3,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FourWayError {
    WrongState,
    UnsupportedProfile,
    ReplayCounter,
    InvalidMessage,
    MicRequired,
    CryptoNotVerified,
}

pub struct FourWayHandshake {
    state: FourWayState,
    last_replay: u64,
    anonce: [u8; 32],
}

impl FourWayHandshake {
    pub const fn new() -> Self {
        Self { state: FourWayState::Idle, last_replay: 0, anonce: [0; 32] }
    }
    pub const fn state(&self) -> FourWayState { self.state }
    pub const fn anonce(&self) -> [u8; 32] { self.anonce }

    pub fn begin(&mut self, profile: RsnProfile) -> Result<(), FourWayError> {
        if self.state != FourWayState::Idle { return Err(FourWayError::WrongState); }
        if profile.akm != Akm::Psk
            || profile.group_cipher != Cipher::Ccmp128
            || profile.pairwise_cipher != Cipher::Ccmp128
        {
            return Err(FourWayError::UnsupportedProfile);
        }
        self.last_replay = 0;
        self.anonce = [0; 32];
        self.state = FourWayState::AwaitingMessage1;
        Ok(())
    }

    pub fn receive_message1(&mut self, key: EapolKey<'_>) -> Result<(), FourWayError> {
        if self.state != FourWayState::AwaitingMessage1 { return Err(FourWayError::WrongState); }
        if !key.key_info.key_type_pairwise() || !key.key_info.ack()
            || key.key_info.mic() || key.key_info.install()
        {
            self.state = FourWayState::Failed;
            return Err(FourWayError::InvalidMessage);
        }
        if key.replay_counter <= self.last_replay {
            self.state = FourWayState::Failed;
            return Err(FourWayError::ReplayCounter);
        }
        self.last_replay = key.replay_counter;
        self.anonce = key.nonce;
        self.state = FourWayState::AwaitingMessage3;
        Ok(())
    }

    pub fn receive_message3_metadata(&mut self, key: EapolKey<'_>) -> Result<(), FourWayError> {
        if self.state != FourWayState::AwaitingMessage3 { return Err(FourWayError::WrongState); }
        if !key.key_info.key_type_pairwise() || !key.key_info.ack()
            || !key.key_info.mic() || !key.key_info.install() || !key.key_info.secure()
        {
            self.state = FourWayState::Failed;
            return Err(FourWayError::InvalidMessage);
        }
        if key.replay_counter < self.last_replay {
            self.state = FourWayState::Failed;
            return Err(FourWayError::ReplayCounter);
        }
        // Critical boundary: message 3 must not complete until MIC verification,
        // PTK derivation, encrypted key-data processing and key installation are
        // supplied by a reviewed crypto layer.
        Err(FourWayError::CryptoNotVerified)
    }

    pub fn mark_message3_verified(&mut self, replay_counter: u64) -> Result<(), FourWayError> {
        if self.state != FourWayState::AwaitingMessage3 { return Err(FourWayError::WrongState); }
        if replay_counter < self.last_replay {
            self.state = FourWayState::Failed;
            return Err(FourWayError::ReplayCounter);
        }
        self.last_replay = replay_counter;
        self.state = FourWayState::Completed;
        Ok(())
    }

    pub fn fail(&mut self) {
        self.state = FourWayState::Failed;
        self.anonce = [0; 32];
        self.last_replay = 0;
    }
}

fn synthetic_eapol_key(
    out: &mut [u8],
    key_info: u16,
    replay: u64,
    nonce_seed: u8,
) -> Option<usize> {
    let total = 4 + EAPOL_KEY_FIXED_LEN;
    if out.len() < total { return None; }
    out[..total].fill(0);
    out[0] = 2;
    out[1] = EAPOL_TYPE_KEY;
    out[2..4].copy_from_slice(&(EAPOL_KEY_FIXED_LEN as u16).to_be_bytes());
    out[4] = EAPOL_KEY_DESCRIPTOR_RSN;
    out[5..7].copy_from_slice(&key_info.to_be_bytes());
    out[9..17].copy_from_slice(&replay.to_be_bytes());
    for (i, byte) in out[17..49].iter_mut().enumerate() {
        *byte = nonce_seed.wrapping_add(i as u8);
    }
    out[97..99].copy_from_slice(&0u16.to_be_bytes());
    Some(total)
}

pub fn self_test() -> bool {
    let rsn = [
        1,0, 0,0x0f,0xac,4, 1,0, 0,0x0f,0xac,4,
        1,0, 0,0x0f,0xac,2, 0,0,
    ];
    let Ok(profile) = parse_rsn(&rsn) else { return false; };
    if profile.akm != Akm::Psk || profile.pairwise_cipher != Cipher::Ccmp128 { return false; }

    let mut handshake = FourWayHandshake::new();
    if handshake.begin(profile).is_err() { return false; }

    let mut frame = [0u8; 128];
    let msg1_info = (1 << 3) | (1 << 7) | 2;
    let Some(n1) = synthetic_eapol_key(&mut frame, msg1_info, 1, 0x20) else { return false; };
    let Ok(msg1) = parse_eapol_key(&frame[..n1]) else { return false; };
    if handshake.receive_message1(msg1).is_err()
        || handshake.state() != FourWayState::AwaitingMessage3
        || handshake.anonce()[0] != 0x20
    { return false; }

    let msg3_info = (1 << 3) | (1 << 6) | (1 << 7) | (1 << 8) | (1 << 9) | 2;
    let Some(n3) = synthetic_eapol_key(&mut frame, msg3_info, 2, 0x20) else { return false; };
    let Ok(msg3) = parse_eapol_key(&frame[..n3]) else { return false; };
    if handshake.receive_message3_metadata(msg3) != Err(FourWayError::CryptoNotVerified) {
        return false;
    }
    if handshake.mark_message3_verified(2).is_err()
        || handshake.state() != FourWayState::Completed
    { return false; }

    let sae = [
        1,0, 0,0x0f,0xac,4, 1,0, 0,0x0f,0xac,4,
        1,0, 0,0x0f,0xac,8, 0,0,
    ];
    parse_rsn(&sae).map(|p| p.akm == Akm::Sae) == Ok(true)
}

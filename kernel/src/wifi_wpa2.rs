//! WovenWiFi Stage 13.9G — WPA2 4-way handshake integration.
//!
//! Integrates Stage 13.9E EAPOL parsing/state with Stage 13.9F cryptography.
//! This stage constructs message 2/4 and 4/4, verifies message 3/4 MIC,
//! enforces replay/ANonce checks, and clears key material on failure.
//!
//! GTK unwrap/install and CCMP data protection remain deferred.

use crate::wifi_crypto::{self, CryptoError, Pmk, Ptk};
use crate::wifi_rsn::{self, EapolError, FourWayError, FourWayHandshake, FourWayState, RsnProfile};
use zeroize::Zeroize;

const EAPOL_HEADER_LEN: usize = 4;
const KEY_BODY_LEN: usize = wifi_rsn::EAPOL_KEY_FIXED_LEN;
const EAPOL_KEY_FRAME_LEN: usize = EAPOL_HEADER_LEN + KEY_BODY_LEN;
const KEY_INFO_PAIRWISE: u16 = 1 << 3;
const KEY_INFO_MIC: u16 = 1 << 8;
const KEY_INFO_SECURE: u16 = 1 << 9;
const KEY_DESCRIPTOR_VERSION_HMAC_SHA1_AES: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupplicantState {
    Idle,
    AwaitingMessage1,
    AwaitingMessage3,
    Completed,
    Failed,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SupplicantError {
    WrongState,
    Protocol(EapolError),
    Handshake(FourWayError),
    Crypto(CryptoError),
    WrongPeer,
    WrongNonce,
    BufferTooSmall,
    UnsupportedDescriptorVersion,
    Entropy(crate::entropy::EntropyError),
}

impl From<EapolError> for SupplicantError {
    fn from(value: EapolError) -> Self { Self::Protocol(value) }
}
impl From<FourWayError> for SupplicantError {
    fn from(value: FourWayError) -> Self { Self::Handshake(value) }
}
impl From<CryptoError> for SupplicantError {
    fn from(value: CryptoError) -> Self { Self::Crypto(value) }
}

pub struct Wpa2Supplicant {
    state: SupplicantState,
    handshake: FourWayHandshake,
    station: [u8; 6],
    authenticator: [u8; 6],
    snonce: [u8; 32],
    ptk: Option<Ptk>,
}

impl Wpa2Supplicant {
    pub const fn new(station: [u8; 6]) -> Self {
        Self {
            state: SupplicantState::Idle,
            handshake: FourWayHandshake::new(),
            station,
            authenticator: [0; 6],
            snonce: [0; 32],
            ptk: None,
        }
    }

    pub const fn state(&self) -> SupplicantState { self.state }

    pub fn begin(
        &mut self,
        profile: RsnProfile,
        authenticator: [u8; 6],
        snonce: [u8; 32],
    ) -> Result<(), SupplicantError> {
        if self.state != SupplicantState::Idle {
            return Err(SupplicantError::WrongState);
        }
        self.handshake.begin(profile)?;
        self.authenticator = authenticator;
        self.snonce = snonce;
        self.ptk = None;
        self.state = SupplicantState::AwaitingMessage1;
        Ok(())
    }

    pub fn begin_with_secure_nonce(
        &mut self,
        profile: RsnProfile,
        authenticator: [u8; 6],
    ) -> Result<(), SupplicantError> {
        let snonce = crate::entropy::snonce().map_err(SupplicantError::Entropy)?;
        self.begin(profile, authenticator, snonce)
    }
    pub fn receive_message1_build_message2(
        &mut self,
        pmk: &Pmk,
        message1: &[u8],
        output: &mut [u8],
    ) -> Result<usize, SupplicantError> {
        if self.state != SupplicantState::AwaitingMessage1 {
            return Err(SupplicantError::WrongState);
        }
        let key = wifi_rsn::parse_eapol_key(message1)?;
        if key.key_info.descriptor_version() != KEY_DESCRIPTOR_VERSION_HMAC_SHA1_AES as u8 {
            self.fail();
            return Err(SupplicantError::UnsupportedDescriptorVersion);
        }
        self.handshake.receive_message1(key)?;
        let anonce = self.handshake.anonce();
        let ptk = wifi_crypto::derive_ptk(
            pmk,
            self.authenticator,
            self.station,
            anonce,
            self.snonce,
        )?;

        let len = build_eapol_key(
            output,
            KEY_INFO_PAIRWISE | KEY_INFO_MIC | KEY_DESCRIPTOR_VERSION_HMAC_SHA1_AES,
            key.replay_counter,
            self.snonce,
        )?;
        let mic = wifi_crypto::compute_eapol_mic(ptk.kck(), &output[..len])?;
        output[81..97].copy_from_slice(&mic);

        self.ptk = Some(ptk);
        self.state = SupplicantState::AwaitingMessage3;
        Ok(len)
    }

    pub fn receive_message3_build_message4(
        &mut self,
        message3: &[u8],
        output: &mut [u8],
    ) -> Result<usize, SupplicantError> {
        if self.state != SupplicantState::AwaitingMessage3 {
            return Err(SupplicantError::WrongState);
        }
        let key = wifi_rsn::parse_eapol_key(message3)?;
        if key.key_info.descriptor_version() != KEY_DESCRIPTOR_VERSION_HMAC_SHA1_AES as u8 {
            self.fail();
            return Err(SupplicantError::UnsupportedDescriptorVersion);
        }
        // Stage 13.9E intentionally returns CryptoNotVerified after metadata
        // validation. This is the trust boundary: Stage 13.9G must verify
        // ANonce and MIC before marking message 3 as verified.
        match self.handshake.receive_message3_metadata(key) {
            Err(FourWayError::CryptoNotVerified) => {}
            Err(error) => {
                self.fail();
                return Err(SupplicantError::Handshake(error));
            }
            Ok(()) => {
                self.fail();
                return Err(SupplicantError::Handshake(FourWayError::CryptoNotVerified));
            }
        }

        let Some(ptk) = self.ptk.as_ref() else {
            self.fail();
            return Err(SupplicantError::WrongState);
        };

        if key.nonce != self.handshake.anonce() {
            self.fail();
            return Err(SupplicantError::WrongNonce);
        }

        if let Err(error) = wifi_crypto::verify_eapol_mic(ptk.kck(), message3) {
            self.fail();
            return Err(SupplicantError::Crypto(error));
        }

        self.handshake.mark_message3_verified(key.replay_counter)?;

        let len = build_eapol_key(
            output,
            KEY_INFO_PAIRWISE | KEY_INFO_MIC | KEY_INFO_SECURE
                | KEY_DESCRIPTOR_VERSION_HMAC_SHA1_AES,
            key.replay_counter,
            [0; 32],
        )?;
        let mic = wifi_crypto::compute_eapol_mic(ptk.kck(), &output[..len])?;
        output[81..97].copy_from_slice(&mic);

        self.state = SupplicantState::Completed;
        Ok(len)
    }

    pub fn temporal_key(&self) -> Option<&[u8]> {
        self.ptk.as_ref().map(Ptk::tk)
    }

    pub fn fail(&mut self) {
        self.handshake.fail();
        self.ptk = None;
        self.snonce.zeroize();
        self.authenticator = [0; 6];
        self.state = SupplicantState::Failed;
    }
}

impl Drop for Wpa2Supplicant {
    fn drop(&mut self) {
        self.ptk = None;
        self.snonce.zeroize();
        self.authenticator.zeroize();
    }
}

fn build_eapol_key(
    output: &mut [u8],
    key_info: u16,
    replay_counter: u64,
    nonce: [u8; 32],
) -> Result<usize, SupplicantError> {
    if output.len() < EAPOL_KEY_FRAME_LEN {
        return Err(SupplicantError::BufferTooSmall);
    }
    output[..EAPOL_KEY_FRAME_LEN].fill(0);
    output[0] = 2;
    output[1] = wifi_rsn::EAPOL_TYPE_KEY;
    output[2..4].copy_from_slice(&(KEY_BODY_LEN as u16).to_be_bytes());
    output[4] = wifi_rsn::EAPOL_KEY_DESCRIPTOR_RSN;
    output[5..7].copy_from_slice(&key_info.to_be_bytes());
    output[9..17].copy_from_slice(&replay_counter.to_be_bytes());
    output[17..49].copy_from_slice(&nonce);
    output[97..99].copy_from_slice(&0u16.to_be_bytes());
    Ok(EAPOL_KEY_FRAME_LEN)
}

fn sign_frame(ptk: &Ptk, frame: &mut [u8]) -> Result<(), SupplicantError> {
    let mic = wifi_crypto::compute_eapol_mic(ptk.kck(), frame)?;
    frame[81..97].copy_from_slice(&mic);
    Ok(())
}

pub fn self_test() -> bool {
    let rsn = [
        1,0, 0,0x0f,0xac,4, 1,0, 0,0x0f,0xac,4,
        1,0, 0,0x0f,0xac,2, 0,0,
    ];
    let Ok(profile) = wifi_rsn::parse_rsn(&rsn) else { return false; };
    let Ok(pmk) = wifi_crypto::derive_pmk(b"password", b"IEEE") else { return false; };

    let station = [0x00,0x13,0x46,0xfe,0x32,0x0c];
    let ap = [0x00,0x14,0x6c,0x7e,0x40,0x80];
    let snonce = [0x22u8; 32];
    let anonce = [0x11u8; 32];

    let mut supplicant = Wpa2Supplicant::new(station);
    if supplicant.begin(profile, ap, snonce).is_err() { return false; }

    let mut msg1 = [0u8; EAPOL_KEY_FRAME_LEN];
    let Ok(msg1_len) = build_eapol_key(
        &mut msg1,
        KEY_INFO_PAIRWISE | (1 << 7) | KEY_DESCRIPTOR_VERSION_HMAC_SHA1_AES,
        1,
        anonce,
    ) else { return false; };

    let mut msg2 = [0u8; EAPOL_KEY_FRAME_LEN];
    let Ok(msg2_len) = supplicant.receive_message1_build_message2(
        &pmk,
        &msg1[..msg1_len],
        &mut msg2,
    ) else { return false; };
    if msg2_len != EAPOL_KEY_FRAME_LEN
        || wifi_crypto::verify_eapol_mic(
            supplicant.ptk.as_ref().map(Ptk::kck).unwrap_or(&[]),
            &msg2[..msg2_len],
        ).is_err()
    {
        return false;
    }

    let Ok(ap_ptk) = wifi_crypto::derive_ptk(&pmk, ap, station, anonce, snonce) else {
        return false;
    };
    let mut msg3 = [0u8; EAPOL_KEY_FRAME_LEN];
    let msg3_info =
        KEY_INFO_PAIRWISE | (1 << 6) | (1 << 7) | KEY_INFO_MIC | KEY_INFO_SECURE
            | KEY_DESCRIPTOR_VERSION_HMAC_SHA1_AES;
    let Ok(msg3_len) = build_eapol_key(&mut msg3, msg3_info, 2, anonce) else {
        return false;
    };
    if sign_frame(&ap_ptk, &mut msg3[..msg3_len]).is_err() { return false; }

    let mut msg4 = [0u8; EAPOL_KEY_FRAME_LEN];
    let Ok(msg4_len) = supplicant.receive_message3_build_message4(
        &msg3[..msg3_len],
        &mut msg4,
    ) else { return false; };

    if supplicant.state() != SupplicantState::Completed
        || supplicant.handshake.state() != FourWayState::Completed
        || supplicant.temporal_key().is_none()
        || wifi_crypto::verify_eapol_mic(ap_ptk.kck(), &msg4[..msg4_len]).is_err()
    {
        return false;
    }

    let mut tamper = Wpa2Supplicant::new(station);
    if tamper.begin(profile, ap, snonce).is_err() { return false; }
    let mut discard = [0u8; EAPOL_KEY_FRAME_LEN];
    if tamper.receive_message1_build_message2(&pmk, &msg1, &mut discard).is_err() {
        return false;
    }
    // Corrupt MIC-covered message-3 material outside the ANonce.  Changing
    // byte 20 mutates the ANonce and correctly triggers WrongNonce before
    // MIC verification; that made the old negative test expect the wrong
    // failure class. Byte 49 is in the key-IV field: it remains covered by
    // the EAPOL MIC while leaving the independently validated ANonce intact.
    msg3[49] ^= 1;
    if !matches!(
        tamper.receive_message3_build_message4(&msg3, &mut discard),
        Err(SupplicantError::Crypto(CryptoError::MicMismatch))
    ) || tamper.state() != SupplicantState::Failed || tamper.temporal_key().is_some()
    {
        return false;
    }

    true
}

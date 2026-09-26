//! WovenWiFi Stage 13.9F — WPA2 cryptographic foundation.
//!
//! Uses reviewed RustCrypto primitives for WPA2-Personal PMK derivation,
//! PTK derivation, EAPOL MIC calculation/verification, and secret zeroization.
//! This remains protocol-foundation work: GTK unwrap/install and CCMP data
//! encryption/decryption are later stages.

use core::cmp::Ordering;
use hmac::{Hmac, Mac};
use pbkdf2::pbkdf2_hmac;
use sha1::Sha1;
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

pub const PMK_LEN: usize = 32;
pub const PTK_LEN: usize = 48;
pub const KCK_LEN: usize = 16;
pub const KEK_LEN: usize = 16;
pub const TK_LEN: usize = 16;

#[derive(Debug, PartialEq, Eq)]
pub enum CryptoError {
    InvalidPassphrase,
    InvalidSsid,
    InvalidEapolFrame,
    MicMismatch,
    Hmac,
}

pub struct Pmk([u8; PMK_LEN]);

impl Pmk {
    pub fn expose(&self) -> &[u8; PMK_LEN] {
        &self.0
    }
}
impl Drop for Pmk {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

pub struct Ptk([u8; PTK_LEN]);

impl Ptk {
    #[cfg(feature = "stage13-9-test")]
    pub fn from_test_bytes(bytes: [u8; PTK_LEN]) -> Self {
        Self(bytes)
    }
    pub fn kck(&self) -> &[u8] {
        &self.0[..KCK_LEN]
    }
    pub fn kek(&self) -> &[u8] {
        &self.0[KCK_LEN..KCK_LEN + KEK_LEN]
    }
    pub fn tk(&self) -> &[u8] {
        &self.0[KCK_LEN + KEK_LEN..]
    }
}
impl Drop for Ptk {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

pub fn derive_pmk(passphrase: &[u8], ssid: &[u8]) -> Result<Pmk, CryptoError> {
    if !(8..=63).contains(&passphrase.len()) {
        return Err(CryptoError::InvalidPassphrase);
    }
    if ssid.is_empty() || ssid.len() > 32 {
        return Err(CryptoError::InvalidSsid);
    }
    let mut pmk = [0u8; PMK_LEN];
    pbkdf2_hmac::<Sha1>(passphrase, ssid, 4096, &mut pmk);
    Ok(Pmk(pmk))
}

fn lexicographic_min_max<const N: usize>(a: [u8; N], b: [u8; N]) -> ([u8; N], [u8; N]) {
    match a.cmp(&b) {
        Ordering::Greater => (b, a),
        _ => (a, b),
    }
}

pub fn derive_ptk(
    pmk: &Pmk,
    aa: [u8; 6],
    spa: [u8; 6],
    anonce: [u8; 32],
    snonce: [u8; 32],
) -> Result<Ptk, CryptoError> {
    let (mac1, mac2) = lexicographic_min_max(aa, spa);
    let (nonce1, nonce2) = lexicographic_min_max(anonce, snonce);

    let mut context = [0u8; 76];
    context[..6].copy_from_slice(&mac1);
    context[6..12].copy_from_slice(&mac2);
    context[12..44].copy_from_slice(&nonce1);
    context[44..76].copy_from_slice(&nonce2);

    let label = b"Pairwise key expansion";
    let mut ptk = [0u8; PTK_LEN];
    let mut generated = 0usize;
    let mut counter = 0u8;

    while generated < PTK_LEN {
        let mut mac = Hmac::<Sha1>::new_from_slice(pmk.expose()).map_err(|_| CryptoError::Hmac)?;
        mac.update(label);
        mac.update(&[0]);
        mac.update(&context);
        mac.update(&[counter]);
        let digest = mac.finalize().into_bytes();
        let take = core::cmp::min(digest.len(), PTK_LEN - generated);
        ptk[generated..generated + take].copy_from_slice(&digest[..take]);
        generated += take;
        counter = counter.wrapping_add(1);
    }

    context.zeroize();
    Ok(Ptk(ptk))
}

pub fn compute_eapol_mic(kck: &[u8], eapol_frame: &[u8]) -> Result<[u8; 16], CryptoError> {
    if kck.len() != KCK_LEN || eapol_frame.len() < 4 + 95 {
        return Err(CryptoError::InvalidEapolFrame);
    }
    let mut frame = [0u8; 256];
    if eapol_frame.len() > frame.len() {
        return Err(CryptoError::InvalidEapolFrame);
    }
    frame[..eapol_frame.len()].copy_from_slice(eapol_frame);
    let mic_start = 4 + 77;
    let mic_end = mic_start + 16;
    frame[mic_start..mic_end].fill(0);

    let mut mac = Hmac::<Sha1>::new_from_slice(kck).map_err(|_| CryptoError::Hmac)?;
    mac.update(&frame[..eapol_frame.len()]);
    let digest = mac.finalize().into_bytes();
    let mut mic = [0u8; 16];
    mic.copy_from_slice(&digest[..16]);
    frame.zeroize();
    Ok(mic)
}

pub fn verify_eapol_mic(kck: &[u8], eapol_frame: &[u8]) -> Result<(), CryptoError> {
    if eapol_frame.len() < 4 + 95 {
        return Err(CryptoError::InvalidEapolFrame);
    }
    let expected = compute_eapol_mic(kck, eapol_frame)?;
    let mic_start = 4 + 77;
    let mic_end = mic_start + 16;
    let actual = eapol_frame
        .get(mic_start..mic_end)
        .ok_or(CryptoError::InvalidEapolFrame)?;
    if bool::from(expected.ct_eq(actual)) {
        Ok(())
    } else {
        Err(CryptoError::MicMismatch)
    }
}

pub fn self_test() -> bool {
    // IEEE 802.11i / common WPA2 PBKDF2 test vector:
    // passphrase "password", SSID "IEEE".
    let Ok(pmk) = derive_pmk(b"password", b"IEEE") else {
        return false;
    };
    let expected_pmk: [u8; 32] = [
        0xf4, 0x2c, 0x6f, 0xc5, 0x2d, 0xf0, 0xeb, 0xef, 0x9e, 0xbb, 0x4b, 0x90, 0xb3, 0x8a, 0x5f,
        0x90, 0x2e, 0x83, 0xfe, 0x1b, 0x13, 0x5a, 0x70, 0xe2, 0x3a, 0xed, 0x76, 0x2e, 0x97, 0x10,
        0xa1, 0x2e,
    ];
    if pmk.expose() != &expected_pmk {
        return false;
    }

    let aa = [0x00, 0x14, 0x6c, 0x7e, 0x40, 0x80];
    let spa = [0x00, 0x13, 0x46, 0xfe, 0x32, 0x0c];
    let anonce = [0x11u8; 32];
    let snonce = [0x22u8; 32];
    let Ok(ptk1) = derive_ptk(&pmk, aa, spa, anonce, snonce) else {
        return false;
    };
    let Ok(ptk2) = derive_ptk(&pmk, spa, aa, snonce, anonce) else {
        return false;
    };
    if ptk1.kck() != ptk2.kck() || ptk1.kek() != ptk2.kek() || ptk1.tk() != ptk2.tk() {
        return false;
    }

    let mut eapol = [0u8; 99];
    eapol[0] = 2;
    eapol[1] = 3;
    eapol[2..4].copy_from_slice(&95u16.to_be_bytes());
    eapol[4] = 2;
    let Ok(mic) = compute_eapol_mic(ptk1.kck(), &eapol) else {
        return false;
    };
    eapol[81..97].copy_from_slice(&mic);
    if verify_eapol_mic(ptk1.kck(), &eapol).is_err() {
        return false;
    }
    eapol[10] ^= 1;
    verify_eapol_mic(ptk1.kck(), &eapol) == Err(CryptoError::MicMismatch)
}

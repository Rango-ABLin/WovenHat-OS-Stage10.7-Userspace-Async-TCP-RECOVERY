//! WovenWiFi Stage 13.9H — encrypted GTK key-data handling.
//!
//! Scalar, safe-Rust AES-128 is deliberately local to the RFC 3394 key-wrap
//! boundary because RustCrypto aes 0.8.4 triggers an LLVM backend failure on
//! WovenHat's pinned x86_64-unknown-none toolchain. No unsafe code or SIMD is used.

use subtle::ConstantTimeEq;
use zeroize::Zeroize;

const WRAP_IV: [u8; 8] = [0xa6; 8];
const MAX_KEY_DATA: usize = 64;
pub const GTK_LEN: usize = 16;
const RSN_OUI: [u8; 3] = [0x00, 0x0f, 0xac];
const GTK_KDE_TYPE: u8 = 1;

const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];
const INV_SBOX: [u8; 256] = [
    0x52, 0x09, 0x6a, 0xd5, 0x30, 0x36, 0xa5, 0x38, 0xbf, 0x40, 0xa3, 0x9e, 0x81, 0xf3, 0xd7, 0xfb,
    0x7c, 0xe3, 0x39, 0x82, 0x9b, 0x2f, 0xff, 0x87, 0x34, 0x8e, 0x43, 0x44, 0xc4, 0xde, 0xe9, 0xcb,
    0x54, 0x7b, 0x94, 0x32, 0xa6, 0xc2, 0x23, 0x3d, 0xee, 0x4c, 0x95, 0x0b, 0x42, 0xfa, 0xc3, 0x4e,
    0x08, 0x2e, 0xa1, 0x66, 0x28, 0xd9, 0x24, 0xb2, 0x76, 0x5b, 0xa2, 0x49, 0x6d, 0x8b, 0xd1, 0x25,
    0x72, 0xf8, 0xf6, 0x64, 0x86, 0x68, 0x98, 0x16, 0xd4, 0xa4, 0x5c, 0xcc, 0x5d, 0x65, 0xb6, 0x92,
    0x6c, 0x70, 0x48, 0x50, 0xfd, 0xed, 0xb9, 0xda, 0x5e, 0x15, 0x46, 0x57, 0xa7, 0x8d, 0x9d, 0x84,
    0x90, 0xd8, 0xab, 0x00, 0x8c, 0xbc, 0xd3, 0x0a, 0xf7, 0xe4, 0x58, 0x05, 0xb8, 0xb3, 0x45, 0x06,
    0xd0, 0x2c, 0x1e, 0x8f, 0xca, 0x3f, 0x0f, 0x02, 0xc1, 0xaf, 0xbd, 0x03, 0x01, 0x13, 0x8a, 0x6b,
    0x3a, 0x91, 0x11, 0x41, 0x4f, 0x67, 0xdc, 0xea, 0x97, 0xf2, 0xcf, 0xce, 0xf0, 0xb4, 0xe6, 0x73,
    0x96, 0xac, 0x74, 0x22, 0xe7, 0xad, 0x35, 0x85, 0xe2, 0xf9, 0x37, 0xe8, 0x1c, 0x75, 0xdf, 0x6e,
    0x47, 0xf1, 0x1a, 0x71, 0x1d, 0x29, 0xc5, 0x89, 0x6f, 0xb7, 0x62, 0x0e, 0xaa, 0x18, 0xbe, 0x1b,
    0xfc, 0x56, 0x3e, 0x4b, 0xc6, 0xd2, 0x79, 0x20, 0x9a, 0xdb, 0xc0, 0xfe, 0x78, 0xcd, 0x5a, 0xf4,
    0x1f, 0xdd, 0xa8, 0x33, 0x88, 0x07, 0xc7, 0x31, 0xb1, 0x12, 0x10, 0x59, 0x27, 0x80, 0xec, 0x5f,
    0x60, 0x51, 0x7f, 0xa9, 0x19, 0xb5, 0x4a, 0x0d, 0x2d, 0xe5, 0x7a, 0x9f, 0x93, 0xc9, 0x9c, 0xef,
    0xa0, 0xe0, 0x3b, 0x4d, 0xae, 0x2a, 0xf5, 0xb0, 0xc8, 0xeb, 0xbb, 0x3c, 0x83, 0x53, 0x99, 0x61,
    0x17, 0x2b, 0x04, 0x7e, 0xba, 0x77, 0xd6, 0x26, 0xe1, 0x69, 0x14, 0x63, 0x55, 0x21, 0x0c, 0x7d,
];
const RCON: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GtkError {
    InvalidKek,
    InvalidWrappedLength,
    Integrity,
    KeyDataTooLarge,
    MalformedKde,
    MissingGtk,
    InvalidGtkLength,
    Replay,
    ConflictingRetransmission,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed,
    Retransmission,
}

pub struct GroupTemporalKey {
    key: [u8; GTK_LEN],
    index: u8,
}
impl GroupTemporalKey {
    pub const fn index(&self) -> u8 {
        self.index
    }
    pub(crate) fn expose(&self) -> &[u8; GTK_LEN] {
        &self.key
    }
    #[cfg(feature = "stage13-9-test")]
    pub fn from_test_bytes(index: u8, key: [u8; GTK_LEN]) -> Self {
        Self {
            key,
            index: index & 3,
        }
    }
}
impl Drop for GroupTemporalKey {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

pub struct GroupKeyStore {
    gtk: Option<GroupTemporalKey>,
    replay_counter: u64,
    installed: bool,
    installs: u64,
}
impl GroupKeyStore {
    pub const fn new() -> Self {
        Self {
            gtk: None,
            replay_counter: 0,
            installed: false,
            installs: 0,
        }
    }
    pub const fn install_count(&self) -> u64 {
        self.installs
    }
    pub fn gtk(&self) -> Option<&GroupTemporalKey> {
        self.gtk.as_ref()
    }
    pub fn install_verified(
        &mut self,
        replay: u64,
        candidate: GroupTemporalKey,
    ) -> Result<InstallOutcome, GtkError> {
        if self.installed {
            if replay < self.replay_counter {
                return Err(GtkError::Replay);
            }
            if replay == self.replay_counter {
                let same = self.gtk.as_ref().is_some_and(|g| {
                    g.index == candidate.index && bool::from(g.key.ct_eq(&candidate.key))
                });
                return if same {
                    Ok(InstallOutcome::Retransmission)
                } else {
                    Err(GtkError::ConflictingRetransmission)
                };
            }
        }
        self.gtk = Some(candidate);
        self.replay_counter = replay;
        self.installed = true;
        self.installs = self.installs.saturating_add(1);
        Ok(InstallOutcome::Installed)
    }
    pub fn clear(&mut self) {
        self.gtk = None;
        self.replay_counter = 0;
        self.installed = false;
    }
}

fn xtime(x: u8) -> u8 {
    (x << 1) ^ if x & 0x80 != 0 { 0x1b } else { 0 }
}
fn mul(mut a: u8, mut b: u8) -> u8 {
    let mut r = 0;
    while b != 0 {
        if b & 1 != 0 {
            r ^= a;
        }
        a = xtime(a);
        b >>= 1;
    }
    r
}
fn add_round_key(s: &mut [u8; 16], rk: &[u8]) {
    for i in 0..16 {
        s[i] ^= rk[i];
    }
}
fn sub_bytes(s: &mut [u8; 16]) {
    for x in s.iter_mut() {
        *x = SBOX[*x as usize];
    }
}
fn inv_sub_bytes(s: &mut [u8; 16]) {
    for x in s.iter_mut() {
        *x = INV_SBOX[*x as usize];
    }
}
fn shift_rows(s: &mut [u8; 16]) {
    let t = *s;
    s[0] = t[0];
    s[1] = t[5];
    s[2] = t[10];
    s[3] = t[15];
    s[4] = t[4];
    s[5] = t[9];
    s[6] = t[14];
    s[7] = t[3];
    s[8] = t[8];
    s[9] = t[13];
    s[10] = t[2];
    s[11] = t[7];
    s[12] = t[12];
    s[13] = t[1];
    s[14] = t[6];
    s[15] = t[11];
}
fn inv_shift_rows(s: &mut [u8; 16]) {
    let t = *s;
    s[0] = t[0];
    s[1] = t[13];
    s[2] = t[10];
    s[3] = t[7];
    s[4] = t[4];
    s[5] = t[1];
    s[6] = t[14];
    s[7] = t[11];
    s[8] = t[8];
    s[9] = t[5];
    s[10] = t[2];
    s[11] = t[15];
    s[12] = t[12];
    s[13] = t[9];
    s[14] = t[6];
    s[15] = t[3];
}
fn mix_columns(s: &mut [u8; 16]) {
    for c in 0..4 {
        let i = 4 * c;
        let a = [s[i], s[i + 1], s[i + 2], s[i + 3]];
        s[i] = mul(a[0], 2) ^ mul(a[1], 3) ^ a[2] ^ a[3];
        s[i + 1] = a[0] ^ mul(a[1], 2) ^ mul(a[2], 3) ^ a[3];
        s[i + 2] = a[0] ^ a[1] ^ mul(a[2], 2) ^ mul(a[3], 3);
        s[i + 3] = mul(a[0], 3) ^ a[1] ^ a[2] ^ mul(a[3], 2);
    }
}
fn inv_mix_columns(s: &mut [u8; 16]) {
    for c in 0..4 {
        let i = 4 * c;
        let a = [s[i], s[i + 1], s[i + 2], s[i + 3]];
        s[i] = mul(a[0], 14) ^ mul(a[1], 11) ^ mul(a[2], 13) ^ mul(a[3], 9);
        s[i + 1] = mul(a[0], 9) ^ mul(a[1], 14) ^ mul(a[2], 11) ^ mul(a[3], 13);
        s[i + 2] = mul(a[0], 13) ^ mul(a[1], 9) ^ mul(a[2], 14) ^ mul(a[3], 11);
        s[i + 3] = mul(a[0], 11) ^ mul(a[1], 13) ^ mul(a[2], 9) ^ mul(a[3], 14);
    }
}
fn expand_key(key: &[u8; 16]) -> [u8; 176] {
    let mut w = [0u8; 176];
    w[..16].copy_from_slice(key);
    let mut n = 16;
    let mut r = 0;
    while n < 176 {
        let mut t = [w[n - 4], w[n - 3], w[n - 2], w[n - 1]];
        if n % 16 == 0 {
            t = [
                SBOX[t[1] as usize],
                SBOX[t[2] as usize],
                SBOX[t[3] as usize],
                SBOX[t[0] as usize],
            ];
            t[0] ^= RCON[r];
            r += 1;
        }
        for &v in &t {
            w[n] = w[n - 16] ^ v;
            n += 1;
        }
    }
    w
}
fn aes_encrypt(key: &[u8; 16], block: &mut [u8; 16]) {
    let mut w = expand_key(key);
    add_round_key(block, &w[..16]);
    for r in 1..10 {
        sub_bytes(block);
        shift_rows(block);
        mix_columns(block);
        add_round_key(block, &w[16 * r..16 * (r + 1)]);
    }
    sub_bytes(block);
    shift_rows(block);
    add_round_key(block, &w[160..176]);
    w.zeroize();
}
fn aes_decrypt(key: &[u8; 16], block: &mut [u8; 16]) {
    let mut w = expand_key(key);
    add_round_key(block, &w[160..176]);
    for r in (1..10).rev() {
        inv_shift_rows(block);
        inv_sub_bytes(block);
        add_round_key(block, &w[16 * r..16 * (r + 1)]);
        inv_mix_columns(block);
    }
    inv_shift_rows(block);
    inv_sub_bytes(block);
    add_round_key(block, &w[..16]);
    w.zeroize();
}
fn key16(kek: &[u8]) -> Result<[u8; 16], GtkError> {
    if kek.len() != 16 {
        return Err(GtkError::InvalidKek);
    }
    let mut k = [0u8; 16];
    k.copy_from_slice(kek);
    Ok(k)
}

pub fn unwrap_key_data(kek: &[u8], wrapped: &[u8], output: &mut [u8]) -> Result<usize, GtkError> {
    if wrapped.len() < 24 || !wrapped.len().is_multiple_of(8) {
        return Err(GtkError::InvalidWrappedLength);
    }
    let plen = wrapped.len() - 8;
    if plen > MAX_KEY_DATA || output.len() < plen {
        return Err(GtkError::KeyDataTooLarge);
    }
    let n = plen / 8;
    let mut key = key16(kek)?;
    let mut a = [0u8; 8];
    a.copy_from_slice(&wrapped[..8]);
    output[..plen].copy_from_slice(&wrapped[8..]);
    for j in (0..=5u64).rev() {
        for i in (1..=n).rev() {
            let t = n as u64 * j + i as u64;
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&(u64::from_be_bytes(a) ^ t).to_be_bytes());
            b[8..].copy_from_slice(&output[(i - 1) * 8..i * 8]);
            aes_decrypt(&key, &mut b);
            a.copy_from_slice(&b[..8]);
            output[(i - 1) * 8..i * 8].copy_from_slice(&b[8..]);
            b.zeroize();
        }
    }
    key.zeroize();
    if !bool::from(a.ct_eq(&WRAP_IV)) {
        output[..plen].zeroize();
        return Err(GtkError::Integrity);
    }
    Ok(plen)
}
pub fn parse_gtk_kde(data: &[u8]) -> Result<GroupTemporalKey, GtkError> {
    let mut o = 0;
    while o < data.len() {
        if data[o] == 0 {
            o += 1;
            continue;
        }
        let id = data[o];
        let len = *data.get(o + 1).ok_or(GtkError::MalformedKde)? as usize;
        let end = o.checked_add(2 + len).ok_or(GtkError::MalformedKde)?;
        let b = data.get(o + 2..end).ok_or(GtkError::MalformedKde)?;
        if id == 0xdd && b.len() >= 6 && b[..3] == RSN_OUI && b[3] == GTK_KDE_TYPE {
            let gtk = &b[6..];
            if gtk.len() != GTK_LEN {
                return Err(GtkError::InvalidGtkLength);
            }
            let mut key = [0u8; GTK_LEN];
            key.copy_from_slice(gtk);
            return Ok(GroupTemporalKey {
                key,
                index: b[4] & 3,
            });
        }
        o = end;
    }
    Err(GtkError::MissingGtk)
}
pub fn unwrap_gtk(kek: &[u8], wrapped: &[u8]) -> Result<GroupTemporalKey, GtkError> {
    let mut p = [0u8; MAX_KEY_DATA];
    let len = unwrap_key_data(kek, wrapped, &mut p)?;
    let r = parse_gtk_kde(&p[..len]);
    p.zeroize();
    r
}

fn wrap_test(kek: &[u8], plain: &[u8], out: &mut [u8]) -> Result<usize, GtkError> {
    if plain.len() < 16 || !plain.len().is_multiple_of(8) {
        return Err(GtkError::InvalidWrappedLength);
    }
    if plain.len() > MAX_KEY_DATA || out.len() < plain.len() + 8 {
        return Err(GtkError::KeyDataTooLarge);
    }
    let n = plain.len() / 8;
    let mut key = key16(kek)?;
    let mut a = WRAP_IV;
    out[8..8 + plain.len()].copy_from_slice(plain);
    for j in 0..=5u64 {
        for i in 1..=n {
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&a);
            b[8..].copy_from_slice(&out[i * 8..(i + 1) * 8]);
            aes_encrypt(&key, &mut b);
            let mut ah = [0u8; 8];
            ah.copy_from_slice(&b[..8]);
            a = (u64::from_be_bytes(ah) ^ (n as u64 * j + i as u64)).to_be_bytes();
            out[i * 8..(i + 1) * 8].copy_from_slice(&b[8..]);
            b.zeroize();
        }
    }
    key.zeroize();
    out[..8].copy_from_slice(&a);
    Ok(plain.len() + 8)
}
#[cfg(feature = "stage13-9-test")]
pub fn wrap_gtk_for_test(
    kek: &[u8],
    index: u8,
    gtk: [u8; GTK_LEN],
    out: &mut [u8],
) -> Result<usize, GtkError> {
    let mut kde = [0u8; 24];
    kde[0] = 0xdd;
    kde[1] = 22;
    kde[2..5].copy_from_slice(&RSN_OUI);
    kde[5] = GTK_KDE_TYPE;
    kde[6] = index & 3;
    kde[7] = 0;
    kde[8..24].copy_from_slice(&gtk);
    wrap_test(kek, &kde, out)
}
pub fn self_test() -> bool {
    // FIPS-197 AES-128 known-answer vector, independent of RFC 3394.
    let key = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f,
    ];
    let mut block = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ];
    let original = block;
    let expected = [
        0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4, 0xc5,
        0x5a,
    ];
    aes_encrypt(&key, &mut block);
    if block != expected {
        return false;
    }
    aes_decrypt(&key, &mut block);
    if block != original {
        return false;
    }

    let wrapped = [
        0x1f, 0xa6, 0x8b, 0x0a, 0x81, 0x12, 0xb4, 0x47, 0xae, 0xf3, 0x4b, 0xd8, 0xfb, 0x5a, 0x7b,
        0x82, 0x9d, 0x3e, 0x86, 0x23, 0x71, 0xd2, 0xcf, 0xe5,
    ];
    let mut u = [0u8; MAX_KEY_DATA];
    let Ok(n) = unwrap_key_data(&key, &wrapped, &mut u) else {
        return false;
    };
    if n != 16 || u[..16] != original {
        return false;
    }

    let gtk = [0x5au8; GTK_LEN];
    let mut kde = [0u8; 24];
    kde[0] = 0xdd;
    kde[1] = 22;
    kde[2..5].copy_from_slice(&RSN_OUI);
    kde[5] = 1;
    kde[6] = 1;
    kde[7] = 0;
    kde[8..24].copy_from_slice(&gtk);
    let tk = [0x33u8; 16];
    let mut enc = [0u8; 40];
    let Ok(en) = wrap_test(&tk, &kde, &mut enc) else {
        return false;
    };
    let Ok(first) = unwrap_gtk(&tk, &enc[..en]) else {
        return false;
    };
    if first.index() != 1 || first.expose() != &gtk {
        return false;
    }
    let mut store = GroupKeyStore::new();
    if store.install_verified(7, first) != Ok(InstallOutcome::Installed)
        || store.install_count() != 1
    {
        return false;
    }
    let Ok(retry) = unwrap_gtk(&tk, &enc[..en]) else {
        return false;
    };
    if store.install_verified(7, retry) != Ok(InstallOutcome::Retransmission)
        || store.install_count() != 1
    {
        return false;
    }
    kde[8] ^= 1;
    let Ok(cn) = wrap_test(&tk, &kde, &mut enc) else {
        return false;
    };
    let Ok(conflict) = unwrap_gtk(&tk, &enc[..cn]) else {
        return false;
    };
    if store.install_verified(7, conflict) != Err(GtkError::ConflictingRetransmission)
        || store.install_count() != 1
    {
        return false;
    }
    let mut bad = wrapped;
    bad[0] ^= 1;
    if unwrap_key_data(&key, &bad, &mut u) != Err(GtkError::Integrity) {
        return false;
    }
    true
}

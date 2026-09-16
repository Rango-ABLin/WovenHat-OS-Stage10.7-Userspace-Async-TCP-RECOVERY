//! Volume integrity envelope boundary. Cipher selection and key storage remain
//! outside the kernel; callers provide per-user key material.
pub fn authentication_tag(key: &[u8], nonce: u64, data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for lane in 0..4 { let mut h = 0xcbf29ce484222325u64 ^ nonce.rotate_left(lane as u32); for b in key.iter().chain(data) { h = (h ^ u64::from(*b)).wrapping_mul(0x100000001b3); } out[lane*8..lane*8+8].copy_from_slice(&h.to_le_bytes()); }
    out
}
pub fn verify(key: &[u8], nonce: u64, data: &[u8], tag: &[u8; 32]) -> bool { authentication_tag(key, nonce, data) == *tag }
#[cfg(feature = "stage12-3-test")]
pub fn structural_self_test() -> bool { let tag=authentication_tag(b"user-key", 9, b"volume"); verify(b"user-key",9,b"volume",&tag) && !verify(b"other",9,b"volume",&tag) }

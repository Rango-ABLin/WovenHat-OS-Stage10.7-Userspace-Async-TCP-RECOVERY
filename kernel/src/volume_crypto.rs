//! Authenticated encryption and bounded key lifecycle for volume records.
//!
//! ChaCha20-Poly1305 is used in detached, in-place mode so callers can encrypt
//! sectors without a heap allocation. Keys are never returned from the vault;
//! callers receive a generation-safe handle that can be revoked explicitly.

use crate::irq_lock::IrqMutex as Mutex;
use poly1305::{
    universal_hash::{NewUniversalHash, UniversalHash},
    Poly1305,
};

pub const KEY_SIZE: usize = 32;
pub const TAG_SIZE: usize = 16;
const MAX_KEYS: usize = 8;

pub type Tag = [u8; TAG_SIZE];

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for index in 0..left.len() {
        difference |= left[index] ^ right[index];
    }
    difference == 0
}

#[inline]
fn load32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[inline]
fn store32(out: &mut [u8], value: u32) {
    out[..4].copy_from_slice(&value.to_le_bytes());
}

#[inline]
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(12);
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(7);
}

fn chacha_block(key: &[u8; KEY_SIZE], nonce: &[u8; 12], counter: u32) -> [u8; 64] {
    let mut state = [0u32; 16];
    state[..4].copy_from_slice(&[0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574]);
    for index in 0..8 {
        state[4 + index] = load32(&key[index * 4..]);
    }
    state[12] = counter;
    for index in 0..3 {
        state[13 + index] = load32(&nonce[index * 4..]);
    }
    let original = state;
    for _ in 0..10 {
        quarter_round(&mut state, 0, 4, 8, 12);
        quarter_round(&mut state, 1, 5, 9, 13);
        quarter_round(&mut state, 2, 6, 10, 14);
        quarter_round(&mut state, 3, 7, 11, 15);
        quarter_round(&mut state, 0, 5, 10, 15);
        quarter_round(&mut state, 1, 6, 11, 12);
        quarter_round(&mut state, 2, 7, 8, 13);
        quarter_round(&mut state, 3, 4, 9, 14);
    }
    let mut block = [0u8; 64];
    for index in 0..16 {
        store32(
            &mut block[index * 4..],
            state[index].wrapping_add(original[index]),
        );
    }
    block
}

fn xor_stream(key: &[u8; KEY_SIZE], nonce: &[u8; 12], counter: u32, data: &mut [u8]) {
    for (block_index, chunk) in data.chunks_mut(64).enumerate() {
        let stream = chacha_block(key, nonce, counter.wrapping_add(block_index as u32));
        for (dst, src) in chunk.iter_mut().zip(stream) {
            *dst ^= src;
        }
    }
}

fn authentication_tag(
    key: &[u8; KEY_SIZE],
    nonce: &[u8; 12],
    aad: &[u8],
    ciphertext: &[u8],
) -> Tag {
    let block0 = chacha_block(key, nonce, 0);
    let mut poly_key = poly1305::Key::default();
    poly_key.copy_from_slice(&block0[..32]);
    let mut poly = Poly1305::new(&poly_key);
    poly.update_padded(aad);
    poly.update_padded(ciphertext);
    let mut lengths = poly1305::Block::default();
    lengths[..8].copy_from_slice(&(aad.len() as u64).to_le_bytes());
    lengths[8..].copy_from_slice(&(ciphertext.len() as u64).to_le_bytes());
    poly.update(&lengths);
    let output = poly.finalize().into_bytes();
    let mut tag = [0u8; TAG_SIZE];
    tag.copy_from_slice(&output);
    tag
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyHandle(u64);

#[derive(Clone, Copy)]
struct KeySlot {
    generation: u32,
    in_use: bool,
    owner: u64,
    key: [u8; KEY_SIZE],
}

impl KeySlot {
    const EMPTY: Self = Self {
        generation: 0,
        in_use: false,
        owner: 0,
        key: [0; KEY_SIZE],
    };

    fn wipe(&mut self) {
        for byte in &mut self.key {
            unsafe {
                core::ptr::write_volatile(byte, 0);
            }
        }
        self.in_use = false;
    }
}

struct KeyVault {
    slots: [KeySlot; MAX_KEYS],
    next_generation: u32,
}

impl KeyVault {
    const fn new() -> Self {
        Self {
            slots: [KeySlot::EMPTY; MAX_KEYS],
            next_generation: 1,
        }
    }

    fn provision(&mut self, owner: u64, key: [u8; KEY_SIZE]) -> Option<KeyHandle> {
        if !key.iter().any(|byte| *byte != 0) {
            return None;
        }
        let slot = self.slots.iter().position(|entry| !entry.in_use)?;
        let mut generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        if generation == 0 {
            generation = 1;
            self.next_generation = 2;
        }
        self.slots[slot] = KeySlot {
            generation,
            in_use: true,
            owner,
            key,
        };
        Some(KeyHandle((u64::from(generation) << 8) | slot as u64))
    }

    fn lookup(&self, owner: u64, handle: KeyHandle) -> Option<[u8; KEY_SIZE]> {
        let slot = (handle.0 & 0xff) as usize;
        let generation = (handle.0 >> 8) as u32;
        let entry = self.slots.get(slot)?;
        (entry.in_use && entry.owner == owner && entry.generation == generation)
            .then_some(entry.key)
    }

    fn revoke(&mut self, owner: u64, handle: KeyHandle) -> bool {
        let slot = (handle.0 & 0xff) as usize;
        let generation = (handle.0 >> 8) as u32;
        let Some(entry) = self.slots.get_mut(slot) else {
            return false;
        };
        if !entry.in_use || entry.owner != owner || entry.generation != generation {
            return false;
        }
        entry.wipe();
        entry.owner = 0;
        true
    }
}

static KEY_VAULT: Mutex<KeyVault> = Mutex::with_rank(KeyVault::new(), 10);

fn nonce_bytes(nonce: u64) -> [u8; 12] {
    let mut bytes = [0u8; 12];
    bytes[4..].copy_from_slice(&nonce.to_le_bytes());
    bytes
}

/// Encrypt `data` in place and return the detached Poly1305 tag.
pub fn seal(key: &[u8; KEY_SIZE], nonce: u64, aad: &[u8], data: &mut [u8]) -> Option<Tag> {
    let nonce = nonce_bytes(nonce);
    xor_stream(key, &nonce, 1, data);
    Some(authentication_tag(key, &nonce, aad, data))
}

/// Verify the tag and decrypt `data` in place.
pub fn open(key: &[u8; KEY_SIZE], nonce: u64, aad: &[u8], data: &mut [u8], tag: &Tag) -> bool {
    let nonce = nonce_bytes(nonce);
    let expected = authentication_tag(key, &nonce, aad, data);
    if !constant_time_eq(&expected, tag) {
        return false;
    }
    xor_stream(key, &nonce, 1, data);
    true
}

/// Provision a key into the bounded kernel vault for an owning identity.
pub fn provision_for(owner: u64, key: [u8; KEY_SIZE]) -> Option<KeyHandle> {
    KEY_VAULT.lock().provision(owner, key)
}

/// Kernel-owned compatibility form for internal volume services.
pub fn provision(key: [u8; KEY_SIZE]) -> Option<KeyHandle> {
    provision_for(0, key)
}

/// Revoke a key and erase its slot. Old handles cannot address a replacement.
pub fn revoke_for(owner: u64, handle: KeyHandle) -> bool {
    KEY_VAULT.lock().revoke(owner, handle)
}

pub fn revoke(handle: KeyHandle) -> bool {
    revoke_for(0, handle)
}

pub fn seal_with_handle(handle: KeyHandle, nonce: u64, aad: &[u8], data: &mut [u8]) -> Option<Tag> {
    let key = KEY_VAULT.lock().lookup(0, handle)?;
    seal(&key, nonce, aad, data)
}

pub fn seal_with_owner(
    owner: u64,
    handle: KeyHandle,
    nonce: u64,
    aad: &[u8],
    data: &mut [u8],
) -> Option<Tag> {
    let key = KEY_VAULT.lock().lookup(owner, handle)?;
    seal(&key, nonce, aad, data)
}

pub fn open_with_handle(
    handle: KeyHandle,
    nonce: u64,
    aad: &[u8],
    data: &mut [u8],
    tag: &Tag,
) -> bool {
    let Some(key) = KEY_VAULT.lock().lookup(0, handle) else {
        return false;
    };
    open(&key, nonce, aad, data, tag)
}

pub fn open_with_owner(
    owner: u64,
    handle: KeyHandle,
    nonce: u64,
    aad: &[u8],
    data: &mut [u8],
    tag: &Tag,
) -> bool {
    let Some(key) = KEY_VAULT.lock().lookup(owner, handle) else {
        return false;
    };
    open(&key, nonce, aad, data, tag)
}

#[cfg(feature = "stage12-3-test")]
pub fn structural_self_test() -> bool {
    // RFC 8439 section 2.8.2 tag vector (also catches an accidental
    // round-trip-only implementation).
    let rfc_key = [
        0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d, 0x8e,
        0x8f, 0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0x9b, 0x9c, 0x9d,
        0x9e, 0x9f,
    ];
    let rfc_nonce = [
        0x07, 0x00, 0x00, 0x00, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47,
    ];
    let rfc_aad = [
        0x50, 0x51, 0x52, 0x53, 0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7,
    ];
    let mut rfc_data = *b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
    xor_stream(&rfc_key, &rfc_nonce, 1, &mut rfc_data);
    let rfc_tag = authentication_tag(&rfc_key, &rfc_nonce, &rfc_aad, &rfc_data);
    if rfc_tag
        != [
            0x1a, 0xe1, 0x0b, 0x59, 0x4f, 0x09, 0xe2, 0x6a, 0x7e, 0x90, 0x2e, 0xcb, 0xd0, 0x60,
            0x06, 0x91,
        ]
    {
        return false;
    }

    let key = [0x42u8; KEY_SIZE];
    let mut data = *b"volume metadata and payload";
    let original = data;
    let Some(tag) = seal(&key, 9, b"header", &mut data) else {
        return false;
    };
    if data == original {
        return false;
    }
    let mut wrong_tag = tag;
    wrong_tag[0] ^= 1;
    if open(&key, 9, b"header", &mut data, &wrong_tag) {
        return false;
    }
    if open(&key, 9, b"wrong", &mut data, &tag) {
        return false;
    }
    if !open(&key, 9, b"header", &mut data, &tag) || data != original {
        return false;
    }

    let Some(handle) = provision_for(1000, key) else {
        return false;
    };
    let mut via_vault = original;
    let Some(vault_tag) = seal_with_owner(1000, handle, 10, b"header", &mut via_vault) else {
        return false;
    };
    if open_with_owner(999, handle, 10, b"header", &mut via_vault, &vault_tag) {
        return false;
    }
    if !open_with_owner(1000, handle, 10, b"header", &mut via_vault, &vault_tag)
        || via_vault != original
    {
        return false;
    }
    if !revoke_for(1000, handle)
        || open_with_owner(1000, handle, 10, b"header", &mut via_vault, &vault_tag)
    {
        return false;
    }
    true
}

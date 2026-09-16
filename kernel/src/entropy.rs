//! Minimal kernel entropy source.
//!
//! Security-sensitive subsystems (initial TCP sequence numbers, and later
//! ASLR / stack-canary values) must not be seeded from a fixed compile-time
//! constant: a static seed makes the corresponding sequence numbers, and
//! therefore any values derived from them, predictable to a network
//! attacker. Before this module existed, `network.rs` seeded smoltcp's
//! `Config::random_seed` with a single hardcoded `u64` literal.
//!
//! `random_u64()` prefers the CPU's hardware RDRAND instruction (already
//! detected at boot in `hal::cpu::detect_features` but previously unused
//! anywhere in the kernel). RDRAND is not guaranteed to be present on every
//! target (older QEMU CPU models, some real hardware), and the instruction
//! itself is documented by Intel/AMD as occasionally failing to produce a
//! value under heavy load, so callers get a graceful, always-available
//! fallback rather than a panic or an `Option`.
//!
//! The fallback mixes the monotonic timer tick count with the memory
//! address of a freshly stack-allocated value (ASLR-relevant only in that
//! kernel stack placement is not attacker-controlled) through a SplitMix64
//! step. This is **not** cryptographically secure — it is a best-effort
//! improvement over a fixed constant for platforms without RDRAND, not a
//! substitute for real hardware entropy. Do not use this fallback path for
//! anything that needs cryptographic unpredictability (key material,
//! nonces); it exists only to avoid a fully static seed.

use x86_64::instructions::random::RdRand;

/// Returns a best-effort random `u64`.
///
/// Tries hardware RDRAND first (a handful of attempts, since RDRAND may
/// transiently fail to produce a value per the ISA documentation). Falls
/// back to a SplitMix64 mix of the boot tick counter and a stack address
/// if RDRAND is unavailable or exhausted its retries.
pub fn random_u64() -> u64 {
    if let Some(rdrand) = RdRand::new() {
        for _ in 0..8 {
            if let Some(value) = rdrand.get_u64() {
                return value;
            }
        }
    }
    fallback_u64()
}

/// SplitMix64-style fallback mix. Not cryptographically secure — see the
/// module-level documentation.
fn fallback_u64() -> u64 {
    let stack_marker: u8 = 0;
    let stack_addr = &stack_marker as *const u8 as u64;
    let mut z = crate::timer::ticks().wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ stack_addr;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

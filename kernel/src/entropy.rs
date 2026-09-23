//! WovenHat Stage 13.9R — split secure entropy from best-effort kernel randomness.
//!
//! The secure-pool prototype is compiled with its Wi-Fi stage consumers only.
//! It has no production entropy source yet.
//!
//! Security-sensitive callers use `fill_secure` / `snonce`. Those APIs fail
//! closed unless a reviewed secure source has seeded the pool.
//!
//! Non-cryptographic callers such as smoltcp may use `random_u64` /
//! `random_range`. Those prefer RDRAND and otherwise fall back to a documented
//! best-effort timer/stack-address mixer. That fallback MUST NOT be used for
//! WPA2 nonces, keys, ASLR secrets, stack canaries, or other cryptographic
//! material.

#[cfg(feature = "stage13-9-test")]
use spin::Mutex;
use x86_64::instructions::random::RdRand;
#[cfg(feature = "stage13-9-test")]
use zeroize::Zeroize;

#[cfg(feature = "stage13-9-test")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntropyError {
    Unavailable,
    InvalidRequest,
}

#[cfg(feature = "stage13-9-test")]
struct Pool {
    seeded: bool,
    key: [u8; 32],
    counter: u64,
}

#[cfg(feature = "stage13-9-test")]
impl Pool {
    const fn new() -> Self {
        Self {
            seeded: false,
            key: [0; 32],
            counter: 0,
        }
    }

    fn clear(&mut self) {
        self.key.zeroize();
        self.counter = 0;
        self.seeded = false;
    }

    fn seed(&mut self, seed: [u8; 32]) {
        self.clear();
        self.key = seed;
        self.counter = 1;
        self.seeded = true;
    }

    fn fill(&mut self, out: &mut [u8]) -> Result<(), EntropyError> {
        if out.is_empty() {
            return Err(EntropyError::InvalidRequest);
        }
        if !self.seeded {
            return Err(EntropyError::Unavailable);
        }

        #[cfg(feature = "stage13-9-test")]
        {
            let mut x =
                self.counter ^ u64::from_le_bytes(self.key[..8].try_into().unwrap_or([0; 8]));
            for (i, byte) in out.iter_mut().enumerate() {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                *byte = (x as u8) ^ self.key[i % 32];
            }
            self.counter = self.counter.wrapping_add(1);
            Ok(())
        }

        #[cfg(not(feature = "stage13-9-test"))]
        {
            let _ = out;
            Err(EntropyError::Unavailable)
        }
    }
}

#[cfg(feature = "stage13-9-test")]
static POOL: Mutex<Pool> = Mutex::new(Pool::new());

/// Cryptographic entropy boundary. Fails closed when no reviewed secure source
/// has seeded the pool.
#[cfg(feature = "stage13-9-test")]
pub fn fill_secure(out: &mut [u8]) -> Result<(), EntropyError> {
    POOL.lock().fill(out)
}

/// Produce a WPA2 SNonce only from the secure entropy boundary.
#[cfg(feature = "stage13-9-test")]
pub fn snonce() -> Result<[u8; 32], EntropyError> {
    let mut nonce = [0u8; 32];
    fill_secure(&mut nonce)?;
    Ok(nonce)
}

/// Secure u64 helper for callers that explicitly require cryptographic entropy.
#[cfg(feature = "stage13-9-test")]
pub fn try_secure_u64() -> Result<u64, EntropyError> {
    let mut bytes = [0u8; 8];
    fill_secure(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

/// Best-effort, non-cryptographic random value.
///
/// Prefer hardware RDRAND. If unavailable or transiently failing, use the
/// historical WovenHat SplitMix64-style timer/stack mixer. This keeps network
/// stack randomized choices from collapsing to a fixed constant, but the
/// fallback is not suitable for secrets or WPA2 nonces.
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

/// Best-effort value in [min, max), using rejection sampling to avoid modulo
/// bias. This inherits the non-cryptographic classification of `random_u64`.
pub fn random_range(min: u64, max: u64) -> u64 {
    if min >= max {
        return min;
    }
    let span = max - min;
    let threshold = span.wrapping_neg() % span;
    loop {
        let value = random_u64();
        if value >= threshold {
            return min + value % span;
        }
    }
}

fn fallback_u64() -> u64 {
    let stack_marker: u8 = 0;
    let stack_addr = &stack_marker as *const u8 as u64;
    let mut z = crate::timer::ticks().wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ stack_addr;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(feature = "stage13-9-test")]
pub fn available() -> bool {
    POOL.lock().seeded
}

#[cfg(feature = "stage13-9-test")]
pub fn seed_for_test(seed: [u8; 32]) {
    POOL.lock().seed(seed);
}

#[cfg(feature = "stage13-9-test")]
pub fn clear_for_test() {
    POOL.lock().clear();
}

#[cfg(feature = "stage13-9-test")]
pub fn self_test() -> bool {
    #[cfg(feature = "stage13-9-test")]
    {
        clear_for_test();

        let mut unavailable = [0u8; 32];
        if fill_secure(&mut unavailable) != Err(EntropyError::Unavailable) || available() {
            return false;
        }

        // The non-cryptographic network API remains available independently
        // of the secure pool. We do not assert unpredictability here.
        let network_a = random_u64();
        let network_b = random_u64();
        if network_a == 0 && network_b == 0 {
            return false;
        }

        seed_for_test([0xa5; 32]);
        if !available() {
            return false;
        }
        let Ok(a) = snonce() else {
            return false;
        };
        let Ok(b) = snonce() else {
            return false;
        };
        if a == [0; 32] || b == [0; 32] || a == b {
            return false;
        }

        clear_for_test();
        !available()
            && snonce() == Err(EntropyError::Unavailable)
            && try_secure_u64() == Err(EntropyError::Unavailable)
    }

    #[cfg(not(feature = "stage13-9-test"))]
    {
        !available()
    }
}

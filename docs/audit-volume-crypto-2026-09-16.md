# Stage 12.3 volume encryption — 2026-09-16

Stage 12.3 now uses an RFC 8439 ChaCha20-Poly1305 implementation in the
freestanding kernel. Encryption and decryption operate on caller-owned buffers,
produce and verify a detached 128-bit tag, authenticate associated metadata,
and compare tags in constant time. The ChaCha20 core uses bounded 32-bit
arithmetic so it builds on `x86_64-unknown-none` without architecture-specific
SIMD code-generation failures.

The bounded key vault holds eight 256-bit keys. Provisioning rejects an all-zero
key, returns a generation-safe opaque handle, and revocation erases the slot.
Stale handles cannot address a subsequently provisioned key. The structural
probe covers ciphertext mutation, tag mutation, associated-data mutation,
successful round trips, revocation, and stale-handle rejection.

Evidence retained on 2026-09-16:

- `cargo build -p wovenhat-kernel --target x86_64-unknown-none` — PASS.
- `cargo clippy -p wovenhat-kernel --target x86_64-unknown-none --features stage12-3-test -- -D warnings` — PASS.
- `python scripts/test-stage10-runtime.py --stage 12.3 --cpus 1` — PASS.
- `python scripts/test-stage10-runtime.py --stage 12.3 --cpus 2` — PASS.
- `python scripts/test-stage10-runtime.py --stage 12.3 --cpus 4` — PASS.
- Full `python scripts/test-release.py` after this implementation — PASS; all
  lint, host, 1/2/4-CPU, release, and shell/SMP gates passed.

The vault is a kernel boundary, not a complete secure-boot key ceremony. Key
provisioning from a measured hardware root, secure persistent storage, key
rotation policy, and encrypted-volume mount integration remain production
integration work.

You are working in the WovenHat OS repository, a from-scratch x86_64
kernel written in Rust (`no_std`, `no_main`), booted via `bootloader_api`,
built for target `x86_64-unknown-none` on `nightly-2026-07-10`. It has
preemptive scheduling, ring-3 userspace with an `int 0x80` syscall ABI,
FAT32/GPT storage, a capability-based security model, and a smoltcp-backed
network stack. `cargo clippy --workspace -- -D warnings` must pass and the
QEMU boot self-test suite (`main.rs`, ~31 subsystem self-tests gating
kernel continuation) must still all pass — do not weaken or delete a
self-test to make the build green.

## Milestone: Address Space Layout Randomization (ASLR) for user processes

### Why this milestone, and why now
A prior audit pass (`docs/AUDIT-2026-09-04.md`) found the kernel had
hardware RDRAND detection (`hal/cpu.rs`) but no actual entropy usage
anywhere, and fixed one instance of it: the network stack's TCP
sequence-number seed was a hardcoded constant. That fix added
`kernel/src/entropy.rs`, exposing `entropy::random_u64() -> u64` — a
best-effort random source (hardware RDRAND, falling back to a documented
non-cryptographic SplitMix64 mix if RDRAND is unavailable).

ASLR is the next natural step: the ELF loader (`elf.rs`) already validates
segment addresses and overlap carefully, and anonymous mmap (`userspace.rs`
`map_anonymous`) already places mappings at deterministic per-slot
addresses (`USER_MMAP_START + slot * USER_MMAP_STRIDE`). Both are one
"randomize the base, keep every existing bounds/overlap check" change away
from ASLR — the hard validation work is already done, only the fixed
placement is missing. Do not build SMP, networking, or filesystem features
in this milestone; those are separate, larger phases in
`docs/wovenhat-os-master-roadmap.md` and out of scope here.

### Required reading before touching anything
1. `docs/AUDIT-2026-09-04.md` — what's already fixed and why.
2. `kernel/src/entropy.rs` — the RNG you must use. Do not add a second RNG;
   if `random_u64()` isn't expressive enough (e.g. you need a bounded
   range), add a `random_range(min, max)` helper *in that file*, reusing
   `random_u64()` internally.
3. `kernel/src/elf.rs` — current loader, especially wherever
   `mapping_start` / address-limit clamping happens.
4. `kernel/src/userspace.rs` — `map_anonymous`, `USER_MMAP_START`,
   `USER_MMAP_STRIDE`, `UserStack` (including `guard_base` — the
   existing stack guard-page logic).
5. `kernel/src/paging.rs` — `map_user_range_in` and friends, so any new
   base address you compute goes through the same validated path
   (checked arithmetic, no bypassing existing overlap/permission checks).
6. `kernel/src/main.rs` — the boot self-test harness. Understand its
   pattern (each subsystem gets a self-test that must pass before boot
   continues) before adding a new one for ASLR.

### Concrete scope
1. **Entropy helper, if needed**: add `entropy::random_range(min: u64, max: u64) -> u64`
   (exclusive or inclusive — pick one, document it) if the ELF/mmap code
   needs a bounded random offset rather than a raw `u64`. Keep it in
   `entropy.rs`, not duplicated elsewhere.
2. **ELF load-base randomization**: introduce a randomized `mapping_start`
   per process load, constrained to the same valid user address range and
   alignment the current fixed-address path already enforces. Every
   existing safety check (segment overlap, address-limit clamping, W^X)
   must still run *after* the randomized base is chosen — randomize the
   input to validation, don't bypass validation.
3. **Anonymous mmap base randomization**: randomize the per-slot base
   address (or add a per-process random offset applied uniformly across
   slots — your call, document the choice) instead of the current fixed
   `USER_MMAP_START + slot * USER_MMAP_STRIDE` formula, while preserving
   the existing `MAX_ANONYMOUS_MAPPINGS` bound and the checked-arithmetic
   style already used in that function (`checked_add`, `checked_mul`).
4. **Stack base randomization**: apply the same treatment to
   `UserStack`'s placement, preserving the existing guard-page invariant
   (`guard_base` must remain correctly unmapped and immediately below the
   randomized stack base — do not weaken `UserStack`'s own invariant
   check).
5. **Determinism knob for testing**: ASLR must be disableable (e.g. a
   `qemu-test` build-time feature flag, matching the existing
   `qemu-test` feature already in `kernel/Cargo.toml`, or a boot config
   flag in `config.rs` if that fits the existing pattern better) so the
   self-test suite and CI can still assert deterministic, reproducible
   process layout when needed. Follow whichever existing config
   mechanism (`config.rs` vs Cargo feature) the codebase already uses for
   similar boot-time toggles — don't introduce a third pattern.
6. **New self-test**: add one boot-time self-test (matching the existing
   style in `main.rs`) that launches two instances of the same program and
   asserts their load addresses/stack/mmap bases differ when ASLR is
   enabled, and that all existing invariants (guard page unmapped, W^X,
   no overlap) still hold for both. This directly extends the existing
   assertion at `main.rs` around "independent user roots, unmapped stack
   guards, and RW/NX stacks verified" — don't replace that assertion,
   add to it or add a sibling test.
7. **Docs**: update `docs/security.md` to state ASLR is implemented, what
   it covers (ELF load base, mmap base, stack base) and does not cover
   (kernel addresses are unaffected — this is user-space only), and how
   to disable it for testing.

### Explicit constraints
- Do not touch scheduling (`task.rs` beyond what's strictly required to
  plumb a randomized stack/mmap base through), SMP, filesystem, or
  networking code. If you find yourself editing `interrupts.rs`, `pic.rs`,
  `ata.rs`, `fat32.rs`, `network.rs`, or `virtio_net.rs`, stop — that's out
  of scope for this milestone.
- Do not change the syscall ABI (`syscall.rs` `Number` enum values or
  argument order) — ASLR is transparent to userspace by design.
- Do not remove or weaken any existing check in `elf.rs`, `paging.rs`, or
  `userspace.rs` to make randomization "simpler." If a check makes
  randomization harder, the randomization logic has to work around it, not
  the other way around.
- Keep `entropy.rs`'s existing doc comments about the fallback path not
  being cryptographically secure — do not present the fallback as suitable
  for anything beyond ASLR/best-effort unpredictability.
- Every new/changed function needs the same level of doc comments already
  present in `elf.rs`/`userspace.rs` (this codebase documents *why*, not
  just *what* — match that).

### Definition of done
- `cargo clippy --workspace -- -D warnings` passes.
- All existing boot self-tests still pass, plus the new ASLR self-test
  from item 6.
- Two runs of the same program get different load/stack/mmap addresses
  with ASLR enabled, and identical addresses with it disabled (verifying
  the disable knob actually works, for testable CI).
- `docs/security.md` reflects the new behavior.
- No changes outside `kernel/src/entropy.rs`, `kernel/src/elf.rs`,
  `kernel/src/userspace.rs`, `kernel/src/task.rs` (plumbing only),
  `kernel/src/main.rs` (self-test addition only), `kernel/src/config.rs`
  (if used for the disable knob), and `docs/security.md`.

Work through this as a single focused PR-sized change. If you hit a design
decision not covered above (e.g. exact entropy bit-width for the load-base
offset, exact alignment granularity), make the most conservative choice
that preserves every existing invariant, state the choice and reasoning in
a comment at the decision point, and continue — don't stop to ask.

# Physical Wi-Fi integration preflight - 2026-09-24

## Result and scope

Physical integration is **not complete**. The requested real-device target
is unresolved: this Windows host reports Intel AX201 `8086:A0F0`, while the
candidate allowlist supports only AX200 `8086:2723`. No hardware allowlist
was broadened, no Windows adapter was detached, and no physical MMIO, DMA,
radio command, disk installation, or host reboot was attempted.

The firmware-container prerequisite is now corrected against a real AX200
firmware image. This is source and firmware-format validation, not a physical
firmware-load or RF result. Hardware selection and a boot/serial-log path
were requested from the user and remain prerequisites for bring-up.

## Reproduced defect and correction

The former parser rejected the published `iwlwifi-cc-a0-77.ucode` with
`TooManySections`: it had a 16-record stack table, while that container has
50 section records (48 payload sections and two separators). It also treated
TLV 32 (paging-size metadata) as a section and ignored secure SEC variants.

The new pure `kernel/src/wifi_tlv.rs` validates the complete container and
streams borrowed section records without allocation or a stack descriptor
table. `wifi_firmware.rs` re-exports the existing parser interface. The
container has a 4 MiB byte bound; hardware context-map/ring bounds remain
unchanged and are separate loader checks. Indexed access is linear, so a
loader should use `sections()` for a single pass.

Paging size must be a single four-byte value, page aligned and at most 1 MiB.
It is exposed as metadata and never counted as downloadable code. Runtime
and init secure-section TLVs are retained. CPU/paging separators retain
order and identity; the DMA stager explicitly rejects them, including
separator records containing four reserved bytes. Empty ordinary sections,
malformed headers, missing padding, invalid lengths, duplicate paging
metadata and oversized containers remain errors.

`tests/wifi_tlv.rs` adds seven host regressions for these boundaries and
forward-compatible unknown-TLV skipping. The existing QEMU firmware staging
probe now rejects both separator kinds before allocation/publication. No
existing assertion was removed or relaxed.

The broader `cargo clippy --tests -- -D warnings` additionally exposed two
existing host-test lints. `heap.rs` expresses its unchanged placement/end
predicate with `Option::filter`, valid in both crate editions. `irq_lock.rs`
moves only its test double after the production items. No suppression,
allocator policy, locking, rank, or interrupt-state change was made.

## Firmware evidence and references

Evidence directory:
`audit-artifacts/physical-wifi-preflight-20260924-160036/`.
The downloaded blob, upstream licence, source URLs, hash records, TLV
inventory, adapter inventory, original/corrected standalone Rust probes,
and initial failure logs are retained there and are not added to Git.

Firmware source: Linux-firmware tag `20250211`,
[`iwlwifi-cc-a0-77.ucode`](https://kernel.googlesource.com/pub/scm/linux/kernel/git/firmware/linux-firmware.git/+/refs/tags/20250211/iwlwifi-cc-a0-77.ucode).
Size: 1,367,692 bytes. SHA-256:
`57021bf0442f3b7d36da0592df1b91783bc17a7d1cd551c5c75300a1cc14ef8a`.
The original parser exits 1; the corrected parser exits 0 with 50 sections,
version 77, build 3020290516, and paging size 585728 bytes.

Primary format references (Linux v6.12):
[TLV identifiers/header](https://github.com/torvalds/linux/blob/v6.12/drivers/net/wireless/intel/iwlwifi/fw/file.h),
[paging limits](https://github.com/torvalds/linux/blob/v6.12/drivers/net/wireless/intel/iwlwifi/fw/img.h),
[metadata/section handling](https://github.com/torvalds/linux/blob/v6.12/drivers/net/wireless/intel/iwlwifi/iwl-drv.c),
[context-info loading](https://github.com/torvalds/linux/blob/v6.12/drivers/net/wireless/intel/iwlwifi/pcie/ctxt-info.c).
The new parser is independently implemented from these format facts; no
upstream driver implementation or firmware binary is embedded in the kernel.

## Validation

- Build, host-test Clippy and normal/feature freestanding-kernel Clippy:
  passed. The initial host-test lint failure is preserved in `host-clippy.log`.
- All 31 Rust host tests passed, including the seven new parser regressions.
- All 54 Wi-Fi markers through 13.10AC passed on 1/2/4 CPUs, exit 33.
- Corrected focused results and logs:
  `audit-artifacts/physical-wifi-preflight-20260924-160036/corrected-validation/`.
- Full `RUN-STAGE10.7.ps1` preservation passed, exit 0: nine Python tests,
  31 Rust tests, build and warning-denying Clippy, 60 memory/boot stress
  boots, three live network boots and 12 async block/file/UDP/TCP boots.
  Evidence: `audit-artifacts/acceptance-20260924-160945-430/`.
- Total for this prerequisite: 78 successful QEMU boots, including the
  three focused Wi-Fi boots. The previous continuation's wider runtime
  matrix is historical evidence and was not rerun for this parser change.

## Ownership, security and remaining integration

No new kernel object, syscall, capability, WovenGuard grant, unsafe code,
lock, IRQ handler, worker, or hardware activation is introduced. The parser
holds an immutable borrow of caller-owned firmware and does no asynchronous
work. Its bytes must remain owned/pinned before any future asynchronous
loader uses them. Parsing is not authentication, device compatibility,
firmware-command ABI selection, or proof of safe hardware execution.

The remaining physical path has material missing pieces:

1. Confirm the test device and a way to boot WovenHat and capture early serial
   output. AX201 needs its own reviewed match/configuration; do not relabel it
   AX200 or extend the allowlist without a transport implementation.
2. Supply and authenticate/version-match firmware at boot. The image builder
   currently provides no Wi-Fi firmware asset to the normal kernel.
3. Implement the real MMIO/PRPH startup transport, RX free/used/status DMA
   rings and firmware command queue. Current startup/publication trait
   implementations and RX completions are synthetic.
4. Establish one lifecycle owner before enabling device MSI. Add an
   event-driven IRQ worker, generation-safe rebind/cancel, bounded service,
   and device-specific interrupt acknowledgement. The present hard IRQ only
   sets a flag; no production worker services it.
5. Prove device/DMA quiescence before freeing context, RX/TX or paging memory,
   including fatal error, timeout, cancellation and reboot paths. Existing
   synthetic completion helpers are not such proof.
6. Integrate NVM/regulatory/channel setup, scanning, authentication,
   association and firmware TX/RX commands, plus a reviewed secure entropy
   source. `WifiSession` currently embeds `VirtualBackend`.
7. Qualify real firmware ALIVE, IRQ/RX/TX, protected network traffic,
   disconnect/reconnect and teardown on the selected hardware, retaining
   serial and host-traffic evidence alongside 1/2/4-CPU preservation.

No later roadmap stage or full physical-integration acceptance is claimed.

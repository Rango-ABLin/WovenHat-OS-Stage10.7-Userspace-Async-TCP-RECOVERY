# AX200 runtime firmware loader - 2026-09-24

## Scope and target

The user selected the current AX200 `8086:2723` driver target after the
physical preflight. The host's AX201 `8086:A0F0` remains unsupported by this
candidate. An AX200 machine and boot/serial-log route remain outstanding.
Physical Wi-Fi integration and Stage 13 production acceptance are incomplete.

This change connects the real-container parser to the existing DMA context
builder. It remains within the `stage13-9-test` candidate boundary and does
not activate a physical adapter or change ordinary VirtIO networking.

## Implementation and boundaries

`wifi_ax200_image.rs` validates the complete runtime layout before any DMA
allocation: nonempty LMAC, CPU separator, nonempty UMAC, paging separator,
then optional paging payloads with agreeing paging-metadata presence. It
rejects missing/reordered/repeated separators, oversized payloads, and total
capacity overflow. Init-image records are excluded from this runtime plan.
The existing bounds remain 64 buffers total and 32 KiB per payload; the
manifest and planner now share the constants. This candidate deliberately
rejects other layouts instead of guessing how to publish them.

The iterator preserves payload bytes/order and never yields separator data.
`Intel22000DmaContextInfo::from_runtime_firmware` builds a fresh context by
copying each payload into an owned DMA buffer. No source borrow survives
construction. Queue addresses remain unset, so the returned context is not
ready for publication. Existing synthetic per-chunk callers remain valid.

The region-order reference is Linux v6.12's
[22000 context-info loader](https://github.com/torvalds/linux/blob/v6.12/drivers/net/wireless/intel/iwlwifi/pcie/ctxt-info.c).
The Rust planner is independently implemented. Layout acceptance does not
authenticate firmware, select a command ABI, validate paging contents, or
prove device compatibility.

## Ownership and concurrency review

No new unsafe code, locks, scheduler waits, IRQ operations, capabilities or
userspace pointers are introduced. The caller immutably owns the source for
the duration of this synchronous constructor. Layout errors happen before
allocation. Allocation/copy failures unwind the local unpublished context
through existing buffer destructors; this path has no device-owned memory.
The constructor publishes no physical address and registers no asynchronous
work, so cancellation, generation reuse and cross-CPU completion cannot race
with it. Existing allocation locks are acquired and released by their current
APIs; no outer IRQ lock is added.

Successful pre-publication drop is tested against physical-frame counts.
Allocator failure midway through this constructor is not fault-injected;
its cleanup relies on the existing RAII buffer ownership. Once future code
publishes a context to real hardware, it must retain both firmware and paging
buffers until the appropriate device quiescence guarantee. This constructor
does not establish that guarantee or make existing publication helpers safe
for an unmanaged real-device lifecycle.

## Validation

Evidence: `audit-artifacts/ax200-runtime-loader-20260924-163612/`.
The real `iwlwifi-cc-a0-77.ucode` from the previous preflight produces 14 LMAC,
15 UMAC and 19 paging payloads. All 48 match source bytes exactly; both
separator records are excluded. The previous audit retains its source,
licence and SHA-256.

Four new host tests exercise routing, malformed ordering, DMA limits and
paging-metadata presence. The Stage 13.10Q QEMU probe additionally constructs
the runtime context from a small TLV image, overwrites the source, verifies
the independent copies and three allocated frames, then verifies all three
frames are reclaimed on unpublished drop. All prior probe assertions remain.

- Build, warning-denying host-test Clippy, normal freestanding-kernel Clippy
  and Wi-Fi feature Clippy passed.
- All 35 Rust host tests passed (24 existing non-parser tests and 11 parser/
  layout tests).
- All 54 Wi-Fi markers through Stage 13.10AC passed on 1/2/4 CPUs, exit 33.
  This includes the strengthened 13.10Q copy/reclamation probe.
- The real-image probe passed with 48 payloads, as described above.
- The preceding change's full Stage 10.7 gate remains preserved in
  `audit-artifacts/acceptance-20260924-160945-430/`. That 75-boot run is
  prior evidence, not a new run for this increment. This increment changes
  only the feature-gated Wi-Fi candidate, its host tests and documentation;
  it adds three focused QEMU boots and no full-stage acceptance claim.

## Remaining physical integration

Firmware boot delivery/trust and ABI selection, real MMIO/PRPH startup,
RX and command queues, a production IRQ worker/lifecycle owner, safe DMA
quiescence, NVM/regulatory setup, secure entropy, and actual association/
traffic tests remain open as recorded in the
[physical preflight audit](audit-physical-wifi-preflight-2026-09-24.md).
This is a tested source-integration increment, not full physical acceptance.

# Stage 13.10AC continuation audit — 2026-09-23

Baseline: `0563d0bb21700f8f9104ef3d97089b78a8a119ca` (clean working tree).
No retained acceptance artifacts were present at the start of this checkout.

## Final result on 2026-09-24

Preservation and bounded candidate acceptance passed: 144 QEMU boots,
24 Rust host tests, nine Python harness tests, ordinary build, normal release
build, host/freestanding-kernel Clippy, and per-feature freestanding Clippy.
This is a validated continuation, not production completion of Stage 13.

| Evidence | Result |
| --- | --- |
| `audit-artifacts/acceptance-20260924-021102-008/acceptance-output.txt` | Full Stage 10.7 gate: PASS, 75 boots, exit 0 |
| `audit-artifacts/continuation-matrix-20260924-114937/results.json` | 54 runtime boots through 13.5 and corresponding lint passed; original 13.6 lint failure retained |
| `audit-artifacts/continuation-matrix-20260924-120822/results.json` | Corrected 13.6 through 13.10AC: 12 runtime boots and lint passed; 2 hotplug boots passed; interrupted release-build attempt retained |
| `audit-artifacts/release-shell-20260924-155123/` | Normal release shell, PS/2 IOAPIC and SMP smoke: PASS; serial/QEMU logs and exit 0 retained |

The only kernel change after the full preservation pass removed the
misplaced HDA check inside `stage13-6-test`; unaffected configurations were
not redundantly rerun. The affected HID feature passed lint and 1/2/4-CPU
boots afterward, as did input, audio and Wi-Fi. Wi-Fi acceptance required all
54 markers and exit code 33 on each CPU count. Earlier failed logs were not
deleted or overwritten.

Files added: this audit and `tests/test_runtime_harness.py`. Files modified:
`main.rs` and `network.rs` for Wi-Fi feature consistency/HID error reporting;
`entropy.rs`, `hal/pci.rs`, `interrupts.rs`, `smp.rs`, `device.rs`, `shell.rs`,
and `woven_input.rs` for candidate boundaries; the Stage 13.9 launcher and
runtime/shell harnesses; architecture and status documents.

No syscall or external ABI change, new unsafe code, capability grant,
WovenGuard policy change, lock, or kernel object is introduced. Existing
ownership, generation, capacity, cancellation, teardown, local-preemption
and cross-CPU synchronization paths are preserved. Candidate input variants
have no ordinary-build producers and no exposed userspace struct ABI.

Remaining prerequisites: a production AX200 worker and lifecycle owner,
secure entropy provisioning, real firmware startup/interrupt delivery/DMA
quiescence/RF qualification, and the earlier roadmap production gaps. The
synthetic CSR test does not emulate hardware write-one-to-clear semantics.
The existing QEMU feature's broad lint allowances were not expanded; a
warning-denying invocation is not evidence that those inherited allowances
have been eliminated. No later stage is authorized by this continuation.

## Findings and changes

The Stage 13.10AC launcher delegated to the shared Stage 13.9 harness, which
required only the old 13.9H Wi-Fi marker. A successful QEMU exit with that
older marker could therefore pass without evidence of the newer tests.
The harness now explicitly requires all 54 existing Wi-Fi success markers
through 13.10AC, reports missing markers, and retains exit-code validation.
The marker list is static; it is not generated from the kernel during testing.

New host regressions exercise the actual harness entry point with mocked
build/QEMU processes and temporary serial files. They reject every individual
missing Wi-Fi marker, the old marker-only evidence set, and a bad QEMU exit.
The focused PowerShell launcher now reuses project-local Cargo directories.

The first preservation run failed host Clippy while building the kernel:
`wifi_session` was included in ordinary builds while four of its required
modules were compiled only with `stage13-9-test`. The network module also
imported an unused `Box`. Inspection found the Wi-Fi transport constructor
used only by its acceptance probe. `wifi_session`, `wifi_smol`, and the Wi-Fi
transport wrapper now share the backend's existing feature boundary. Ordinary
builds use `VirtioSmolDevice` directly; the candidate selector remains available
to its existing Wi-Fi acceptance tests. The unused import is removed.

No kernel ABI, syscall, capability, WovenGuard policy, lock, or kernel object
is added in this continuation. The architecture guide records the
feature-gated source and missing runtime-worker/lifecycle integration. The
existing synthetic CSR test cannot qualify real AX200 interrupt delivery,
firmware startup, DMA quiescence, or RF operation.

## Initial validation on 2026-09-23

The nine Python harness regressions passed, including rejection of each
missing Wi-Fi marker. The initial full preservation attempt failed at host
lint; its transcript is retained under
`audit-artifacts/acceptance-20260923-211805-671/acceptance-output.txt`.
The next preservation run passed all nine Python regressions and resolved the
import errors, but failed at host lint with 36 warning-denied unused-code
diagnostics. Evidence:
`audit-artifacts/acceptance-20260923-212851-951/acceptance-output.txt`.
The remaining errors concern candidate audio, secure entropy, PCI/MSI, IRQ,
and input APIs with no ordinary-build consumers. They are not suppressed.
The chain stopped before Rust host tests and preservation QEMU boots.

The incomplete initial focused Wi-Fi validation was recorded in
`audit-artifacts/wifi-continuation-20260923-213321-837/acceptance-output.txt`.
Full stage acceptance has not been established.
Do not treat this harness correction as completion of Stage 13.10AC or as
authorization to skip earlier unfulfilled roadmap requirements.

## Continuation on 2026-09-24

The 36 ordinary-build unused-code diagnostics were traced to candidate-only
APIs outside their callers' feature boundaries. Secure entropy/MSI candidate
modules, driver-specific PCI enablement, audio registration, and extended
input event candidates now follow those boundaries. Two unused private PCI
write wrappers were removed. Ordinary VirtIO traffic, keyboard bytes, the
reserved interrupt handler, and candidate test behavior are preserved.
No warning suppressions, capability grants, or timeout changes were added.

Host Clippy with `-D warnings` passed. The full preservation rerun is in
`audit-artifacts/acceptance-20260924-021102-008/acceptance-output.txt`;
it passed completely with exit code 0 and `STAGE 10.7 ACCEPTANCE: PASS`.
Results: build; host/kernel warning-denying lint; 24 Rust and nine Python
host tests; 60 memory/SMP boots (10/30/20 on 1/2/4 CPUs); three live network
boots; and 12 isolated async block/file/UDP/TCP boots. All 75 QEMU boots
passed. The later-stage results follow below.


The subsequent matrix passed stages 1-5, 10.8-10.9, 11.1-11.5,
12.1-12.5, and 13.1-13.5 (per-feature freestanding Clippy and 1/2/4-CPU
QEMU), then failed Stage 13.6 lint. A duplicated HDA topology check had been
inserted in the USB HID report-error branch, referencing the disabled audio
module. The misplaced duplicate was removed; the original audio topology
check remains in Stage 13.8, and HID errors still fail with their original
error report. The change is inside `stage13-6-test` and does not affect
previously tested feature configurations. Failure evidence is retained in
`audit-artifacts/continuation-matrix-20260924-114937/13.6-clippy.log`.
Validation resumed at Stage 13.6 after repeating normal kernel lint.


The resumed matrix passed Stage 13.6-13.9 Clippy and all 1/2/4-CPU boots,
including every Wi-Fi marker, followed by 2/4-CPU hotplug. Evidence is in
`audit-artifacts/continuation-matrix-20260924-120822/`. The release shell
build then blocked in the bootloader dependency's nested `cargo install`.
After identifying the exact installer PID and parent, only that installer
was stopped; no QEMU process was stopped. Its captured stderr identified
an artifact-lock wait on `target/release/.cargo-artifact-lock`, held by the
outer release build. The interrupted build is retained as a failed attempt;
no shell boot had started.

`scripts/test-shell-qemu.py` now passes the main target/build directories as
Cargo options and gives nested installer processes a separate project-local
`target/bootloader` artifact directory. The parent's explicit `--target-dir`
is necessary: `--config build.target-dir` loses to `CARGO_TARGET_DIR` and
was rejected by the first retry. Build intermediates remain project-local. This
avoids recursive release-lock acquisition without changing dependencies,
profiles, kernel code, test assertions, or timeouts. The first retry is preserved in
`audit-artifacts/release-shell-20260924-154928/`. The corrected invocation
passed in `audit-artifacts/release-shell-20260924-155123/` (exit code 0),
using cached locked dependencies with `CARGO_NET_OFFLINE=true`. It built the
normal release image and passed PS/2 IOAPIC input plus the shell SMP test.
The offline setting was local to this validation invocation, not committed
as a dependency or network-policy change.

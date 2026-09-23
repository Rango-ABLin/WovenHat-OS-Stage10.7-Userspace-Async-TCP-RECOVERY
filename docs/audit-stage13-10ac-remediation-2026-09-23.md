# Stage 13.10AC software remediation — 2026-09-23

Baseline: `0563d0b` on `stage13.9-wifi`. This report follows the
[handoff audit](audit-stage13-10ac-2026-09-23.md). The historical AX200 labels
13.10A–AC extend Wi-Fi roadmap Stage 13.9; Bluetooth Stage 13.10 is unaffected.

## Scope and implementation

The default build previously included half of several experimental subsystems.
Their compile boundaries now match their consumers: the secure-entropy
prototype and MSI programming belong to the Wi-Fi probe, non-keyboard input
variants belong to Stage 13.7, and audio registry support belongs to Stage 13.8.
The existing default VirtIO and keyboard paths remain available. PCI MMIO/DMA
enablement is compiled with its NVMe/AHCI/USB/HDA/Wi-Fi consumers. No warning
allowance, test assertion, or existing acceptance timeout was weakened.

The heap runtime-growth boot probe requires the kernel global allocator and
real paging. It is now excluded from host `cfg(test)` compilation, while all
host HeapState tests and QEMU growth assertions remain active. Host Clippy
across all targets also required an equivalent `Option::filter` formulation
of one allocation condition (compatible with kernel Rust 2021 and host Rust
2024) and relocating the host lock-order shim after non-test items.

The existing CSR controller now clears its saved RX mask on disable. Deferred
work cannot restore a disabled mask. Its self-test checks untouched idle
registers, coalescing, pending work at disable, acknowledgment writes, and both
fatal-error results.

`wifi_runtime.rs` adds an event-driven kernel worker to the opt-in Wi-Fi build.
The real vector-0xd0 handler publishes work, performs LAPIC EOI, then signals
the worker through the scheduler's event latch. The worker owns one bounded
subscription and one coalescing completion, services the existing CSR helper,
and notifies the owner after dropping the slot guard. It sleeps without any
lock held. A fatal error retires the subscription and disables the controller;
further work cannot re-enable it.

`irq_mailbox.rs` packs the active epoch and pending bit into one atomic word.
The owner allocates monotonically increasing epochs and refuses exhaustion.
Claiming consumes only that epoch's pending bit. Retirement invalidates queued
work; producers holding an old epoch cannot publish into a replacement slot.
Host tests exercise four concurrent producers racing claim, retirement, and
rebinding. This is a software subscription protocol, not a claim that an
untagged hardware MSI contains a generation.

The post-SMP QEMU probe owns a synthetic CSR page, starts the service worker,
and invokes the installed interrupt handler from a task pinned to the last
online CPU. It checks that CPU identity, receive-cause delivery, fatal-error
masking, stale epochs, bounded attachment, and owner-exit reclamation work
through real scheduler blocking and wakeup. Early synthetic tests remain.

## Ownership, security, and concurrency

The rank-20 slot lock serializes CSR access and retirement. No controller or
page reference escapes service. The backing frame is released after the slot
guard is dropped. Kernel-task exit and both normal/forced process teardown
paths call owner cleanup. Scheduler notifications occur after the slot unlock,
so scheduler rank 10 may enter cleanup without an inverse device-to-scheduler
lock path. The IRQ handler takes no device lock. IRQ mutex holders mask local
interrupts; the sole IRQ-enabled preemption mutex is the rank-10 terminal
guard, which permits the rank-10 scheduler wakeup path.

The worker and owner retain no CPU-local mutable state across blocking. The
coalescing completion retains observed status/acknowledgment bits and sticky
errors; it does not buffer packets or claim DMA completion. No syscall, user
pointer, userspace ABI, capability grant, or WovenGuard policy changed. The
only new assembly invokes an initialized kernel interrupt vector for testing.

Discovery no longer enables PCI bus mastering, and the early boot contract
probe no longer programs physical MSI before RX ownership and the service
owner exist. The explicit binding helpers remain available to the opt-in
transport. Hardware activation must follow a reviewed DMA-ready attachment.

## Acceptance tooling

The focused checker requires all 55 Wi-Fi markers, including the worker probe,
the requested CPU count, and exit 33. Missing markers and contradictory failure
logs are tested by host regressions. `RUN-STAGE13.10AC.ps1` runs a serial full
software chain with project-local Cargo directories, an OS-released lock,
unique evidence directories, source hashes, commands, return codes, and copies
of logs from legacy fixed output paths. It includes the required full Stage
10.7 launcher, release matrix, 2/4-CPU hotplug, journal, completion/timer,
process/thread/runtime, storage, and driver gates through AC. Original stage
launchers retain their existing timeouts and assertions.

The release harness now delegates host-test dependency resolution to Cargo;
it no longer guesses an rlib path under `target/debug/deps`, which is wrong
when the project-local Cargo build directory is enabled.

The release gate exposed a separate nested-Cargo deadlock: bootloader 0.11.17's
UEFI build script runs `cargo install` while inheriting the parent's target
directory. Its child waits for `target/release/.cargo-artifact-lock`, held by
the parent waiting for that child. The initial registry connection was not
the whole cause; an offline preflight reproduced the lock wait. Both attempts
were stopped only by terminating their identified installer child, allowing
the harness to record failure and retain diagnostics.

`vendor/bootloader` retains the published 0.11.17 package build inputs and
licenses. Its sole code patch assigns the nested UEFI installer separate target
and intermediate directories under `OUT_DIR`; no runtime or image-generation
code changes. The root Cargo patch selects this dependency, and the acceptance
manifest hashes its source and manifests. See its `WOVENHAT-PATCH.md` for the
upstream commit and crate checksum. The final rerun uses cached dependencies
with `CARGO_NET_OFFLINE=true`; this does not change acceptance assertions.

## Files and interface changes

Added: `kernel/src/irq_mailbox.rs`, `kernel/src/wifi_runtime.rs`,
`kernel/src/hal/pci/msi.rs`, `tests/irq_mailbox.rs`,
`tests/test_runtime_harness.py`, `scripts/test-stage13-10ac.py`,
`RUN-STAGE13.10AC.ps1`, and both AC audit documents.
Also added: the narrowly patched `vendor/bootloader` dependency with provenance.

Modified: feature-owned declarations in device, entropy, PCI, SMP, interrupt,
input, shell, and network modules; kernel initialization and task teardown;
heap host/boot configuration and lock shim placement; Wi-Fi CSR tests and
disable/discovery behavior; runtime/release harnesses, the focused launcher,
and architecture/status documentation. MSI code was moved without changing
its programming protocol. There are no new syscall numbers or capabilities.
Root Cargo configuration and lockfile select the same bootloader version with
the build-directory patch.

## Preservation failure: queued output lost at close

The subsequent complete run `ac-full-1790160662768434100` passed the 60 memory
stress boots and preservation through Stage 10.6, then failed the four-CPU
Stage 10.7 host receive with a socket timeout. Its one/two-CPU cases passed.
A focused four-CPU retry passed, which did not explain or resolve the failure.

Review found that `socket_close`, owner exit, and the last async unpin could
remove a smoltcp socket while its send buffer still held data. Completion of
the async send means the bytes were queued; it does not mean a network poll
has transmitted them. The original close-before-completion test could lose
its first payload if descriptor consumption/unpin beat network polling.

Closing entries now retain their existing bounded slot and generation until
TCP output is acknowledged (or the connection closes), or queued UDP output
is dispatched. TCP close starts only after the final async pin is released.
An unresponsive peer has a 30-second drain bound; regular polling reaps the
entry. Closing descriptors reject new operations, and all mutations remain
under the existing rank-20 runtime lock. No sleep, scheduling, new allocation,
slot expansion, or capability change was added. Socket statistics continue to
count draining entries; teardown assertions are unchanged. The probes now
wait for both requests and sockets to drain within the original 200-tick
cleanup deadline.

The Stage 10.7 build deliberately withholds polling the first queued TCP
payload until its descriptor is closed and the final pin is released. The
acceptance predicate requires observing that queued close. Thus the existing
host-verified two-connection exchange exercises the failing order on every
run, rather than relying on a lucky scheduling race. Other builds have no such
polling interlock.

## Preservation failure: USB error path referenced HDA

Run `ac-full-1790162261267779600` then passed the complete Stage 10.7 gate,
release matrix, hotplug, runtime/storage gates and drivers through Stage 13.5.
Stage 13.6 failed to compile: its HID report-error branch contained a copied
`hda::discover_topology` block, even though HDA is only enabled for Stage 13.8.
That unrelated block also existed in baseline `0563d0b`. It was removed from
the USB error branch; the original USB failure report and failure exit remain,
and the actual Stage 13.8 topology probe remains in its audio branch. The
remaining driver gates are checked before another complete final-source run.

The focused continuation passed Stage 13.6, then caught a remediation error:
the PCI memory/bus-master helper's new feature guard omitted Stage 13.7, which
also compiles xHCI. Stage 13.7 was added to that guard to match the existing
consumer; no new device authority was granted. The first failure is retained
in `ac-driver-preflight-13-7.txt`, and the corrected driver continuation uses
separate `ac-driver-final-*` logs.

Those focused logs report all Stage 13.6/13.7/13.8/13.9 CPU cases passing.
Before the final aggregate rerun, disk space fell to about 1.2 GB. Only the
two verified project-local debug `incremental` cache directories were removed,
recovering about 7 GB; historical logs and validated images were retained.
The final aggregate additionally uses `CARGO_INCREMENTAL=0` to bound compiler
cache growth. This changes build caching, not kernel behavior or test coverage.

## Results

Final software acceptance: **PASS**. Evidence:
`audit-artifacts/ac-full-1790166196507145600`, launched by
`RUN-STAGE13.10AC.ps1` with project-local Cargo directories,
`CARGO_NET_OFFLINE=true`, and `CARGO_INCREMENTAL=0`.

All 40 aggregate checks returned 0 (2,951.78 seconds of child-check runtime).
The aggregate verified its source hashes were unchanged and printed
`STAGE 13.10AC SOFTWARE ACCEPTANCE: PASS`. The matrix includes 157 QEMU boots:
75 in full Stage 10.7 preservation, 14 in the release matrix, 2 hotplug,
6 Stage 1–5/10.8, 3 Stage 10.9, 15 Stage 11, 15 Stage 12, and 27 Stage 13.
Default build, warning-denying host/kernel and feature Clippy, 27 Rust host
tests in each of the default/Wi-Fi configurations, and all 9 Python harness
tests passed. Commands, exit codes, timing, source hashes and serial snapshots
are retained alongside `results.json`.

Earlier failures remain evidence, not accepted gates: the initial all-target
Clippy diagnostics; the interrupted run in `ac-full-1790157588437175600`;
the bootloader build-lock failure in `ac-full-1790158698498588700`; the TCP
receive failure in `ac-full-1790160662768434100`; and the USB feature build
failure in `ac-full-1790162261267779600`. Focused fixes were followed by the
complete final-source aggregate above. The historical Stage 10.7 pass in
`ac-full-1790158698498588700` alone was not used to certify the final source.

## Remaining physical integration requirements

The host inventory reports Intel AX201 (`8086:a0f0`), while the reviewed device
is AX200 (`8086:2723`). No AX200 passthrough target or firmware image is supplied.
The worker probe binds owned synthetic CSR storage. Real firmware loading,
RX-ring programming/draining, ALIVE delivery from DMA, hardware MSI/W1C
behavior, and DMA visibility still require integration and hardware validation.
The default build therefore does not activate this experimental Wi-Fi driver.
Physical detach/rebind must quiesce the device and drain its interrupt vector;
software epochs alone cannot reject a late untagged MSI after vector reuse.

The next integration requires a device-specific owner for the firmware image,
staged DMA sections, context-info block, command queue, free/used RX descriptor
rings, and status page. The existing `IntelRxDmaQueue::complete_from_device`
copies synthetic bytes and marks a software slot complete; it does not read a
hardware completion ring. Likewise, the startup publication interfaces are
exercised with mock implementations. Connecting these pieces requires real
register access, descriptor-index validation, bounded draining and replenishment,
and a shutdown barrier that stops DMA before any backing allocation is freed.
ALIVE acceptance must come from a device-written notification and match the
selected firmware ABI. A simulated ALIVE is insufficient for hardware acceptance.

A read-only comparison with the
[upstream Intel firmware parser](https://github.com/torvalds/linux/blob/master/drivers/net/wireless/intel/iwlwifi/iwl-drv.c)
also identifies prerequisites for real-image loading: TLV 32 is a four-byte
paging-size declaration, while the current prototype treats it as an addressed
payload and rejects a four-byte value as empty. Secure runtime/init section
types 24/25 are not collected by the prototype. These are open firmware-loader
compatibility defects, not covered by the bounded CSR worker acceptance.
Correcting them requires real-image fixtures, separator/group handling and
loader-policy tests before any claim of firmware compatibility.

Update: the [firmware parser follow-up](audit-stage13-10ac-firmware-parser.md)
addresses those container parsing defects and adds an upstream binary fixture.
Its separate evidence applies to the follow-up source; hardware loading and
firmware ABI policy remain outstanding.

AX201 must not be enabled by adding its PCI ID to AX200 classification. Intel
documents AX201's [CNVio2 system interface](https://www.intel.com/content/www/us/en/products/sku/130293/intel-wifi-6-ax201-gig/specifications.html)
and AX200's [PCIe interface](https://www.intel.com/content/www/us/en/support/articles/000057689/wireless.html).
The chosen adapter/platform and firmware revision must be recorded before
physical bring-up. Firmware provenance is available through the
[upstream iwlwifi documentation](https://wireless.docs.kernel.org/en/latest/en/users/drivers/iwlwifi.html).

Passing this software gate closes the recorded build/test defects and validates
the bounded deferred-worker mechanism. It does not complete production Wi-Fi,
Bluetooth, or physical AX200 qualification, and does not authorize a later
roadmap stage while those requirements remain open.

# Stage 13.10AC firmware parser follow-up

Baseline: `42f5f11`, branch `stage13.9-wifi`, 2026-09-23. This closes additional
software defects found after the deferred-worker acceptance. Historical AC
remains part of Wi-Fi roadmap Stage 13.9; no later stage is started.

## Defects and changes

The original parser interpreted TLV 32 as an addressed data section, rejected
valid four-byte paging metadata, skipped secure runtime/init types 24/25, and
could expose separator words as executable payloads. Its 16-section generic
image limit also rejected the selected upstream AX200 container's 48 sections.

`kernel/src/wifi_firmware_tlv.rs` now holds the allocation-free parser used by
both kernel probes and host tests. Paging metadata has an exact four-byte
length, 4-KiB alignment and 1-MiB bound; duplicate declarations fail closed.
Secure and plain runtime/init sections share parsing. CPU and paging separators
advance independent per-image LMAC/UMAC/paging groups and are never returned as
data sections. Malformed, repeated, empty-group and dangling delimiters fail.
The reviewed delimiter profile accepts an offset-only marker or one zero word.

Whole-image size, section size, address overflow, truncated records, padding,
empty containers and section capacity are checked before exposing a result.
The Intel parser's 64-entry limit matches the existing owned DMA buffer limit;
the generic `FirmwareImage` contract remains 16. No DMA allocation limit,
hardware identity, capability, or automatic device activation is expanded.
Parsing a container is not authorization to load it: device/firmware selection,
command ABI validation and signature verification remain loader requirements.

The unchanged upstream binary in `tests/fixtures/iwlwifi` is pinned by SHA-256,
with its redistribution license and provenance. Tests inspect its documented
container structure; no instruction decoding or firmware modification occurs.
The aggregate source manifest now hashes test fixtures as well as source.

## Ownership and concurrency

The parser uses only safe Rust, fixed arrays and immutable slices borrowed from
the caller. It performs no asynchronous work, takes no locks and creates no
kernel handle or DMA object. Rust lifetimes prevent these borrowed sections
from outliving their input. A future asynchronous loader must copy them into
owned DMA storage and quiesce the device before teardown. Existing worker
epochs and shutdown limitations are unchanged.

## Evidence and limits

Seven new host tests cover the upstream image, secure/plain interleaving,
paging metadata, delimiters, capacity and input/range bounds. The kernel's
existing Stage 13.10L probe now also checks secure sections, paging metadata
and group transitions. A Python test verifies fixture size and hash.
The final-source aggregate is blocked by host memory exhaustion; this follow-up
is not full stage acceptance.

The first follow-up aggregate, `audit-artifacts/ac-full-1790188788583138000`,
passed build, strict lint, both host-test configurations and Python tests, then
failed before the first Stage 10.7 preservation guest boot. QEMU could not
allocate a 1,026,486,272-byte JIT buffer under host commit pressure (Windows
reported its paging file was too small). The complete failure is retained.
All eight QEMU test launchers now explicitly bound the TCG translation cache
at 128 MiB using `-accel tcg,tb-size=128`, as documented in
[QEMU's accelerator options](https://www.qemu.org/docs/master/system/invocation.html).
Guest RAM, SMP matrices, TCG threading defaults, workloads, assertions and
timeouts are unchanged. This changes host resource use, not the acceptance
criteria. No unrelated application was terminated or host paging setting changed.

A four-CPU memory/boot check then passed with the bounded cache. The next
aggregate, `audit-artifacts/ac-full-1790189175727349400`, again passed its first
six checks but failed while creating a nested PowerShell process (`Thread
failed to start`), before any preservation guest boot. The prior-stage chain
retained 27 nested PowerShell processes. Its calls now invoke the same child
scripts through `& "$PSScriptRoot\..."` in separate script scopes within one
process. Exceptions and native exit-code checks still propagate failure;
test bodies, stage ordering and repetition counts are unchanged. PowerShell
syntax validation passed. Both unsuccessful aggregate runs remain evidence,
not accepted gates.

The final attempt, `audit-artifacts/ac-full-1790189677915188900`, passed default
build, warning-denying host/kernel Clippy, all 34 Rust host tests in each of the
default/Wi-Fi configurations, and all 10 Python tests. Its Stage 10.7 chain
reached the first memory boot in one PowerShell process, but QEMU failed with
`cannot set up guest memory 'pc.ram'`. The guest never started. This failure
propagated correctly through the revised chain. All three aggregate attempts
stopped at the failed gate; no later-stage pass is inferred.

Focused Wi-Fi acceptance passed on 1/2/4 CPUs before the launcher resource
changes (`audit-artifacts/firmware-parser/focused-1.txt`), and the four-CPU
memory/boot suite passed after the TCG cache bound was applied. These focused
passes are not a substitute for the blocked final aggregate. Kernel/parser
source was unchanged between that focused run and the final aggregate.

Resume prerequisite: free host memory/commit capacity, then rerun
`powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\RUN-STAGE13.10AC.ps1`
as the only acceptance chain, using the project-local Cargo directories.
Do not shrink guest RAM, change assertions or increase timeouts to work around
the host shortage. Physical AX200 integration and qualification remain separate
requirements even after the software aggregate succeeds.

This is container-parser acceptance only. Physical RX descriptor integration,
device-written ALIVE, firmware ABI policy, DMA shutdown and actual AX200 MSI/DMA
qualification remain open. The available host is AX201; adding its PCI ID would
not establish AX201 support. No successful physical firmware boot is claimed.

## Format references

Reviewed against Linux v6.12's documented Intel container interface:

- [TLV definitions](https://github.com/torvalds/linux/blob/v6.12/drivers/net/wireless/intel/iwlwifi/fw/file.h)
- [Paging and secure-section parsing](https://github.com/torvalds/linux/blob/v6.12/drivers/net/wireless/intel/iwlwifi/iwl-drv.c)
- [Paging limits](https://github.com/torvalds/linux/blob/v6.12/drivers/net/wireless/intel/iwlwifi/fw/img.h)
- [Context-info section groups and DMA lifetime](https://github.com/torvalds/linux/blob/v6.12/drivers/net/wireless/intel/iwlwifi/pcie/ctxt-info.c)

The implementation is local safe Rust; those references establish format facts.

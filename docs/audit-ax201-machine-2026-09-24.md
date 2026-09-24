# This-machine AX201 integration audit - 2026-09-24

## Result

**Physical integration is incomplete and no native AX201 test has run.**
The latest user instruction selects the current machine and supersedes the
previous AX200 preference. This is an HP Pavilion x360 Convertible 14-dw1xxx,
Windows 11 Pro, with active AX201 PCI `8086:A0F0`, subsystem `8086:0074`,
revision `20`. Windows adapter operation is not evidence for a WovenHat driver.

The implementation is materially short of a physical driver: current startup
and completion helpers are primarily synthetic, and normal WovenHat networking
still uses VirtIO. An inventory image is not completion of that work.

## Actual host constraints

`scripts/physical-host-preflight.ps1` collected a read-only report at
`audit-artifacts/physical-host-20260924-164626-599/host.json`:

- This process is not elevated. `Confirm-SecureBootUEFI` failed for insufficient
  privileges; Secure Boot status is unknown, not disabled.
- `bcdedit /enum firmware` returned access denied, exit 1.
- `Get-VMHostAssignableDevice` returned a permission error. No assignable
  device or usable passthrough route was established.
- Only disk 0, the internal 256 GB Samsung system disk, is enumerated.
  No removable boot disk and no serial port are present in this inventory.
- The installed QEMU device list did not expose an AX201 or `vfio-pci` device.
  User-mode network connectivity through Windows cannot test a native driver.

Later inventory at `audit-artifacts/physical-host-20260924-173754-839/`
detected a Kingston DataTraveler 2.0, disk 1 / D:, 15,479,597,056 bytes.
It has one NTFS partition labelled `SSS_X64FREE_EN-US_DV9`, with Windows
installer files and additional user files. It is not blank. Details are
retained in `ax201-host-preflight-20260924-164148/usb-inventory.json`.
Writing the raw inventory image would replace that partition layout; reuse
requires an explicit decision and elevated disk access. Merely copying an
IMG file onto its current filesystem would not make this image bootable.

At that initial preflight no disk had been changed. Subsequently the user
explicitly authorized erasing this Kingston USB. A standard UAC-elevated helper
rechecked its exact UniqueId, model, size, USB bus and non-system/non-boot status,
validated the source hash, cleared only that disk and wrote 8,454,144 bytes.
Fresh-handle raw readback matched the complete source SHA-256. Windows now
reports GPT with one 8 MiB EFI system partition. This replaces its old filesystem;
it is not a secure wipe of every old data sector. The internal disk was not written.

Evidence is retained in `audit-artifacts/kingston-usb-20260924-1747/`, including
validate-only and actual-write JSON records and the exact helper. The actual write
record ended `passed`, `verified=true` at 17:47:32 local time. No kernel/source
change was required, so earlier kernel preservation results remain applicable.

The elevated query confirmed **Secure Boot enabled**. The image's embedded EFI
PE security directory is `(0, 0)`, with no Authenticode certificate. Windows C:
is 100% encrypted and BitLocker protection is on. A local operator must have
the recovery key available before a temporary Secure Boot change; do not expose
that key in chat. No BitLocker protection, firmware settings, boot entries or
Windows drivers were changed, and no reboot was initiated. Physical inventory
and the entire native AX201 checklist remain outstanding.

## Inventory image

The new independent `physical-probe` feature builds a UEFI inventory image.
It uses normal bootloader/framebuffer, ACPI, RAM/paging setup and checks,
then enumerates PCI and stops before driver activation or storage/network
I/O. It reports both to framebuffer and serial so the current lack of serial
hardware need not prevent a human from recording the result.
Serial readiness polling is bounded to 4096 reads per byte in this feature
only; a stalled/missing UART drops that diagnostic byte instead of hanging
the framebuffer report. Normal/test builds retain their existing serial path.

Reported fields include the network-controller PCI BDF, vendor/device/revision,
and, for the selected AX201, command bits, MSI/MSI-X/PCIe capability presence,
malformed-capability flag and BAR addresses. Truncated PCI inventory is
reported explicitly. The mode does not read radio CSR/PRPH registers, select
firmware, enable bus mastering/MSI, allocate device DMA, or issue radio commands.
Its `RADIO=UNIMPLEMENTED` output is intentional.

Reproduce the image and emulator smoke with:

```powershell
python scripts/test-physical-probe.py --cpus 1
python scripts/test-physical-probe.py --cpus 2
python scripts/test-physical-probe.py --cpus 4
```

Each invocation retains an independent image, SHA-256, build/QEMU/serial logs,
command record, QMP screen dump and result JSON under `audit-artifacts/`.
All drives passed to QEMU are read-only. Only that invocation's QEMU process
is terminated. The image deliberately halts and requires no keyboard driver.
Configured CPU counts do not constitute AP/SMP execution in this mode.

## Device-specific research and implementation gate

Linux v6.12's [PCI configuration table](https://github.com/torvalds/linux/blob/v6.12/drivers/net/wireless/intel/iwlwifi/pcie/drv.c)
matches `A0F0:0074` to AX201 configuration. The
[22000 configuration](https://github.com/torvalds/linux/blob/v6.12/drivers/net/wireless/intel/iwlwifi/cfg/22000.c)
distinguishes integrated transport and its crystal/LTR timing. MAC/RF type
and stepping participate in device/firmware selection. Consequently the
AX200 `cc-a0` blob validated in earlier audits is not approved for this AX201.
This change does not broaden the AX200 activation list or guess a firmware
variant from the Windows display name.

## Complete outstanding acceptance checklist

Every item below remains open. Source scaffolding and emulator passes must
not mark these complete:

1. **Native boot and evidence:** establish a boot medium, firmware boot route,
   screen or serial capture and a return-to-Windows procedure. Collect this
   machine's WovenHat PCI inventory; then qualify normal boot on this board.
2. **AX201 identity/configuration:** read the required MAC/RF/stepping values
   through a reviewed MMIO mapping and device-access sequence; match integrated
   transport, power/reset timing and firmware command ABI.
3. **Firmware provisioning:** supply the selected, version-matched image at
   boot with integrity/trust policy and licence; validate and own it for all
   asynchronous use. Prove firmware ALIVE on this device.
4. **Physical queues and startup:** implement actual PRPH/CSR transport, RX
   free/used/status DMA rings and firmware command TX/completion queues,
   including address/alignment/cache constraints and bounded capacities.
5. **IRQ/lifecycle ownership:** one production owner, event-driven worker,
   device-specific interrupt acknowledgement, wakeup ordering, timeout/error
   recovery, generation-safe reuse, cancellation, local preemption and
   cross-CPU locking review. No blocking under IRQ locks.
6. **DMA teardown:** prove device quiescence before freeing every context,
   firmware/paging and RX/TX buffer on success, failure, cancellation, reset,
   disconnect, removal and reboot. Synthetic completion is not that proof.
7. **Radio configuration:** NVM/calibration, regulatory country/channel policy,
   antenna/PHY setup, scan commands/results and actual auth/association.
8. **Security and packet path:** reviewed secure entropy, real firmware/backend
   integration replacing the virtual-only session, WPA2/key installation and
   protected TX/RX with replay/rekey/disconnect behavior preserved.
9. **End-to-end physical acceptance:** host-verified DHCP/DNS/UDP/TCP traffic,
   reconnect/rekey, cancellation, teardown, resource return, error recovery
   and relevant 1/2/4-CPU preservation with retained evidence. Then update the
   stage gate and commit real acceptance separately.

## Ownership and validation

The inventory mode introduces no kernel object or asynchronous operation,
no new unsafe code, and no new capability grant. It copies existing PCI
inventory records, prints outside the inventory lock and halts. Only existing
RAM/paging/PCI-read APIs run; no source buffer is handed to hardware. There
is no new cancellation/generation/owner-teardown path.

The initial diagnostic Clippy run found three locals whose uses were after
the early halt. The branch was moved after the existing RAM/paging checks,
preserving their original uses and avoiding warning suppression.

The first full preservation attempt failed on the final 4-CPU TCP test.
Diagnosis reproduced queued-data loss when close/unpin removed a live TCP
transport. The repair and all original failure evidence are documented in
[the separate TCP close/drain audit](audit-tcp-close-drain-2026-09-24.md).
No assertion or existing test timeout was weakened.

- Corrected full Stage 10.7 gate: passed, exit 0, 75 QEMU boots, 35 Rust
  tests, nine Python harness tests, build and warning-denying Clippy.
  Evidence: `audit-artifacts/acceptance-20260924-171843-691/`.
- Final host-test/physical-probe/10.8/10.9/13.9 Clippy: passed.
- Related 10.8/10.9/13.9 QEMU: passed on 1/2/4 CPUs, including all 54 Wi-Fi
  candidate markers. Ordinary release shell/PS2/IOAPIC/SMP smoke: passed.
- Final inventory-image smoke: passed with 1/2/4 CPUs configured. These are
  pre-AP inventory boots, not SMP or physical Wi-Fi acceptance.
- No-UART QEMU boot: passed; the final framebuffer bytes exactly matched
  the serial-enabled reference. This checks emulated UART absence, not a
  real physical UART or every possible stuck-register behavior.
- First corrected TCP boot and ten further 4-CPU repeats: passed, including
  queued-close cases; original failure evidence remains intact.

Final focused results, no-UART records and retained release logs are under
`audit-artifacts/ax201-host-preflight-20260924-164148/`.

The verified image bundle is `target/physical-test-ax201-20260924/`.
Its image SHA-256 is
`c635d322fc53cbbdef92f2c2d21ed4432acd04ef19d276d125e3fe2dc8bf488a`.
It was copied from `audit-artifacts/physical-probe-1cpu-1790264108463067600/`
and the copied hash was rechecked. Its screen was visually inspected; it
clearly reports inventory-only status and unimplemented radio support.

No physical result or full-stage acceptance is claimed by this audit.

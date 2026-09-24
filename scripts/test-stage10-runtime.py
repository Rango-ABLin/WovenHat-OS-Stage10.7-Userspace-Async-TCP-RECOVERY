"""Run isolated completion-port/timer/runtime acceptance with durable logs."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

# Explicit acceptance contract: do not derive these from the kernel at runtime.
WIFI_REQUIRED_MARKERS = (
    '[S13.9A] WovenWiFi framework + PCI classification: PASSED',
    '[S13.9B] IEEE 802.11 frame/IE core: PASSED',
    '[S13.9C] beacon/probe scan pipeline: PASSED',
    '[S13.9D] Open System authentication + association: PASSED',
    '[S13.9E] RSN + EAPOL-Key protocol foundation: PASSED',
    '[S13.9F] WPA2 cryptographic foundation: PASSED',
    '[S13.9G] WPA2 4-way handshake integration: PASSED',
    '[S13.9H] GTK + encrypted key data: PASSED',
    '[S13.9I] CCMP protected data path: PASSED',
    '[S13.9J] WovenWiFi <-> WovenNet integration: PASSED',
    '[S13.9X] GTK/group-addressed CCMP data path: PASSED',
    '[S13.9Y] WPA2 reconnect/rekey lifecycle: PASSED',
    '[S13.9Z] WPA2 live group-key rekey: PASSED',
    '[S13.10A] physical PCI Wi-Fi backend boundary: PASSED',
    '[S13.10B] Wi-Fi MMIO/DMA/interrupt scaffolding: PASSED',
    '[S13.10C] Wi-Fi real MMIO + DMA memory ownership: PASSED',
    '[S13.10D] Wi-Fi DMA descriptor ownership + queue lifecycle: PASSED',
    '[S13.10E] Wi-Fi supported chipset binding boundary: PASSED',
    '[S13.10F] Wi-Fi DMA buffer ownership binding: PASSED',
    '[S13.10G] Wi-Fi activation authority hardening: PASSED',
    '[S13.10H] Intel AX200 CSR boundary: PASSED',
    '[S13.10I] Intel reset/readiness state machine: PASSED',
    '[S13.10J] Intel MAC access/device initialization: PASSED',
    '[S13.10K] Wi-Fi firmware validation/lifecycle foundation: PASSED',
    '[S13.10L] Intel TLV firmware parser: PASSED',
    '[S13.10M] Intel firmware DMA staging/transfer boundary: PASSED',
    '[S13.10N] Intel firmware transfer executor boundary: PASSED',
    '[S13.10O] Intel 22000 firmware transport contract: PASSED',
    '[S13.10P] AX200 context-info self-load manifest: PASSED',
    '[S13.10Q] AX200 DMA-backed context-info construction: PASSED',
    '[S13.10R] AX200 context-info ABI + publication: PASSED',
    '[S13.10S] AX200 CPU_INIT_RUN startup sequencing: PASSED',
    '[S13.10T] AX200 ALIVE notification validation: PASSED',
    '[S13.10U] contiguous firmware DMA ownership: PASSED',
    '[S13.10V] version-aware AX200 ALIVE ABI: PASSED',
    '[S13.10W] Intel RX notification delivery: PASSED',
    '[S13.10X] Intel RX DMA completion boundary: PASSED',
    '[S13.10Y] Intel RX interrupt service boundary: PASSED',
    '[S13.10Z] PCI device interrupt foundation: PASSED',
    '[S13.10AA] PCI MSI programming contract: PASSED',
    '[S13.10AB] AX200 PCI MSI binding contract: PASSED',
    '[S13.10AC] deferred AX200 IRQ -> CSR RX service: PASSED',
    '[S13.9K] Wi-Fi transport/backend contract: PASSED',
    '[S13.9L] reconnect/timeout/lifecycle hardening: PASSED',
    '[S13.9M] cross-module integration closure: PASSED',
    '[S13.9N] WPA2 security lifecycle integration: PASSED',
    '[S13.9O] kernel entropy/secure SNonce boundary: PASSED',
    '[S13.9P] live WPA2 GTK/KRACK integration: PASSED',
    '[S13.9Q] WPA2 association + RSN integration: PASSED',
    '[S13.9R] entropy/network randomness split: PASSED',
    '[S13.9S] WPA2 supplicant/session key handoff: PASSED',
    '[S13.9T] backend RX/CCMP/Ethernet integration: PASSED',
    '[S13.9U] WovenWiFi/smoltcp transport adapter: PASSED',
    '[S13.9V] selectable WovenNet transport integration: PASSED',
)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--stage', choices=('10.8', '10.9', '11.1', '11.2', '11.3', '11.4', '11.5', '12.1', '12.2', '12.3', '12.4', '12.5', '13.1', '13.2', '13.3', '13.4', '13.5', '13.6', '13.7', '13.8', '13.9', '1-5'), default='10.8')
    parser.add_argument('--cpus', type=int, choices=(1, 2, 4), default=1)
    parser.add_argument('--qemu', default=shutil.which('qemu-system-x86_64') or r'C:\Program Files\qemu\qemu-system-x86_64.exe')
    parser.add_argument('--firmware', type=Path)
    parser.add_argument('--timeout', type=float, default=180)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    qemu = Path(args.qemu)
    firmware = args.firmware or qemu.parent / 'share' / 'edk2-x86_64-code.fd'
    if not qemu.is_file() or not firmware.is_file():
        parser.error('QEMU and firmware files must exist')
    out = root / 'audit-artifacts' / f'stage{args.stage}-{args.cpus}cpu-{time.time_ns()}'
    out.mkdir(parents=True)
    build = subprocess.run(['cargo', 'run', '--quiet', '--features', f'stage{args.stage.replace(".", "-")}-test', '--', '--print-image'],
                           cwd=root, text=True, capture_output=True)
    (out / 'build.log').write_text(build.stdout + build.stderr, encoding='utf-8')
    if build.returncode:
        print(build.stderr, file=sys.stderr)
        return 1
    serial = out / 'serial.log'
    command = [str(qemu), '-machine', 'q35', '-m', '256M', '-smp', str(args.cpus),
               '-display', 'none', '-serial', f'file:{serial}', '-no-reboot',
               '-device', 'isa-debug-exit,iobase=0xf4,iosize=0x04',
               '-drive', f'if=pflash,format=raw,readonly=on,file={firmware}',
               '-drive', f'if=none,id=boot,format=raw,readonly=on,file={build.stdout.strip()}',
               '-device', 'virtio-blk-pci,drive=boot,bootindex=1']
    if args.stage in ('13.5', '13.6'):
        command.extend(['-device', 'qemu-xhci,id=xhci',
                        '-device', 'usb-kbd,bus=xhci.0'])
    if args.stage == '13.8':
        command.extend(['-audiodev', 'none,id=wh_audio', '-device', 'intel-hda,id=hda', '-device', 'hda-duplex,bus=hda.0,audiodev=wh_audio'])
    if args.stage == '13.4':
        ahci_image = out / 'ahci.img'
        with ahci_image.open('wb') as image:
            image.truncate(16 * 1024 * 1024)
        command.extend(['-drive', f'if=none,id=sata0,format=raw,file={ahci_image}',
                        '-device', 'ich9-ahci,id=ahci',
                        '-device', 'ide-hd,drive=sata0,bus=ahci.0'])
    if args.stage == '13.3':
        nvme_image = out / 'nvme.img'
        with nvme_image.open('wb') as image:
            image.truncate(16 * 1024 * 1024)
        command.extend(['-drive', f'if=none,id=nvme0,format=raw,file={nvme_image}',
                        '-device', 'nvme,drive=nvme0,serial=WOVENHAT133'])
    flags = subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0
    with (out / 'qemu.log').open('w') as output:
        try:
            result = subprocess.run(command, cwd=root, stdout=output, stderr=output,
                                    timeout=args.timeout, creationflags=flags)
        except subprocess.TimeoutExpired:
            print('QEMU timeout; evidence:', out, file=sys.stderr)
            return 1
    log = serial.read_text(errors='replace') if serial.exists() else ''
    required = [f'[SMP] online={args.cpus} expected={args.cpus}',
                '[S10.3] userspace async completion ABI + cancellation/teardown: PASSED',
                ({'10.8': '[S10.8] completion ports + batch/cancel/timeout/teardown/SMP: PASSED',
                  '10.9': '[S10.9] timers/events/deadlines/cancellation/teardown: PASSED',
                  '11.1': '[S11.1] production process model: PASSED',
                  '11.2': '[S11.2] threads/TLS/join: PASSED',
                  '11.3': '[S11.3] notifications: PASSED',
                  '11.4': '[S11.4] libwoven runtime boundary: PASSED',
                  '11.5': '[S11.5] loader hardening: PASSED',
                  '12.1': '[S12.1] VFS boundary: PASSED', '12.2': '[S12.2] WovenFS: PASSED',
                  '12.3': '[S12.3] encryption: PASSED', '12.4': '[S12.4] snapshots: PASSED',
                  '12.5': '[S12.5] storage management: PASSED', '13.1': '[S13.1] driver framework: PASSED',
                  '13.2': '[S13.2] PCI/PCIe configuration + inventory: PASSED',
                  '13.3': '[S13.3] NVMe controller/queue foundation: PASSED',
                  '13.4': '[S13.4] AHCI/SATA DMA block I/O: PASSED',
                  '13.5': '[S13.5] xHCI USB core: PASSED',
                  '13.6': '[S13.6] USB HID keyboard: PASSED',
                  '13.7': '[S13.7] WovenInput unified event framework: PASSED',
                  '13.8': '[S13.8] WovenAudio stream/API integration: PASSED',
                  '13.9': '[S13.9H] GTK + encrypted key data: PASSED',
                  '1-5': '[S1-5] storage journal: PASSED'}[args.stage])]
    if args.stage == '13.9':
        required.extend(WIFI_REQUIRED_MARKERS)
    missing = [marker for marker in required if marker not in log]
    if result.returncode != 33 or missing:
        print('Missing required markers:', missing, file=sys.stderr)
        print(log[-12000:], file=sys.stderr)
        print('FAILED; evidence:', out, file=sys.stderr)
        return 1
    print(f'Stage {args.stage}: PASS ({args.cpus} CPU, exit 33). Evidence: {out}')
    return 0

if __name__ == '__main__':
    raise SystemExit(main())

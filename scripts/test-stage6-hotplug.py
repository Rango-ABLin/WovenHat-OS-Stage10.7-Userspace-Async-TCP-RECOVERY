"""Run the bounded Stage 6 AP-offline acceptance gate with durable logs."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default=shutil.which('qemu-system-x86_64') or r'C:\Program Files\qemu\qemu-system-x86_64.exe')
    parser.add_argument('--firmware', type=Path)
    parser.add_argument('--timeout', type=float, default=180)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    qemu = Path(args.qemu)
    firmware = args.firmware or qemu.parent / 'share' / 'edk2-x86_64-code.fd'
    if not qemu.is_file() or not firmware.is_file():
        parser.error('QEMU and firmware files must exist')
    out = root / 'audit-artifacts' / f'stage6-hotplug-2cpu-{time.time_ns()}'
    out.mkdir(parents=True)
    build = subprocess.run(['cargo', 'run', '--quiet', '--features', 'stage6-hotplug-test', '--', '--print-image'],
                           cwd=root, text=True, capture_output=True)
    (out / 'build.log').write_text(build.stdout + build.stderr, encoding='utf-8')
    if build.returncode:
        print(build.stderr, file=sys.stderr)
        return 1
    serial = out / 'serial.log'
    command = [str(qemu), '-machine', 'q35', '-m', '256M', '-smp', '2',
               '-display', 'none', '-serial', f'file:{serial}', '-no-reboot',
               '-device', 'isa-debug-exit,iobase=0xf4,iosize=0x04',
               '-drive', f'if=pflash,format=raw,readonly=on,file={firmware}',
               '-drive', f'if=none,id=boot,format=raw,readonly=on,file={build.stdout.strip()}',
               '-device', 'virtio-blk-pci,drive=boot,bootindex=1']
    flags = subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0
    with (out / 'qemu.log').open('w') as output:
        try:
            result = subprocess.run(command, cwd=root, stdout=output, stderr=output,
                                    timeout=args.timeout, creationflags=flags)
        except subprocess.TimeoutExpired:
            print('QEMU timeout; evidence:', out, file=sys.stderr)
            return 1
    log = serial.read_text(errors='replace') if serial.exists() else ''
    required = ['[SMP] online=2 expected=2',
                '[SMP] topology/NUMA affinity: PASSED',
                '[S6.HOTPLUG] offline AP: PASSED online=1 mask=0x1']
    if result.returncode != 33 or any(marker not in log for marker in required):
        print(log[-12000:], file=sys.stderr)
        print('FAILED; evidence:', out, file=sys.stderr)
        return 1
    print(f'Stage 6 hotplug: PASS (2->1 CPUs, exit 33). Evidence: {out}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

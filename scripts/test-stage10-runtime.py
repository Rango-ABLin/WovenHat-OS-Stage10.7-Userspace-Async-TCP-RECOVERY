"""Run isolated completion-port/timer/runtime acceptance with durable logs."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--stage', choices=('10.8', '10.9', '11.1', '11.2', '11.3', '11.4', '11.5', '12.1', '12.2', '12.3', '12.4', '12.5', '13.1'), default='10.8')
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
                  '12.5': '[S12.5] storage management: PASSED', '13.1': '[S13.1] driver framework: PASSED'}[args.stage])]
    if result.returncode != 33 or any(marker not in log for marker in required):
        print(log[-12000:], file=sys.stderr)
        print('FAILED; evidence:', out, file=sys.stderr)
        return 1
    print(f'Stage {args.stage}: PASS ({args.cpus} CPU, exit 33). Evidence: {out}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

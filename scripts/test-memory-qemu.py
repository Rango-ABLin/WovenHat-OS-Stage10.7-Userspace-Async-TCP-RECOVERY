"""Run the complete boot validation suite without touching user VM disks."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default=shutil.which('qemu-system-x86_64') or r'C:\Program Files\qemu\qemu-system-x86_64.exe')
    parser.add_argument('--firmware', type=Path)
    parser.add_argument('--cpus', type=int, choices=(1, 2, 4), default=2)
    parser.add_argument('--timeout', type=float, default=180)
    parser.add_argument('--release', action='store_true')
    parser.add_argument('--legacy-irq', action='store_true')
    parser.add_argument('--stage10-4', action='store_true', help='enable the isolated Stage 10.4 Ring-3 block-I/O acceptance probe')
    parser.add_argument('--stage10-5', action='store_true', help='enable the isolated Stage 10.5 Ring-3 VFS async-I/O acceptance probe')
    args = parser.parse_args()
    if args.legacy_irq and args.cpus != 1:
        parser.error('--legacy-irq requires --cpus 1')
    root = Path(__file__).resolve().parents[1]
    qemu = Path(args.qemu)
    firmware = args.firmware or qemu.parent / 'share' / 'edk2-x86_64-code.fd'
    if not qemu.is_file() or not firmware.is_file():
        parser.error('Set --qemu and --firmware to existing QEMU and OVMF files.')
    env = os.environ.copy()
    if args.stage10_4 and args.stage10_5:
        parser.error('--stage10-4 and --stage10-5 are mutually exclusive')
    feature = 'legacy-pic-test' if args.legacy_irq else ('stage10-4-test' if args.stage10_4 else ('stage10-5-test' if args.stage10_5 else 'qemu-test'))
    image = subprocess.check_output(
        ['cargo', 'run', '--quiet'] + (['--release'] if args.release else []) + ['--features', feature, '--', '--print-image'],
        cwd=root, env=env, text=True).strip()
    prefix = 'stage10-4-regression' if args.stage10_4 else ('stage10-5-regression' if args.stage10_5 else 'memory-regression')
    out = root / 'target' / f'{prefix}-{args.cpus}-{"release" if args.release else "debug"}{"-legacy" if args.legacy_irq else ""}'
    out.mkdir(parents=True, exist_ok=True)
    serial = out / 'serial.log'
    serial.write_text('')
    command = [str(qemu), '-machine', 'q35', '-m', '256M', '-smp', str(args.cpus),
               '-display', 'none', '-serial', f'file:{serial}', '-no-reboot',
               '-device', 'isa-debug-exit,iobase=0xf4,iosize=0x04',
               '-drive', f'if=pflash,format=raw,readonly=on,file={firmware}',
               '-drive', f'if=none,id=boot,format=raw,readonly=on,file={image}',
               '-device', 'virtio-blk-pci,drive=boot,bootindex=1']
    flags = subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0
    try:
        result = subprocess.run(command, cwd=root, capture_output=True, text=True,
                                timeout=args.timeout, creationflags=flags)
    except subprocess.TimeoutExpired:
        log = serial.read_text(errors='replace')
        if log:
            print(log[-12000:], file=sys.stderr)
        print('QEMU boot suite timed out. See', serial, file=sys.stderr)
        return 1
    log = serial.read_text(errors='replace')
    if args.stage10_5:
        required = [
            f'[SMP] online={args.cpus} expected={args.cpus}',
            '[S10.3] userspace async completion ABI + cancellation/teardown: PASSED',
            '[S10.5] userspace async VFS read/write + fd pinning/cancel/teardown: PASSED',
        ]
    elif args.stage10_4:
        required = [
            f'[SMP] online={args.cpus} expected={args.cpus}',
            '[S10.3] userspace async completion ABI + cancellation/teardown: PASSED',
            '[S10.4] userspace async block read/write + bounce-buffer teardown: PASSED',
        ]
    else:
        required = ['[BOOT] ALL VALIDATIONS PASSED', f'[SMP] online={args.cpus} expected={args.cpus}', '[SMP] scheduler/barrier: PASSED', '[SMP] acknowledged TLB shootdowns: PASSED', '[SMP] remote stale-translation/refree: PASSED', '[SMP] per-CPU timer preemption: PASSED', '[SMP] automatic rebalancing: PASSED', '[SMP] reschedule IPI: PASSED', '[SMP] migration stress: PASSED']
    if result.returncode != 33 or any(marker not in log for marker in required):
        print(log[-10000:], file=sys.stderr)
        print(result.stderr, file=sys.stderr)
        return 1
    print('QEMU memory/boot suite: PASS (exit 33)')
    print('Serial log:', serial)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

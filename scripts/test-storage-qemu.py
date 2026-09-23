"""Validate the complete SMP boot suite with a disposable ATA/FAT32 disk."""
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
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    qemu = Path(args.qemu)
    firmware = args.firmware or qemu.parent / 'share' / 'edk2-x86_64-code.fd'
    if not qemu.is_file() or not firmware.is_file():
        parser.error('Set --qemu and --firmware to existing QEMU and OVMF files.')
    image = subprocess.check_output(
        ['cargo', 'run', '--quiet'] + (['--release'] if args.release else []) + ['--features', 'qemu-test', '--', '--print-image'],
        cwd=root, text=True).strip()
    out = root / 'target' / f'storage-regression-{args.cpus}-{"release" if args.release else "debug"}'
    out.mkdir(parents=True, exist_ok=True)
    serial = out / 'serial.log'
    serial.write_text('')
    # Fixed, workspace-local disposable path; never use the user's runtime disk.
    disk = out / 'fat32.img'
    if disk.exists():
        disk.unlink()
    subprocess.check_call([sys.executable, str(root / 'scripts' / 'create-fat32.py'), str(disk)], cwd=root)
    command = [str(qemu), '-accel', 'tcg,tb-size=128', '-machine', 'pc', '-m', '256M', '-smp', str(args.cpus),
               '-display', 'none', '-serial', f'file:{serial}', '-no-reboot',
               '-device', 'isa-debug-exit,iobase=0xf4,iosize=0x04',
               '-drive', f'if=pflash,format=raw,readonly=on,file={firmware}',
               '-drive', f'if=none,id=boot,format=raw,readonly=on,file={image}',
               '-device', 'virtio-blk-pci,drive=boot,bootindex=1',
               '-drive', f'if=ide,index=0,format=raw,file={disk}']
    flags = subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0
    try:
        result = subprocess.run(command, cwd=root, capture_output=True, text=True,
                                timeout=args.timeout, creationflags=flags)
    except subprocess.TimeoutExpired:
        print('QEMU storage boot timed out. See', serial, file=sys.stderr)
        return 1
    log = serial.read_text(errors='replace')
    required = ['[STORAGE MUTATION] live FAT32 rename/delete/growth/lifecycle: PASSED',
                '[BOOT] ALL VALIDATIONS PASSED', f'[SMP] online={args.cpus} expected={args.cpus}',
                '[SMP] scheduler/barrier: PASSED', '[SMP] acknowledged TLB shootdowns: PASSED',
                '[SMP] remote stale-translation/refree: PASSED', '[SMP] per-CPU timer preemption: PASSED', '[SMP] automatic rebalancing: PASSED', '[SMP] reschedule IPI: PASSED', '[SMP] migration stress: PASSED']
    if result.returncode != 33 or any(marker not in log for marker in required):
        print(log[-12000:], file=sys.stderr)
        print(result.stderr, file=sys.stderr)
        return 1
    print(f'QEMU storage + complete boot suite: PASS ({args.cpus} CPUs, exit 33)')
    print('Serial log:', serial)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

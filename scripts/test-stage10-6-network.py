"""Run the isolated Stage 10.6 Ring-3 asynchronous socket-I/O acceptance boot."""
import argparse
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import time

PAYLOAD = b"WOVENHAT-NET-RX!"


def reserve_udp_port():
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


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

    build = ['cargo', 'run', '--quiet']
    if args.release:
        build.append('--release')
    build += ['--features', 'stage10-6-test', '--', '--print-image']
    image = subprocess.check_output(build, cwd=root, text=True).strip()

    mode = 'release' if args.release else 'debug'
    out = root / 'target' / f'stage10-6-regression-{args.cpus}-{mode}'
    out.mkdir(parents=True, exist_ok=True)
    serial = out / 'serial.log'
    serial.write_text('')
    qemu_log = out / 'qemu.log'
    host_port = reserve_udp_port()
    netdev = f'user,id=net0,hostfwd=udp:127.0.0.1:{host_port}-10.0.2.15:7001'
    command = [
        str(qemu), '-machine', 'q35', '-m', '256M', '-smp', str(args.cpus),
        '-display', 'none', '-serial', f'file:{serial}', '-no-reboot',
        '-device', 'isa-debug-exit,iobase=0xf4,iosize=0x04',
        '-drive', f'if=pflash,format=raw,readonly=on,file={firmware}',
        '-drive', f'if=none,id=boot,format=raw,readonly=on,file={image}',
        '-device', 'virtio-blk-pci,drive=boot,bootindex=1',
        '-device', 'virtio-net-pci,netdev=net0,disable-modern=on',
        '-netdev', netdev,
    ]
    flags = subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0
    deadline = time.monotonic() + args.timeout
    ready_marker = '[S10.6] async UDP receive target ready on port 7001'
    pass_marker = '[S10.6] userspace async UDP send/recv + socket pinning/cancel/teardown: PASSED'

    with qemu_log.open('w') as errors:
        process = subprocess.Popen(command, cwd=root, stdout=errors, stderr=errors, creationflags=flags)
        try:
            ready = False
            next_send = 0.0
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as injector:
                while time.monotonic() < deadline:
                    log = serial.read_text(errors='replace')
                    if ready_marker in log:
                        ready = True
                    if pass_marker in log:
                        break
                    if '[S10.6]' in log and ('FAILED' in log or 'TIMEOUT' in log):
                        raise RuntimeError('Kernel reported a Stage 10.6 failure')
                    if process.poll() is not None:
                        break
                    now = time.monotonic()
                    if ready and now >= next_send:
                        injector.sendto(PAYLOAD, ('127.0.0.1', host_port))
                        next_send = now + 0.05
                    time.sleep(0.02)
            remaining = max(1.0, deadline - time.monotonic())
            result = process.wait(timeout=remaining)
        except Exception as error:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            print(error, file=sys.stderr)
            print(serial.read_text(errors='replace')[-12000:], file=sys.stderr)
            print(qemu_log.read_text(errors='replace')[-4000:], file=sys.stderr)
            return 1
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)

    log = serial.read_text(errors='replace')
    required = [
        f'[SMP] online={args.cpus} expected={args.cpus}',
        '[S10.3] userspace async completion ABI + cancellation/teardown: PASSED',
        ready_marker,
        pass_marker,
    ]
    if result != 33 or any(marker not in log for marker in required):
        print(log[-12000:], file=sys.stderr)
        print(qemu_log.read_text(errors='replace')[-4000:], file=sys.stderr)
        return 1
    print('QEMU Stage 10.6 async-network suite: PASS (exit 33)')
    print('Serial log:', serial)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

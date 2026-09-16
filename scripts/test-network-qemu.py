"""Run live DHCP/DNS/ICMP/UDP/TCP regressions against QEMU user networking."""
import argparse
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import time


def reserve_tcp_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def reserve_udp_port():
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_for_marker(serial_path, marker, process, deadline, failure_markers=()):
    while time.monotonic() < deadline:
        log = serial_path.read_text(errors="replace")
        if marker in log:
            return log
        for failure in failure_markers:
            if failure in log:
                raise RuntimeError(
                    f"Kernel reported {failure!r} while waiting for {marker!r}"
                )
        if process.poll() is not None:
            raise RuntimeError(f"QEMU exited before {marker!r}")
        time.sleep(0.05)
    raise RuntimeError(f"Timed out waiting for {marker!r}")


def udp_round_trip(port):
    payload = b"wovenhat-stage5-udp"
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.settimeout(4)
        sock.sendto(payload, ("127.0.0.1", port))
        echoed, _ = sock.recvfrom(2048)
    if echoed != payload:
        raise RuntimeError(f"UDP echo mismatch: {echoed!r}")


def tcp_round_trip(port):
    payload = b"wovenhat-stage5-tcp"
    deadline = time.monotonic() + 6
    last_error = None
    while time.monotonic() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=2) as sock:
                sock.settimeout(4)
                sock.sendall(payload)
                echoed = b""
                while len(echoed) < len(payload):
                    chunk = sock.recv(len(payload) - len(echoed))
                    if not chunk:
                        break
                    echoed += chunk
                if echoed != payload:
                    raise RuntimeError(f"TCP echo mismatch: {echoed!r}")
                return
        except OSError as error:
            last_error = error
            time.sleep(0.1)
    raise RuntimeError(f"TCP host-forward connection failed: {last_error}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default=shutil.which('qemu-system-x86_64') or r'C:\Program Files\qemu\qemu-system-x86_64.exe')
    parser.add_argument('--firmware', type=Path)
    parser.add_argument('--cpus', type=int, choices=(1, 2, 4), default=4)
    parser.add_argument('--timeout', type=float, default=240)
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
    build += ['--features', 'network-test', '--', '--print-image']
    print(f"[1/6] Building WovenHat network-test image ({'release' if args.release else 'debug'})...", flush=True)
    image = subprocess.check_output(build, cwd=root, text=True).strip()
    print(f"      image: {image}", flush=True)

    mode = 'release' if args.release else 'debug'
    out = root / 'target' / f'network-regression-{args.cpus}-{mode}'
    out.mkdir(parents=True, exist_ok=True)
    serial = out / 'serial.log'
    serial.write_text('')
    tcp_port = reserve_tcp_port()
    udp_port = reserve_udp_port()

    netdev = (
        f'user,id=net0,'
        f'hostfwd=udp:127.0.0.1:{udp_port}-10.0.2.15:7000,'
        f'hostfwd=tcp:127.0.0.1:{tcp_port}-10.0.2.15:8080'
    )
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
    qemu_log = out / 'qemu.log'
    deadline = time.monotonic() + args.timeout

    print(f"[2/6] Starting QEMU with {args.cpus} CPU(s)...", flush=True)
    print(f"      UDP host:{udp_port} -> guest:7000", flush=True)
    print(f"      TCP host:{tcp_port} -> guest:8080", flush=True)

    early_failures = (
        '[NETTEST] init failed:',
        '[NETTEST] DHCP: TIMEOUT',
        '[NETTEST] DNS: START FAILED',
        '[NETTEST] DNS: FAILED',
        '[NETTEST] DNS: TIMEOUT',
        '[NETTEST] ICMP: START FAILED',
        '[NETTEST] ICMP: FAILED',
        '[NETTEST] ICMP: TIMEOUT',
        '[NETTEST] runtime regression: FAILED',
    )

    with qemu_log.open('w') as errors:
        process = subprocess.Popen(command, cwd=root, stdout=errors, stderr=errors, creationflags=flags)
        try:
            print('[3/6] Waiting for DHCP + DNS + ICMP...', flush=True)
            wait_for_marker(
                serial,
                '[NETTEST] UDP READY port=7000',
                process,
                deadline,
                early_failures,
            )
            print('      DHCP + DNS + ICMP: PASS', flush=True)

            print('[4/6] Verifying host <-> WovenHat UDP round trip...', flush=True)
            udp_round_trip(udp_port)
            wait_for_marker(
                serial,
                '[NETTEST] UDP: PASSED',
                process,
                deadline,
                ('[NETTEST] UDP: TIMEOUT', '[NETTEST] runtime regression: FAILED'),
            )
            print('      UDP: PASS', flush=True)

            print('[5/6] Verifying host <-> WovenHat TCP round trip...', flush=True)
            wait_for_marker(
                serial,
                '[NETTEST] TCP READY port=8080',
                process,
                deadline,
                ('[NETTEST] TCP: OPEN FAILED', '[NETTEST] TCP: LISTEN FAILED',
                 '[NETTEST] runtime regression: FAILED'),
            )
            tcp_round_trip(tcp_port)
            wait_for_marker(
                serial,
                '[NETTEST] TCP: PASSED',
                process,
                deadline,
                ('[NETTEST] TCP: RECEIVE FAILED', '[NETTEST] TCP: RECEIVE TIMEOUT',
                 '[NETTEST] TCP: SEND FAILED', '[NETTEST] TCP: SEND TIMEOUT',
                 '[NETTEST] runtime regression: FAILED'),
            )
            print('      TCP: PASS', flush=True)

            remaining = max(1.0, deadline - time.monotonic())
            result = process.wait(timeout=remaining)
        except Exception as error:
            print(error, file=sys.stderr)
            log = serial.read_text(errors='replace')
            print(log[-12000:], file=sys.stderr)
            return 1
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)

    print('[6/6] Verifying final SMP/TLB/release markers...', flush=True)
    log = serial.read_text(errors='replace')
    required = [
        f'[SMP] online={args.cpus} expected={args.cpus}',
        '[NETTEST] DHCP: PASSED',
        '[NETTEST] DNS: PASSED',
        '[NETTEST] ICMP: PASSED',
        '[NETTEST] UDP: PASSED',
        '[NETTEST] TCP: PASSED',
        '[NETTEST] DHCP/DNS/ICMP/UDP/TCP: PASSED',
        '[SMP] acknowledged TLB shootdowns: PASSED',
        '[BOOT] ALL VALIDATIONS PASSED',
    ]
    if result != 33 or any(marker not in log for marker in required):
        print(log[-12000:], file=sys.stderr)
        print(qemu_log.read_text(errors='replace')[-4000:], file=sys.stderr)
        return 1

    print(f'QEMU live network suite: PASS ({args.cpus} CPU, {mode}, exit 33)')
    print('DHCP + DNS + ICMP + host-verified UDP/TCP round trips passed.')
    print('Serial log:', serial)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

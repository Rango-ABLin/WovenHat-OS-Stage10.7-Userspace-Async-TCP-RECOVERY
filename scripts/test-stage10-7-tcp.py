"""Run the isolated Stage 10.7 Ring-3 asynchronous TCP acceptance boot."""
import argparse
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import threading
import time

HOST_PORT = 18080
TX = b"WOVENHAT-TCP-TX!"
RX = b"WOVENHAT-TCP-RX!"


def recv_exact(conn, count):
    data = bytearray()
    while len(data) < count:
        chunk = conn.recv(count - len(data))
        if not chunk:
            break
        data += chunk
    return bytes(data)


def serve(stop, errors, ready, finished):
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as server:
            server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            server.bind(("127.0.0.1", HOST_PORT))
            server.listen(8)
            server.settimeout(0.2)
            ready.set()
            completed = 0
            while not stop.is_set() and completed < 2:
                try:
                    conn, _ = server.accept()
                except socket.timeout:
                    continue
                with conn:
                    conn.settimeout(5)
                    print(f'Host TCP connection {completed + 1}: accepted, awaiting TX', flush=True)
                    payload = recv_exact(conn, len(TX))
                    if payload != TX:
                        raise RuntimeError(f"unexpected guest TCP payload: {payload!r}")
                    completed += 1
                    print(f'Host TCP connection {completed}: TX verified', flush=True)
                    if completed == 2:
                        conn.sendall(RX)
                        try:
                            conn.shutdown(socket.SHUT_WR)
                        except OSError:
                            pass
            if completed < 2:
                raise RuntimeError("guest did not complete two Stage 10.7 TCP connections")
            finished.set()
    except Exception as exc:
        errors.append(exc)
        stop.set()
    finally:
        ready.set()


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
    build += ['--features', 'stage10-7-test', '--', '--print-image']
    image = subprocess.check_output(build, cwd=root, text=True).strip()

    mode = 'release' if args.release else 'debug'
    out = root / 'target' / f'stage10-7-regression-{args.cpus}-{mode}'
    out.mkdir(parents=True, exist_ok=True)
    serial = out / 'serial.log'
    serial.write_text('')
    qemu_log = out / 'qemu.log'

    stop = threading.Event()
    ready = threading.Event()
    finished = threading.Event()
    server_errors = []
    thread = threading.Thread(target=serve, args=(stop, server_errors, ready, finished), daemon=True)
    thread.start()
    if not ready.wait(timeout=5) or server_errors:
        stop.set()
        thread.join(timeout=2)
        print(server_errors[0] if server_errors else 'Host TCP listener did not become ready', file=sys.stderr)
        return 1

    command = [
        str(qemu), '-accel', 'tcg,tb-size=128', '-machine', 'q35', '-m', '256M', '-smp', str(args.cpus),
        '-display', 'none', '-serial', f'file:{serial}', '-no-reboot',
        '-device', 'isa-debug-exit,iobase=0xf4,iosize=0x04',
        '-drive', f'if=pflash,format=raw,readonly=on,file={firmware}',
        '-drive', f'if=none,id=boot,format=raw,readonly=on,file={image}',
        '-device', 'virtio-blk-pci,drive=boot,bootindex=1',
        '-device', 'virtio-net-pci,netdev=net0,disable-modern=on',
        '-netdev', 'user,id=net0',
    ]
    flags = subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0
    deadline = time.monotonic() + args.timeout
    ready_marker = '[S10.7] async TCP host target 10.0.2.2:18080'
    pass_marker = '[S10.7] userspace async TCP connect/send/recv + EOF/pinning/cancel/teardown: PASSED'

    with qemu_log.open('w') as errors:
        process = subprocess.Popen(command, cwd=root, stdout=errors, stderr=errors, creationflags=flags)
        try:
            while time.monotonic() < deadline:
                log = serial.read_text(errors='replace')
                if pass_marker in log:
                    break
                if '[S10.7]' in log and ('FAILED' in log or 'TIMEOUT' in log):
                    raise RuntimeError('Kernel reported a Stage 10.7 failure')
                if server_errors:
                    raise server_errors[0]
                if process.poll() is not None:
                    break
                time.sleep(0.02)
            remaining = max(1.0, deadline - time.monotonic())
            result = process.wait(timeout=remaining)
        except Exception as error:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            print(error, file=sys.stderr)
            print(serial.read_text(errors='replace')[-14000:], file=sys.stderr)
            print(qemu_log.read_text(errors='replace')[-4000:], file=sys.stderr)
            stop.set(); thread.join(timeout=2)
            return 1
        finally:
            stop.set()
            if process.poll() is None:
                process.kill(); process.wait(timeout=5)
    thread.join(timeout=2)

    log = serial.read_text(errors='replace')
    required = [
        f'[SMP] online={args.cpus} expected={args.cpus}',
        '[S10.3] userspace async completion ABI + cancellation/teardown: PASSED',
        ready_marker,
        pass_marker,
    ]
    if result != 33 or server_errors or not finished.is_set() or thread.is_alive() or any(marker not in log for marker in required):
        if server_errors:
            print(server_errors[0], file=sys.stderr)
        print(log[-14000:], file=sys.stderr)
        print(qemu_log.read_text(errors='replace')[-4000:], file=sys.stderr)
        return 1
    print(f'QEMU Stage 10.7 async-TCP suite: PASS ({args.cpus} CPU, exit 33)')
    print('Host TCP exchange: PASS (connect + 2x TX + RX + FIN/EOF)')
    print('Serial log:', serial)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

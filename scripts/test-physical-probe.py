"""Build and smoke-test the read-only inventory image; never physical Wi-Fi acceptance."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cpus', type=int, choices=(1, 2, 4), default=1)
    parser.add_argument('--qemu', default=shutil.which('qemu-system-x86_64') or r'C:\Program Files\qemu\qemu-system-x86_64.exe')
    parser.add_argument('--firmware', type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    out = root / 'audit-artifacts' / f'physical-probe-{args.cpus}cpu-{time.time_ns()}'
    out.mkdir(parents=True)
    env = os.environ.copy()
    env.setdefault('CARGO_BUILD_BUILD_DIR', str(root / '.cargo-build'))
    env.setdefault('CARGO_TARGET_DIR', str(root / 'target'))
    with (out / 'build.log').open('w', encoding='utf-8') as log:
        result = subprocess.run(['cargo', 'run', '--quiet', '--features', 'physical-probe', '--', '--print-image'],
                                cwd=root, env=env, stdout=subprocess.PIPE, stderr=log, text=True)
    result.check_returncode()
    source = Path(result.stdout.strip())
    image = out / 'wovenhat-physical-inventory.img'
    shutil.copy2(source, image)
    digest = hashlib.sha256(image.read_bytes()).hexdigest()
    (out / 'image.sha256').write_text(f'{digest}  {image.name}\n', encoding='utf-8')
    qemu = Path(args.qemu)
    firmware = args.firmware or qemu.parent / 'share' / 'edk2-x86_64-code.fd'
    serial = out / 'serial.log'
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    command = [str(qemu), '-machine', 'q35', '-m', '256M', '-smp', str(args.cpus),
               '-display', 'none', '-serial', f'file:{serial}', '-no-reboot', '-nic', 'none',
               '-qmp', f'tcp:127.0.0.1:{port},server=on,wait=off',
               '-drive', f'if=pflash,format=raw,readonly=on,file={firmware}',
               '-drive', f'if=none,id=boot,format=raw,readonly=on,file={image}',
               '-device', 'virtio-blk-pci,drive=boot,bootindex=1']
    (out / 'command.json').write_text(json.dumps(command, indent=2), encoding='utf-8')
    flags = subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0
    with (out / 'qemu.log').open('w', encoding='utf-8') as log:
        process = subprocess.Popen(command, cwd=root, stdout=log, stderr=log, creationflags=flags)
        try:
            deadline = time.monotonic() + 120
            while True:
                content = serial.read_text(encoding='utf-8', errors='replace') if serial.exists() else ''
                if '[PHYSICAL] SCREEN READY' in content:
                    break
                if process.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError(f'Inventory boot failed; evidence: {out}')
                time.sleep(0.1)
            required = ['[PHYSICAL] MODE=READ-ONLY PCI INVENTORY',
                        '[PHYSICAL] COMPLETE - INVENTORY ONLY',
                        '[PHYSICAL] AX201_COUNT=0',
                        '[PHYSICAL] RADIO=UNIMPLEMENTED DMA=NOT-ACTIVATED']
            if any(marker not in content for marker in required):
                raise RuntimeError(f'Incorrect inventory status; evidence: {out}')
            if '[BOOT] shell-first runtime ready' in content or '[S13.9A]' in content:
                raise RuntimeError('Inventory mode entered the normal/test runtime')
            with socket.create_connection(('127.0.0.1', port), timeout=5) as connection:
                stream = connection.makefile('rwb')
                json.loads(stream.readline())

                def execute(name, arguments=None):
                    request = {'execute': name}
                    if arguments is not None:
                        request['arguments'] = arguments
                    stream.write((json.dumps(request) + '\n').encode())
                    stream.flush()
                    while True:
                        response = json.loads(stream.readline())
                        if 'error' in response:
                            raise RuntimeError(response['error'])
                        if 'return' in response:
                            return response['return']

                execute('qmp_capabilities')
                execute('screendump', {'filename': str(out / 'screen.ppm')})
                execute('quit')
            if process.wait(timeout=5) != 0:
                raise RuntimeError(f'QEMU did not exit cleanly; evidence: {out}')
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
    (out / 'result.json').write_text(json.dumps({
        'inventory_smoke': 'passed', 'configured_cpus': args.cpus,
        'ap_startup_exercised': False, 'physical_wifi_acceptance': False,
        'sha256': digest,
    }, indent=2), encoding='utf-8')
    print(f'Inventory QEMU smoke: PASS ({args.cpus} configured CPUs; no AP startup). Evidence: {out}')
    print('Physical Wi-Fi acceptance: NOT PERFORMED')


if __name__ == '__main__':
    main()

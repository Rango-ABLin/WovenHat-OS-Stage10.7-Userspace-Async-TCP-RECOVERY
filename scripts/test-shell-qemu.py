"""Exercise normal-boot PS/2 input through IOAPIC using QMP key events."""
import argparse
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default=shutil.which('qemu-system-x86_64') or r'C:\Program Files\qemu\qemu-system-x86_64.exe')
    parser.add_argument('--firmware', type=Path)
    parser.add_argument('--cpus', type=int, choices=(1, 2, 4), default=4)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    qemu = Path(args.qemu)
    firmware = args.firmware or qemu.parent / 'share' / 'edk2-x86_64-code.fd'
    if not qemu.is_file() or not firmware.is_file():
        parser.error('Set --qemu and --firmware to existing files.')
    image = subprocess.check_output(['cargo', 'run', '--quiet', '--release', '--', '--print-image'], cwd=root, text=True).strip()
    out = root / 'target' / 'shell-regression'
    out.mkdir(parents=True, exist_ok=True)
    serial = out / 'serial.log'
    serial.write_text('')
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    command = [str(qemu), '-machine', 'pc', '-m', '256M', '-smp', str(args.cpus),
               '-display', 'none', '-serial', f'file:{serial}', '-no-reboot',
               '-qmp', f'tcp:127.0.0.1:{port},server=on,wait=off',
               '-drive', f'if=pflash,format=raw,readonly=on,file={firmware}',
               '-drive', f'if=none,id=boot,format=raw,readonly=on,file={image}',
               '-device', 'virtio-blk-pci,drive=boot,bootindex=1']
    flags = subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0
    with (out / 'qemu.log').open('w') as errors:
        process = subprocess.Popen(command, cwd=root, stdout=errors, stderr=errors, creationflags=flags)
        try:
            deadline = time.monotonic() + 180
            while '[BOOT] shell-first runtime ready' not in serial.read_text(errors='replace'):
                if process.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError('Normal boot failed; inspect shell-regression logs')
                time.sleep(0.1)
            time.sleep(1)
            with socket.create_connection(('127.0.0.1', port), timeout=5) as connection:
                stream = connection.makefile('rwb')
                json.loads(stream.readline())

                def execute(command, arguments=None):
                    request = {'execute': command}
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
                for key in list('smptest') + ['ret']:
                    execute('send-key', {'keys': [{'type': 'qcode', 'data': key}], 'hold-time': 40})
                    time.sleep(0.1)
                deadline = time.monotonic() + 60
                while '[SMP] acknowledged TLB shootdowns: PASSED' not in serial.read_text(errors='replace'):
                    if process.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError('Keyboard/SMP shell command failed; inspect shell-regression logs')
                    time.sleep(0.1)
                execute('quit')
            process.wait(timeout=5)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
    print('Normal release shell + PS/2 IOAPIC + smptest: PASS')
    print('Serial log:', serial)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

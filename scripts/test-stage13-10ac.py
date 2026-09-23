"""Serial Windows software gate for the AC remediation; never certifies physical AX200."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time


def main():
    if os.name != 'nt':
        raise SystemExit('This gate invokes the repository Windows preservation launchers.')
    import msvcrt
    root = Path(__file__).resolve().parents[1]
    artifacts = root / 'audit-artifacts'
    artifacts.mkdir(exist_ok=True)
    # The OS releases this lock even if the launcher is interrupted.
    with (artifacts / 'ac-acceptance.lock').open('a+b') as lock:
        lock.write(b'0')
        lock.flush()
        lock.seek(0)
        try:
            msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
        except OSError:
            raise SystemExit('Another AC acceptance chain holds the lock.')
        return run(root, artifacts)


def run(root, artifacts):
    out = artifacts / f'ac-full-{time.time_ns()}'
    out.mkdir()
    env = os.environ.copy()
    env['CARGO_BUILD_BUILD_DIR'] = str(root / '.cargo-build')
    env['CARGO_TARGET_DIR'] = str(root / 'target')
    sources = [*root.glob('*.toml'), root / 'Cargo.lock', *root.glob('*.rs'),
               *root.glob('*.ps1'), *root.glob('.cargo/*.toml')]
    for directory, pattern in [('kernel', '*.rs'), ('kernel', '*.S'), ('kernel', '*.toml'),
                               ('src', '*.rs'), ('libwoven', '*.rs'), ('libwoven', '*.toml'),
                               ('tests', '*.rs'), ('tests', '*.py'), ('scripts', '*.py'),
                               ('vendor', '*.rs'), ('vendor', '*.toml'), ('vendor', '*.lock')]:
        sources.extend((root / directory).rglob(pattern))
    manifest = {
        'base_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip(),
        'source_sha256': {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
                          for p in sorted(set(sources))},
    }
    (out / 'source-manifest.json').write_text(json.dumps(manifest, indent=2), encoding='utf-8')
    ps = ['powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File']
    kernel_lint = ['cargo', 'clippy', '-p', 'wovenhat-kernel', '--target', 'x86_64-unknown-none']
    checks = [
        ('build', ['cargo', 'build']),
        ('host-clippy', ['cargo', 'clippy', '-p', 'wovenhat-os', '--all-targets', '--', '-D', 'warnings']),
        ('kernel-clippy', [*kernel_lint, '--', '-D', 'warnings']),
        ('host-tests', ['cargo', 'test']),
        ('wifi-host-tests', ['cargo', 'test', '--features', 'stage13-9-test']),
        ('python-tests', [sys.executable, '-m', 'unittest', 'discover', '-s', 'tests', '-p', 'test_*.py']),
        ('stage10.7-preservation', [*ps, 'RUN-STAGE10.7.ps1']),
        ('release', [sys.executable, 'scripts/test-release.py']),
    ]
    for cpu in (2, 4):
        checks.append((f'hotplug-{cpu}', [sys.executable, 'scripts/test-stage6-hotplug.py', '--cpus', str(cpu)]))
    for stage in ('1-5', '10.8', '11.3', '11.4', '11.5', '12.1', '12.2', '12.3', '12.4', '12.5'):
        checks.append((f'lint-{stage}', [*kernel_lint, '--features', f'stage{stage.replace(".", "-")}-test', '--', '-D', 'warnings']))
    for stage in ('1-5', '10.8'):
        for cpu in (1, 2, 4):
            checks.append((f'stage{stage}-{cpu}', [sys.executable, 'scripts/test-stage10-runtime.py', '--stage', stage, '--cpus', str(cpu)]))
    for stage in ('10-9', '11-1', '11-2', '11-3-5', '12', *(f'13-{n}' for n in range(1, 10))):
        checks.append((f'stage{stage}', [*ps, f'run-stage{stage}-acceptance.ps1']))
    results = []
    print('Evidence:', out, flush=True)
    for name, command in checks:
        print('START', name, flush=True)
        started = time.monotonic()
        with (out / f'{name}.txt').open('w', encoding='utf-8') as output:
            result = subprocess.run(command, cwd=root, env=env, stdout=output,
                                    stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW)
        results.append({'check': name, 'command': command, 'exit_code': result.returncode,
                        'seconds': round(time.monotonic() - started, 2)})
        (out / 'results.json').write_text(json.dumps(results, indent=2), encoding='utf-8')
        # Legacy harnesses overwrite fixed paths. Snapshot their evidence before
        # another gate runs; never remove earlier success or failure evidence.
        for folder in (root / 'target').iterdir():
            if folder.is_dir() and ('regression' in folder.name or folder.name == 'release-validation'):
                for log in folder.rglob('*'):
                    if log.is_file() and log.suffix in ('.log', '.json', '.txt'):
                        destination = out / name / folder.name / log.relative_to(folder)
                        destination.parent.mkdir(parents=True, exist_ok=True)
                        shutil.copy2(log, destination)
        print('RESULT', name, result.returncode, flush=True)
        if result.returncode:
            print('FAILED; evidence:', out, flush=True)
            return result.returncode
    for name, digest in manifest['source_sha256'].items():
        if hashlib.sha256((root / name).read_bytes()).hexdigest() != digest:
            raise SystemExit(f'Source changed during acceptance: {name}')
    print('STAGE 13.10AC SOFTWARE ACCEPTANCE: PASS', flush=True)
    print('Physical AX200, firmware/RX-ring integration and hardware MSI/DMA qualification remain separate.', flush=True)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

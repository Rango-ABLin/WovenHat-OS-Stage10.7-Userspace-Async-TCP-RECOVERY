"""Package only the image and source state recorded by the successful release gate."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import zipfile

root = Path(__file__).resolve().parents[1]
validation = root / 'target' / 'release-validation'
manifest = json.loads((validation / 'validated-image.json').read_text())
report = validation / 'results.json'
checks = json.loads(report.read_text())
required = {'lint-kernel', 'lint-host', 'legacy-pic', 'build-release', 'shell-smoke',
            'test-buffer_cache', 'test-file_mapping', 'test-page_cache'}
required |= {f'{suite}-{cpus}-debug' for suite in ('memory', 'storage') for cpus in (1, 2, 4)}
required |= {f'{suite}-4-release' for suite in ('memory', 'storage')}
if not required <= {item['check'] for item in checks} or not all(item['passed'] for item in checks):
    raise SystemExit('Release gates are incomplete or failed.')
sha = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
if sha(report) != manifest['checks_sha256']:
    raise SystemExit('Validation report changed; rerun the release gate.')
current_sources = [root / name for name in ('Cargo.toml', 'Cargo.lock', 'kernel/Cargo.toml', 'build.rs', 'rust-toolchain.toml', '.cargo/config.toml')]
current_sources += list((root / 'kernel' / 'src').rglob('*.rs')) + list((root / 'kernel' / 'src').rglob('*.S'))
current_sources += list((root / 'src').rglob('*.rs')) + list((root / 'scripts').glob('test-*.py'))
if {str(path.relative_to(root)) for path in current_sources} != set(manifest['sources']):
    raise SystemExit('Source file inventory changed; rerun the release gate.')
for name, digest in manifest['sources'].items():
    if sha(root / name) != digest:
        raise SystemExit(f'Source changed after validation: {name}')
image = Path(manifest['image'])
if sha(image) != manifest['image_sha256']:
    raise SystemExit('Image changed after validation; rerun the release gate.')
out = root / 'target' / 'releases' / '0.8.0'
out.mkdir(parents=True, exist_ok=True)
packaged = out / 'wovenhat-os-0.8.0-uefi.img'
shutil.copyfile(image, packaged)
shutil.copyfile(root / 'docs' / 'release-0.8.0.md', out / 'RELEASE-NOTES.md')
shutil.copyfile(report, out / 'results.json')
manifest['image'] = packaged.name
manifest['base_commit'] = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()
manifest['working_tree_dirty'] = bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=root, text=True).strip())
(out / 'manifest.json').write_text(json.dumps(manifest, indent=2), encoding='utf-8')
(out / 'SHA256SUMS').write_text(f"{sha(packaged)}  {packaged.name}\n", encoding='utf-8')
archive = out / 'wovenhat-os-0.8.0.zip'
with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as bundle:
    for name in (packaged.name, 'RELEASE-NOTES.md', 'results.json', 'manifest.json', 'SHA256SUMS'):
        bundle.write(out / name, name)
    for log in sorted(validation.glob('*.log')):
        bundle.write(log, 'validation/' + log.name)
print('Packaged validated release:', archive)
print('UEFI image:', packaged)
print('SHA256:', sha(packaged))

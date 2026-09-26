$ErrorActionPreference = 'Stop'
$env:CARGO_BUILD_BUILD_DIR = Join-Path $PSScriptRoot '.cargo-build'
$env:CARGO_TARGET_DIR = Join-Path $PSScriptRoot 'target'
Set-Location $PSScriptRoot
python -m unittest discover -s tests -p 'test_*.py'
if ($LASTEXITCODE -ne 0) { throw 'Host harness regressions failed' }
cargo build --features stage13-9-test
if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
cargo clippy -p wovenhat-kernel --target x86_64-unknown-none --features stage13-9-test -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'Kernel lint failed' }
foreach ($cpu in 1,2,4) {
    python .\scripts\test-stage10-runtime.py --stage 13.9 --cpus $cpu --timeout 120
    if ($LASTEXITCODE -ne 0) { throw "Stage 13.10AC failed on $cpu CPU(s)" }
}
Write-Host '=== STAGE 13.10AC BOUNDED DEFERRED AX200 IRQ RX SERVICE: PASS ==='
Write-Host 'Synthetic contract gate only; full preservation and physical AX200 qualification are separate requirements.'

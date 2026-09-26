$ErrorActionPreference = 'Stop'
cargo build --features stage13-5-test
if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
cargo clippy -p wovenhat-kernel --target x86_64-unknown-none --features stage13-5-test -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'Kernel lint failed' }
foreach ($cpu in 1,2,4) {
    python .\scripts\test-stage10-runtime.py --stage 13.5 --cpus $cpu --timeout 120
    if ($LASTEXITCODE -ne 0) { throw "Stage 13.5 failed on $cpu CPUs" }
}
Write-Host '=== STAGE 13.5 ACCEPTANCE: PASS ==='

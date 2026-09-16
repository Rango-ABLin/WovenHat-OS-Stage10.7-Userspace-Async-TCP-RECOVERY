$ErrorActionPreference = 'Stop'
cargo build --features stage13-1-test
if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
cargo clippy -p wovenhat-kernel --target x86_64-unknown-none --features stage13-1-test -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'Kernel lint failed' }
foreach ($cpu in 1,2,4) {
    python .\scripts\test-stage10-runtime.py --stage 13.1 --cpus $cpu --timeout 60
    if ($LASTEXITCODE -ne 0) { throw "Stage 13.1 failed on $cpu CPUs" }
}
Write-Host '=== STAGE 13.1 ACCEPTANCE: PASS ==='

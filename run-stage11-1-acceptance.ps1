$ErrorActionPreference = 'Stop'
Write-Host '=== WovenHat Stage 11.1 Process Model Acceptance ==='
cargo build
if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
cargo clippy -p wovenhat-kernel --target x86_64-unknown-none --features stage11-1-test -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'Kernel lint failed' }
cargo test
if ($LASTEXITCODE -ne 0) { throw 'Host tests failed' }
foreach ($cpu in 1,2,4) {
    python .\scripts\test-stage10-runtime.py --stage 11.1 --cpus $cpu --timeout 60
    if ($LASTEXITCODE -ne 0) { throw "Stage 11.1 boot failed on $cpu CPUs" }
}
Write-Host '=== STAGE 11.1 ACCEPTANCE: PASS ==='

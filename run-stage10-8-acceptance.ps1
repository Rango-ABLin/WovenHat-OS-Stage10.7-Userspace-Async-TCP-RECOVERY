$ErrorActionPreference = 'Stop'
Write-Host '=== WovenHat Stage 10.8 Completion Port Acceptance ==='
cargo build
if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
cargo clippy -p wovenhat-kernel --target x86_64-unknown-none -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'Kernel lint failed' }
cargo clippy -p wovenhat-kernel --features stage10-8-test --target x86_64-unknown-none -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'Kernel probe lint failed' }
cargo test
if ($LASTEXITCODE -ne 0) { throw 'Host tests failed' }
& "$PSScriptRoot\run-stage10-7-acceptance.ps1"
if ($LASTEXITCODE -ne 0) { throw 'Stage 10.7 preservation failed' }
foreach ($cpu in 1,2,4) {
    python .\scripts\test-stage10-runtime.py --stage 10.8 --cpus $cpu
    if ($LASTEXITCODE -ne 0) { throw "Completion-port boot failed on $cpu CPUs" }
}
Write-Host '=== STAGE 10.8 ACCEPTANCE: PASS ==='

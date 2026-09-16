$ErrorActionPreference = 'Stop'
Write-Host '=== WovenHat Stage 10.9 Timers + Asynchronous Events Acceptance ==='
cargo build
if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
cargo clippy -p wovenhat-kernel --target x86_64-unknown-none -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'Kernel lint failed' }
cargo clippy -p wovenhat-kernel --features stage10-9-test --target x86_64-unknown-none -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'Timer/event probe lint failed' }
cargo test
if ($LASTEXITCODE -ne 0) { throw 'Host tests failed' }
foreach ($cpu in 1,2,4) {
    python .\scripts\test-stage10-runtime.py --stage 10.9 --cpus $cpu
    if ($LASTEXITCODE -ne 0) { throw "Timer/event boot failed on $cpu CPUs" }
}
Write-Host '=== STAGE 10.9 ACCEPTANCE: PASS ==='

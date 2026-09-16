$ErrorActionPreference = "Stop"
Write-Host "=== WovenHat Stage 10.7 Userspace Async TCP Acceptance ==="
Write-Host ""
Write-Host "=== Host acceptance-harness regressions ==="
python -m unittest discover -s tests -p 'test_*.py'
if ($LASTEXITCODE -ne 0) { throw 'Host acceptance-harness regressions failed' }
Write-Host "=== Host lint ==="
cargo clippy -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'Host lint failed' }
Write-Host "=== 1/2 Preserve complete validated Stage 10.6 baseline ==="
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage10-6-acceptance.ps1
if ($LASTEXITCODE -ne 0) { throw "Stage 10.6 preservation acceptance failed with exit code $LASTEXITCODE" }
Write-Host ""
Write-Host "=== 2/2 Isolated Stage 10.7 Ring-3 async TCP boots: 1/2/4 CPUs ==="
foreach ($cpu in 1,2,4) {
    python .\scripts\test-stage10-7-tcp.py --cpus $cpu
    if ($LASTEXITCODE -ne 0) { throw "Stage 10.7 isolated TCP boot failed on $cpu CPU(s)" }
    Write-Host "Stage 10.7 userspace async TCP marker: PASS ($cpu CPU)"
}
Write-Host ""
Write-Host "=== STAGE 10.7 ACCEPTANCE: PASS ==="
Write-Host "Stage 10.6 preserved; ordinary Ring-3 applications now use asynchronous TCP connect/send/receive with generation-pinned sockets, readiness-driven retries, remote EOF completion, deferred close, cancellation, retry-safe copyout, and deterministic teardown on 1/2/4 CPUs."

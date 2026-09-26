$ErrorActionPreference = "Stop"
Write-Host "=== WovenHat Stage 10.6 Userspace Async Networking Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve complete validated Stage 10.5 baseline ==="
& "$PSScriptRoot\run-stage10-5-acceptance.ps1"
if ($LASTEXITCODE -ne 0) { throw "Stage 10.5 preservation acceptance failed with exit code $LASTEXITCODE" }
Write-Host ""
Write-Host "=== 2/2 Isolated Stage 10.6 Ring-3 async networking boots: 1/2/4 CPUs ==="
foreach ($cpu in 1,2,4) {
    python .\scripts\test-stage10-6-network.py --cpus $cpu
    if ($LASTEXITCODE -ne 0) { throw "Stage 10.6 isolated network boot failed on $cpu CPU(s)" }
    Write-Host "Stage 10.6 userspace async networking marker: PASS ($cpu CPU)"
}
Write-Host ""
Write-Host "=== STAGE 10.6 ACCEPTANCE: PASS ==="
Write-Host "Stage 10.5 preserved; ordinary Ring-3 applications now use owner-bound asynchronous UDP send/receive with generation-pinned sockets, kernel-owned buffers, readiness-driven worker wakeups, cancellation, retry-safe completion copyout, and deterministic teardown on 1/2/4 CPUs."

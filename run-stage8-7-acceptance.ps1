$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 8.7 SMP IPC Stress Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 8.6 baseline ==="
& "$PSScriptRoot\run-stage8-6-acceptance.ps1"
if ($LASTEXITCODE -ne 0) {
    throw "Stage 8.6 preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 8.7 marker on 1/2/4 CPU production boots ==="
$marker = "[S8.7] multicore IPC/service/capability stress: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) {
        throw "Missing Stage 8.7 serial log: $serial"
    }
    $found = Select-String -Path $serial -SimpleMatch $marker -Quiet
    if (-not $found) {
        Write-Host "Stage 8.7 marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 240
        throw "Stage 8.7 SMP IPC stress validation failed for $cpu CPU(s)"
    }
    Write-Host "Stage 8.7 SMP IPC stress marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 8.7 ACCEPTANCE: PASS ==="
Write-Host "Stage 8.6 preserved; every online CPU concurrently exercised service discovery, shared-memory I/O, blocking queue backpressure, reduced-rights capability transfer, sender teardown, receiver cleanup and bounded queue saturation."

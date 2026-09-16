$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 9.2C Real Task Authority Enforcement Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 9.2B baseline ==="
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-2b-acceptance.ps1
if ($LASTEXITCODE -ne 0) {
    throw "Stage 9.2B preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 9.2C enforcement marker on 1/2/4 CPU production boots ==="
$marker = "[S9.2C] WovenGuard task capability lineage enforcement: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) { throw "Missing Stage 9.2C serial log: $serial" }
    if (-not (Select-String -Path $serial -SimpleMatch $marker -Quiet)) {
        Write-Host "Stage 9.2C marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 320
        throw "Stage 9.2C authority-enforcement validation failed for $cpu CPU(s)"
    }
    Write-Host "Stage 9.2C task capability lineage enforcement marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 9.2C ACCEPTANCE: PASS ==="
Write-Host "Stage 9.2B preserved; delegated task capabilities are lineage-backed, recursive revocation removes effective authority immediately, fork preserves lineage binding, and task reap recalls owned delegation trees."

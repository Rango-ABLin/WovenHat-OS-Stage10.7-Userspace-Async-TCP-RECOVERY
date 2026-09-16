$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 9.3 WovenGuard Security Audit Ledger Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 9.2E baseline ==="
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-2e-acceptance.ps1
if ($LASTEXITCODE -ne 0) {
    throw "Stage 9.2E preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 9.3 audit-ledger marker on 1/2/4 CPU production boots ==="
$marker = "[S9.3] WovenGuard security audit ledger: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) { throw "Missing Stage 9.3 serial log: $serial" }
    if (-not (Select-String -Path $serial -SimpleMatch $marker -Quiet)) {
        Write-Host "Stage 9.3 marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 460
        throw "Stage 9.3 security audit ledger validation failed for $cpu CPU(s)"
    }
    Write-Host "Stage 9.3 WovenGuard security audit ledger marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 9.3 ACCEPTANCE: PASS ==="
Write-Host "Stage 9.2E preserved; the bounded WovenGuard ledger maintains monotonic global sequence ordering, CPU/task attribution, action-specific detail, explicit allow/deny outcomes, deterministic overwrite accounting and chronological bounded readout."

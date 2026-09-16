$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 9.4B IPC / Service Exposure Policy Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 9.4A baseline ==="
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-4a-acceptance.ps1
if ($LASTEXITCODE -ne 0) {
    throw "Stage 9.4A preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 9.4B IPC/service policy marker on 1/2/4 CPU production boots ==="
$marker = "[S9.4B] WovenGuard IPC/service exposure policy: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) { throw "Missing Stage 9.4B serial log: $serial" }
    if (-not (Select-String -Path $serial -SimpleMatch $marker -Quiet)) {
        Write-Host "Stage 9.4B marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 500
        throw "Stage 9.4B IPC/service exposure validation failed for $cpu CPU(s)"
    }
    Write-Host "Stage 9.4B IPC/service exposure policy marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 9.4B ACCEPTANCE: PASS ==="
Write-Host "Stage 9.4A preserved; service publication/discovery obey TCB-bound sandbox service-class masks, service-derived handles retain policy identity across grants/transfers, profile tightening blocks already-open service handles on later use, and allow/deny decisions are recorded in the WovenGuard ledger."

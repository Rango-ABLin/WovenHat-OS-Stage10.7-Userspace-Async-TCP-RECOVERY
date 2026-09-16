$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 8.5 Capability Handle Transfer Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 8.4 baseline ==="
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage8-4-acceptance.ps1
if ($LASTEXITCODE -ne 0) {
    throw "Stage 8.4 preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 8.5 marker on 1/2/4 CPU production boots ==="
$marker = "[S8.5] capability handle transfer through IPC: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) {
        throw "Missing Stage 8.5 serial log: $serial"
    }
    $found = Select-String -Path $serial -SimpleMatch $marker -Quiet
    if (-not $found) {
        Write-Host "Stage 8.5 marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 200
        throw "Stage 8.5 capability-transfer validation failed for $cpu CPU(s)"
    }
    Write-Host "Stage 8.5 capability-transfer marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 8.5 ACCEPTANCE: PASS ==="
Write-Host "Stage 8.4 preserved; queued capability escrow, rights reduction, fresh receiver-local handles, sender-close survival, full-table atomicity, enqueue rollback and abandoned-message teardown validated."

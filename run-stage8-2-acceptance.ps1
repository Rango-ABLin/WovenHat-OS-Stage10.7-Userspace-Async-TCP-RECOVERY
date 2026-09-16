$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 8.2 Handle-Addressed Message Passing Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 8.1 baseline ==="
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage8-1-acceptance.ps1
if ($LASTEXITCODE -ne 0) {
    throw "Stage 8.1 preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 8.2 marker on 1/2/4 CPU production boots ==="
$marker = "[S8.2] handle-addressed bounded message passing: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) {
        throw "Missing Stage 8.2 serial log: $serial"
    }
    $found = Select-String -Path $serial -SimpleMatch $marker -Quiet
    if (-not $found) {
        Write-Host "Stage 8.2 marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 120
        throw "Stage 8.2 handle-addressed messaging failed for $cpu CPU(s)"
    }
    Write-Host "Stage 8.2 bounded handle messaging marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 8.2 ACCEPTANCE: PASS ==="
Write-Host "Stage 8.1 preserved; bounded FIFO endpoint queues, SEND/RECEIVE rights, nonblocking empty/full semantics, stale-handle rejection and shared-object teardown validated."

$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 8.1 IPC Endpoint/Handle Foundation Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 7.6 baseline ==="
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage7-6-acceptance.ps1
if ($LASTEXITCODE -ne 0) {
    throw "Stage 7.6 preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 8.1 marker on 1/2/4 CPU production boots ==="
$marker = "[S8.1] kernel endpoint objects + process-local handles: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) {
        throw "Missing Stage 8.1 serial log: $serial"
    }
    $found = Select-String -Path $serial -SimpleMatch $marker -Quiet
    if (-not $found) {
        Write-Host "Stage 8.1 marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 100
        throw "Stage 8.1 IPC handle foundation failed for $cpu CPU(s)"
    }
    Write-Host "Stage 8.1 IPC endpoint/handle marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 8.1 ACCEPTANCE: PASS ==="
Write-Host "Stage 7.6 preserved; kernel IPC endpoint objects, process-local generation-tagged handles, rights metadata and deterministic teardown validated."

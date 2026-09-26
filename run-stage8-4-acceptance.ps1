$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 8.4 Shared Memory Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 8.3 baseline ==="
& "$PSScriptRoot\run-stage8-3-acceptance.ps1"
if ($LASTEXITCODE -ne 0) {
    throw "Stage 8.3 preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 8.4 marker on 1/2/4 CPU production boots ==="
$marker = "[S8.4] shared-memory objects + cross-address-space mappings: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) {
        throw "Missing Stage 8.4 serial log: $serial"
    }
    $found = Select-String -Path $serial -SimpleMatch $marker -Quiet
    if (-not $found) {
        Write-Host "Stage 8.4 marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 180
        throw "Stage 8.4 shared-memory validation failed for $cpu CPU(s)"
    }
    Write-Host "Stage 8.4 shared-memory marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 8.4 ACCEPTANCE: PASS ==="
Write-Host "Stage 8.3 preserved; shared-memory object lifetime, rights, two-page cross-address-space visibility, explicit unmap/remap and mapping-held references validated."

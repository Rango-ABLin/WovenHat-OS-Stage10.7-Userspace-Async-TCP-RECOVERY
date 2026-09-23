$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 8.6 Service Discovery Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 8.5 baseline ==="
& "$PSScriptRoot\run-stage8-5-acceptance.ps1"
if ($LASTEXITCODE -ne 0) {
    throw "Stage 8.5 preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 8.6 marker on 1/2/4 CPU production boots ==="
$marker = "[S8.6] named service registry + capability discovery: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) {
        throw "Missing Stage 8.6 serial log: $serial"
    }
    $found = Select-String -Path $serial -SimpleMatch $marker -Quiet
    if (-not $found) {
        Write-Host "Stage 8.6 marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 200
        throw "Stage 8.6 service-discovery validation failed for $cpu CPU(s)"
    }
    Write-Host "Stage 8.6 service-discovery marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 8.6 ACCEPTANCE: PASS ==="
Write-Host "Stage 8.5 preserved; bounded unique service names, publisher-only registration, registry-held endpoint lifetime, reduced client rights, fresh discovery handles and owner-exit cleanup validated."

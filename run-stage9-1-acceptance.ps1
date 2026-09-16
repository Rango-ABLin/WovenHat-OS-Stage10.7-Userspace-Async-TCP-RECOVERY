$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 9.1 WovenGuard Domain/Policy Foundation Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 8.7 baseline ==="
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage8-7-acceptance.ps1
if ($LASTEXITCODE -ne 0) {
    throw "Stage 8.7 preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 9.1 WovenGuard marker on 1/2/4 CPU production boots ==="
$marker = "[S9.1] WovenGuard domains + least-privilege policy: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) {
        throw "Missing Stage 9.1 serial log: $serial"
    }
    $found = Select-String -Path $serial -SimpleMatch $marker -Quiet
    if (-not $found) {
        Write-Host "Stage 9.1 marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 260
        throw "Stage 9.1 WovenGuard domain/policy validation failed for $cpu CPU(s)"
    }
    Write-Host "Stage 9.1 WovenGuard domain/policy marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 9.1 ACCEPTANCE: PASS ==="
Write-Host "Stage 8.7 preserved; security domains, least-privilege capability ceilings, centrally authorized delegation, fork domain inheritance and WovenGuard allow/deny auditing validated."

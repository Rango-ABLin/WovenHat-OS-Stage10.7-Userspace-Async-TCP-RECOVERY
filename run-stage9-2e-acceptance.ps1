$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 9.2E SMP Revocation + Lineage Closure Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 9.2D baseline ==="
& "$PSScriptRoot\run-stage9-2d-acceptance.ps1"
if ($LASTEXITCODE -ne 0) {
    throw "Stage 9.2D preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 9.2E SMP revocation marker on 1/2/4 CPU production boots ==="
$marker = "[S9.2E] WovenGuard SMP revocation + lineage closure: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) { throw "Missing Stage 9.2E serial log: $serial" }
    if (-not (Select-String -Path $serial -SimpleMatch $marker -Quiet)) {
        Write-Host "Stage 9.2E marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 420
        throw "Stage 9.2E SMP revocation validation failed for $cpu CPU(s)"
    }
    Write-Host "Stage 9.2E SMP revocation + lineage closure marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 9.2E ACCEPTANCE: PASS ==="
Write-Host "Stage 9.2D preserved; every online CPU exercised delegated authority before recall, stale authority was denied after the revocation linearization point, revoked handles remained teardown-safe, generation reuse rejected stale handles, and object/lineage counts returned to baseline."

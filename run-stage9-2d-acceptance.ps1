$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 9.2D IPC/Object Capability Lineage Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 9.2C baseline ==="
& "$PSScriptRoot\run-stage9-2c-acceptance.ps1"
if ($LASTEXITCODE -ne 0) {
    throw "Stage 9.2C preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 9.2D IPC/object lineage marker on 1/2/4 CPU production boots ==="
$marker = "[S9.2D] WovenGuard IPC/object capability lineage: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) { throw "Missing Stage 9.2D serial log: $serial" }
    if (-not (Select-String -Path $serial -SimpleMatch $marker -Quiet)) {
        Write-Host "Stage 9.2D marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 360
        throw "Stage 9.2D IPC/object lineage validation failed for $cpu CPU(s)"
    }
    Write-Host "Stage 9.2D IPC/object capability lineage marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 9.2D ACCEPTANCE: PASS ==="
Write-Host "Stage 9.2C preserved; direct grants, queued capability escrow and service discovery carry revocable WovenGuard IPC lineage without conflating object-specific u8 rights with task CapabilitySet authority."

$ErrorActionPreference = "Stop"
Write-Host "=== WovenHat Stage 9.4D Sandbox Lifecycle + SMP Closure Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 9.4C baseline ==="
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-4c-acceptance.ps1
if ($LASTEXITCODE -ne 0) { throw "Stage 9.4C preservation acceptance failed with exit code $LASTEXITCODE" }

Write-Host ""
Write-Host "=== 2/2 Verify Stage 9.4D sandbox lifecycle marker on 1/2/4 CPU production boots ==="
foreach ($cpu in @(1,2,4)) {
    $log = ".\target\memory-regression-$cpu-debug\serial.log"
    if (!(Test-Path $log)) { throw "Missing $log" }
    if (!(Select-String -Path $log -SimpleMatch "[S9.4D] WovenGuard sandbox lifecycle + SMP closure: PASSED" -Quiet)) {
        throw "Stage 9.4D marker missing for $cpu CPU(s): $log"
    }
    Write-Host "Stage 9.4D sandbox lifecycle + SMP closure marker: PASS ($cpu CPU)"
}
Write-Host ""
Write-Host "=== STAGE 9.4D ACCEPTANCE: PASS ==="
Write-Host "Stage 9.4C preserved; every online CPU observed live sandbox tightening through the scheduler linearization point, stripped authority stayed revoked after profile broadening, and service/file policy tightening remained coherent across SMP task lifecycle transitions."

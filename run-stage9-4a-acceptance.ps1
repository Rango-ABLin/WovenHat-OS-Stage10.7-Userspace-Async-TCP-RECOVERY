$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 9.4A Sandbox Profile + Capability Ceiling Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 9.3 baseline ==="
& "$PSScriptRoot\run-stage9-3-acceptance.ps1"
if ($LASTEXITCODE -ne 0) {
    throw "Stage 9.3 preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 9.4A sandbox marker on 1/2/4 CPU production boots ==="
$marker = "[S9.4A] WovenGuard sandbox profile + capability ceiling: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) { throw "Missing Stage 9.4A serial log: $serial" }
    if (-not (Select-String -Path $serial -SimpleMatch $marker -Quiet)) {
        Write-Host "Stage 9.4A marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 500
        throw "Stage 9.4A sandbox profile validation failed for $cpu CPU(s)"
    }
    Write-Host "Stage 9.4A sandbox profile + capability ceiling marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 9.4A ACCEPTANCE: PASS ==="
Write-Host "Stage 9.3 preserved; sandbox profiles are TCB-bound, fork-inherited, subordinate to SecurityDomain ceilings, enforced by effective capability checks and grant policy, destructively strip out-of-profile authority, and audit bind/deny decisions in the WovenGuard ledger."

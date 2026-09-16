$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 9.2A Capability Lineage Foundation Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 9.1 baseline ==="
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-1-acceptance.ps1
if ($LASTEXITCODE -ne 0) {
    throw "Stage 9.1 preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 9.2A lineage marker on 1/2/4 CPU production boots ==="
$marker = "[S9.2A] WovenGuard capability lineage foundation: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) {
        throw "Missing Stage 9.2A serial log: $serial"
    }
    $found = Select-String -Path $serial -SimpleMatch $marker -Quiet
    if (-not $found) {
        Write-Host "Stage 9.2A marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 280
        throw "Stage 9.2A capability-lineage validation failed for $cpu CPU(s)"
    }
    Write-Host "Stage 9.2A capability-lineage marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 9.2A ACCEPTANCE: PASS ==="
Write-Host "Stage 9.1 preserved; generation-tagged lineage identities, bounded ancestry, rights-reducing derivation, stale-token rejection, owner checks and deterministic leaf cleanup validated."

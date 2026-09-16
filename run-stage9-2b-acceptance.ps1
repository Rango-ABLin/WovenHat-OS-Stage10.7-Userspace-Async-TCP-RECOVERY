$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 9.2B Recursive Capability Revocation Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 9.2A baseline ==="
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-2a-acceptance.ps1
if ($LASTEXITCODE -ne 0) {
    throw "Stage 9.2A preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 9.2B revocation marker on 1/2/4 CPU production boots ==="
$marker = "[S9.2B] WovenGuard recursive capability revocation: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) {
        throw "Missing Stage 9.2B serial log: $serial"
    }
    $found = Select-String -Path $serial -SimpleMatch $marker -Quiet
    if (-not $found) {
        Write-Host "Stage 9.2B marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 300
        throw "Stage 9.2B recursive-revocation validation failed for $cpu CPU(s)"
    }
    Write-Host "Stage 9.2B recursive-revocation marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 9.2B ACCEPTANCE: PASS ==="
Write-Host "Stage 9.2A preserved; atomic subtree revocation, ancestor-authorized recall, sibling preservation, generation-safe invalidation and stale-descendant rejection validated."

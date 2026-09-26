$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 9.4C Filesystem / Resource Sandbox Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 9.4B baseline ==="
& "$PSScriptRoot\run-stage9-4b-acceptance.ps1"
if ($LASTEXITCODE -ne 0) {
    throw "Stage 9.4B preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 9.4C filesystem/resource sandbox marker on 1/2/4 CPU production boots ==="
$marker = "[S9.4C] WovenGuard filesystem/resource sandbox: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) { throw "Missing Stage 9.4C serial log: $serial" }
    if (-not (Select-String -Path $serial -SimpleMatch $marker -Quiet)) {
        Write-Host "Stage 9.4C marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 500
        throw "Stage 9.4C filesystem/resource sandbox validation failed for $cpu CPU(s)"
    }
    Write-Host "Stage 9.4C filesystem/resource sandbox marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 9.4C ACCEPTANCE: PASS ==="
Write-Host "Stage 9.4B preserved; resolved filesystem paths are classified into bounded sandbox scopes, open descriptors retain scope identity across dup/fork, current policy is rechecked on read/write/seek/new mmap, file-backed mapping scope is retained for msync, path mutations are scope-gated, and file policy allow/deny decisions are recorded in the WovenGuard ledger."

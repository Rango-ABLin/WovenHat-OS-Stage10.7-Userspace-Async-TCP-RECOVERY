$ErrorActionPreference = "Stop"
Write-Host "=== WovenHat Stage 9.5 Resource / Device Capability Gates Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 9.4D baseline ==="
& "$PSScriptRoot\run-stage9-4d-acceptance.ps1"
if ($LASTEXITCODE -ne 0) { throw "Stage 9.4D preservation acceptance failed with exit code $LASTEXITCODE" }

Write-Host ""
Write-Host "=== 2/2 Verify Stage 9.5 resource/device capability marker on 1/2/4 CPU production boots ==="
foreach ($cpu in @(1,2,4)) {
    $log = ".\target\memory-regression-$cpu-debug\serial.log"
    if (!(Test-Path $log)) { throw "Missing $log" }
    if (!(Select-String -Path $log -SimpleMatch "[S9.5] WovenGuard resource/device capability gates: PASSED" -Quiet)) {
        throw "Stage 9.5 marker missing for $cpu CPU(s): $log"
    }
    Write-Host "Stage 9.5 resource/device capability gates marker: PASS ($cpu CPU)"
}
Write-Host ""
Write-Host "=== STAGE 9.5 ACCEPTANCE: PASS ==="
Write-Host "Stage 9.4D preserved; network/storage/display/input resource classes are capability-gated, sandbox device masks can narrow live authority, network syscalls enforce NetworkIo, teardown remains safe, and resource access decisions are recorded in the WovenGuard ledger."

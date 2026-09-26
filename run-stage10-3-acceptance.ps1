$ErrorActionPreference = "Stop"
Write-Host "=== WovenHat Stage 10.3 Userspace Async Completion ABI Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 10.2 baseline ==="
& "$PSScriptRoot\run-stage10-2-acceptance.ps1"
if ($LASTEXITCODE -ne 0) { throw "Stage 10.2 preservation acceptance failed with exit code $LASTEXITCODE" }

Write-Host ""
Write-Host "=== 2/2 Verify Stage 10.3 userspace async completion ABI marker on 1/2/4 CPU production boots ==="
foreach ($cpu in @(1,2,4)) {
    $log = ".\target\memory-regression-$cpu-debug\serial.log"
    if (!(Test-Path $log)) { throw "Missing $log" }
    if (!(Select-String -Path $log -SimpleMatch "[S10.3] userspace async completion ABI + cancellation/teardown: PASSED" -Quiet)) {
        throw "Stage 10.3 marker missing for $cpu CPU(s): $log"
    }
    if (Select-String -Path $log -Pattern "KERNEL PANIC|userspace async completion ABI: FAILED" -Quiet) {
        throw "Stage 10.3 failure marker or panic found for $cpu CPU(s): $log"
    }
    Write-Host "Stage 10.3 userspace async completion ABI marker: PASS ($cpu CPU)"
}
Write-Host ""
Write-Host "=== STAGE 10.3 ACCEPTANCE: PASS ==="
Write-Host "Stage 10.2 preserved; Ring-3 create/poll/wait/cancel/service-complete syscalls use generation-tagged owner-bound handles, WovenGuard rechecks authority, completion copyout is retry-safe, stale handles are rejected, and normal/forced process teardown reclaims abandoned operations on 1/2/4 CPU production boots."

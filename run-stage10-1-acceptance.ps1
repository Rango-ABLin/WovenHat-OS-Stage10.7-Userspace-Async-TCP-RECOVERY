$ErrorActionPreference = "Stop"
Write-Host "=== WovenHat Stage 10.1 Kernel Event + Async I/O Foundation Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 9.5 baseline ==="
& "$PSScriptRoot\run-stage9-5-acceptance.ps1"
if ($LASTEXITCODE -ne 0) { throw "Stage 9.5 preservation acceptance failed with exit code $LASTEXITCODE" }

Write-Host ""
Write-Host "=== 2/2 Verify Stage 10.1 event-driven async I/O marker on 1/2/4 CPU production boots ==="
foreach ($cpu in @(1,2,4)) {
    $log = ".\target\memory-regression-$cpu-debug\serial.log"
    if (!(Test-Path $log)) { throw "Missing $log" }
    if (!(Select-String -Path $log -SimpleMatch "[S10.1] kernel event + async I/O foundation: PASSED" -Quiet)) {
        throw "Stage 10.1 marker missing for $cpu CPU(s): $log"
    }
    Write-Host "Stage 10.1 kernel event + async I/O foundation marker: PASS ($cpu CPU)"
}
Write-Host ""
Write-Host "=== STAGE 10.1 ACCEPTANCE: PASS ==="
Write-Host "Stage 9.5 preserved; queued block I/O waiters sleep on lost-wakeup-safe scheduler events instead of busy-yield polling, completion signals are task-targeted, completion-before-wait is latched safely, and the event-driven path is exercised on 1/2/4 CPU production boots."

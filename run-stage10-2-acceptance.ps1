$ErrorActionPreference = "Stop"
Write-Host "=== WovenHat Stage 10.2 Generic Async Request / Completion API Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 10.1 baseline ==="
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage10-1-acceptance.ps1
if ($LASTEXITCODE -ne 0) { throw "Stage 10.1 preservation acceptance failed with exit code $LASTEXITCODE" }

Write-Host ""
Write-Host "=== 2/2 Verify Stage 10.2 generic async request/completion marker on 1/2/4 CPU production boots ==="
foreach ($cpu in @(1,2,4)) {
    $log = ".\target\memory-regression-$cpu-debug\serial.log"
    if (!(Test-Path $log)) { throw "Missing $log" }
    if (!(Select-String -Path $log -SimpleMatch "[S10.2] generic async request/completion API: PASSED" -Quiet)) {
        throw "Stage 10.2 marker missing for $cpu CPU(s): $log"
    }
    Write-Host "Stage 10.2 generic async request/completion API marker: PASS ($cpu CPU)"
}
Write-Host ""
Write-Host "=== STAGE 10.2 ACCEPTANCE: PASS ==="
Write-Host "Stage 10.1 preserved; bounded generation-tagged async operations are owner-bound, completion/wait is scheduler-event driven, completion-before-wait is state-latched, stale handles are rejected, rollback releases capacity, and real block I/O uses the generic API on 1/2/4 CPU production boots."

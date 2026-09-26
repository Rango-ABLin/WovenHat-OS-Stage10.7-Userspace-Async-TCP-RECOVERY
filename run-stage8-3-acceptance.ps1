$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 8.3 Race-Free IPC Events/Waits Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 8.2 baseline ==="
& "$PSScriptRoot\run-stage8-2-acceptance.ps1"
if ($LASTEXITCODE -ne 0) {
    throw "Stage 8.2 preservation acceptance failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "=== 2/2 Verify Stage 8.3 marker on 1/2/4 CPU production boots ==="
$marker = "[S8.3] race-free blocking IPC events/waits: PASSED"
foreach ($cpu in @(1, 2, 4)) {
    $serial = ".\target\memory-regression-$cpu-debug\serial.log"
    if (-not (Test-Path $serial)) {
        throw "Missing Stage 8.3 serial log: $serial"
    }
    $found = Select-String -Path $serial -SimpleMatch $marker -Quiet
    if (-not $found) {
        Write-Host "Stage 8.3 marker missing for $cpu CPU(s). Tail follows:"
        Get-Content $serial -Tail 160
        throw "Stage 8.3 race-free IPC wait/wake failed for $cpu CPU(s)"
    }
    Write-Host "Stage 8.3 IPC event/wait marker: PASS ($cpu CPU)"
}

Write-Host ""
Write-Host "=== STAGE 8.3 ACCEPTANCE: PASS ==="
Write-Host "Stage 8.2 preserved; scheduler-latched event permits, bounded endpoint wait queues, blocking send/receive, early-signal protection and SMP wakeups validated."

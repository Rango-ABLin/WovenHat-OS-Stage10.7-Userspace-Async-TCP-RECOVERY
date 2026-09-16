$ErrorActionPreference = "Stop"
Write-Host "=== WovenHat Stage 10.4 Userspace Async Block I/O Acceptance ==="
Write-Host ""
Write-Host "=== 1/2 Preserve the complete validated Stage 10.3 baseline (Stage 10.4 probe disabled) ==="
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage10-3-acceptance.ps1
if ($LASTEXITCODE -ne 0) { throw "Stage 10.3 preservation acceptance failed with exit code $LASTEXITCODE" }

Write-Host ""
Write-Host "=== 2/2 Isolated Stage 10.4 Ring-3 block I/O boots: 1/2/4 CPUs ==="
foreach ($cpu in @(1,2,4)) {
    python .\scripts\test-memory-qemu.py --cpus $cpu --stage10-4
    if ($LASTEXITCODE -ne 0) { throw "Stage 10.4 isolated boot failed on $cpu CPU(s)" }
    $log = ".\target\stage10-4-regression-$cpu-debug\serial.log"
    if (!(Test-Path $log)) { throw "Missing $log" }
    if (!(Select-String -Path $log -SimpleMatch "[S10.4] userspace async block read/write + bounce-buffer teardown: PASSED" -Quiet)) {
        throw "Stage 10.4 marker missing for $cpu CPU(s): $log"
    }
    if (Select-String -Path $log -Pattern "KERNEL PANIC|\[S10\.4\].*FAILED|\[S10\.4\].*TIMEOUT" -Quiet) {
        throw "Stage 10.4 failure marker, timeout or panic found for $cpu CPU(s): $log"
    }
    Write-Host "Stage 10.4 userspace async block I/O marker: PASS ($cpu CPU)"
}
Write-Host ""
Write-Host "=== STAGE 10.4 ACCEPTANCE: PASS ==="
Write-Host "Stage 10.3 preserved in unmodified qemu-test boots; Stage 10.4 is exercised only in dedicated stage10-4-test boots, preventing the new storage probe from contaminating legacy SMP stress while still validating real Ring-3 queued block I/O on 1/2/4 CPUs."

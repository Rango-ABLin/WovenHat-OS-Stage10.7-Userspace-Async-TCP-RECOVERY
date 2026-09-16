$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 8.7 Focused 2-CPU Diagnostic ==="
Write-Host "Build + strict Clippy + one 2-CPU QEMU boot"

& cargo build
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

& cargo clippy -- -D warnings
if ($LASTEXITCODE -ne 0) { throw "cargo clippy -D warnings failed with exit code $LASTEXITCODE" }

& python .\scripts\test-memory-qemu.py --cpus 2 --timeout 180
$qemuExit = $LASTEXITCODE
$serial = ".\target\memory-regression-2-debug\serial.log"

if (Test-Path $serial) {
    Write-Host ""
    Write-Host "=== Stage 8.7 diagnostic markers ==="
    Select-String -Path $serial -Pattern "\[S8\.7\]|\[S8\.6\]|EXCEPTION|FAULT|PANIC" | ForEach-Object { $_.Line }
    Write-Host ""
    Write-Host "=== Serial tail ==="
    Get-Content $serial -Tail 160
}

if ($qemuExit -ne 0) {
    throw "Focused Stage 8.7 2-CPU QEMU run failed with exit code $qemuExit"
}

Write-Host ""
Write-Host "=== STAGE 8.7 FOCUSED 2-CPU: PASS ==="

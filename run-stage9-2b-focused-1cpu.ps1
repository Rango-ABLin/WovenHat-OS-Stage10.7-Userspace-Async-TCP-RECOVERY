$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 9.2B Focused 1-CPU Validation ==="
Write-Host "Build + strict Clippy + one 1-CPU QEMU boot"

& cargo build
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

& cargo clippy -- -D warnings
if ($LASTEXITCODE -ne 0) { throw "cargo clippy -D warnings failed with exit code $LASTEXITCODE" }

& python .\scripts\test-memory-qemu.py --cpus 1 --timeout 180
$qemuExit = $LASTEXITCODE
$serial = ".\target\memory-regression-1-debug\serial.log"
$marker = "[S9.2B] WovenGuard recursive capability revocation: PASSED"

if (Test-Path $serial) {
    Write-Host ""
    Write-Host "=== Stage 9.2B security markers ==="
    Select-String -Path $serial -Pattern "\[S9\.2B\]|\[S9\.2A\]|\[S9\.1\]|\[S8\.7\]|CAPABILITY|AUDIT|EXCEPTION|FAULT|PANIC" | ForEach-Object { $_.Line }
}

if ($qemuExit -ne 0) {
    if (Test-Path $serial) {
        Write-Host ""
        Write-Host "=== Serial tail ==="
        Get-Content $serial -Tail 260
    }
    throw "Focused Stage 9.2B 1-CPU QEMU run failed with exit code $qemuExit"
}

if (-not (Test-Path $serial)) {
    throw "Focused Stage 9.2B serial log missing: $serial"
}
if (-not (Select-String -Path $serial -SimpleMatch $marker -Quiet)) {
    Write-Host ""
    Write-Host "=== Serial tail ==="
    Get-Content $serial -Tail 260
    throw "Stage 9.2B marker missing from focused 1-CPU boot"
}

Write-Host ""
Write-Host "=== STAGE 9.2B FOCUSED 1-CPU: PASS ==="

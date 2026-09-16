$ErrorActionPreference = "Stop"

Write-Host "=== WovenHat Stage 9.2C Focused 1-CPU Validation ==="
Write-Host "Build + strict Clippy + one 1-CPU QEMU boot"

& cargo build
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

& cargo clippy -- -D warnings
if ($LASTEXITCODE -ne 0) { throw "cargo clippy -D warnings failed with exit code $LASTEXITCODE" }

& python .\scripts\test-memory-qemu.py --cpus 1 --timeout 180
$qemuExit = $LASTEXITCODE
$serial = ".\target\memory-regression-1-debug\serial.log"
$marker = "[S9.2C] WovenGuard task capability lineage enforcement: PASSED"

if (Test-Path $serial) {
    Write-Host ""
    Write-Host "=== Stage 9.2C security markers ==="
    Select-String -Path $serial -Pattern "\[S9\.2C\]|\[S9\.2B\]|\[S9\.2A\]|\[S9\.1\]|CAPABILITY|AUDIT|EXCEPTION|FAULT|PANIC" | ForEach-Object { $_.Line }
}

if ($qemuExit -ne 0) {
    if (Test-Path $serial) {
        Write-Host ""
        Write-Host "=== Serial tail ==="
        Get-Content $serial -Tail 300
    }
    throw "Focused Stage 9.2C 1-CPU QEMU run failed with exit code $qemuExit"
}

if (-not (Test-Path $serial)) { throw "Focused Stage 9.2C serial log missing: $serial" }
if (-not (Select-String -Path $serial -SimpleMatch $marker -Quiet)) {
    Write-Host ""
    Write-Host "=== Serial tail ==="
    Get-Content $serial -Tail 300
    throw "Stage 9.2C marker missing from focused 1-CPU boot"
}

Write-Host ""
Write-Host "=== STAGE 9.2C FOCUSED 1-CPU: PASS ==="

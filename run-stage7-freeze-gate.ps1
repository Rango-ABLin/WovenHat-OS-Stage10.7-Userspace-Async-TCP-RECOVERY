$ErrorActionPreference = 'Stop'

Write-Host '=== WovenHat Stage 7 freeze gate ==='
Write-Host '1/4 Build + lint + host tests'
cargo build
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
cargo clippy -p wovenhat-kernel -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'cargo clippy failed' }
cargo test
if ($LASTEXITCODE -ne 0) { throw 'cargo test failed' }

Write-Host '2/4 100-run 2-CPU memory stress'
$logDir = '.\stage7-freeze-2cpu-logs'
Remove-Item $logDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
$failures = @()

1..100 | ForEach-Object {
    Write-Host "=== MEMORY 2 CPU RUN $_ OF 100 ==="
    Get-Process qemu-system-x86_64 -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 300
    python .\scripts\test-memory-qemu.py --cpus 2
    $code = $LASTEXITCODE
    $src = '.\target\memory-regression-2-debug\serial.log'
    $dst = Join-Path $logDir "run-$_.log"
    if (Test-Path $src) { Copy-Item $src $dst -Force }
    if ($code -ne 0) { $failures += $_ }
}

if ($failures.Count -ne 0) {
    Write-Host "FAILED 2-CPU runs: $($failures -join ', ')"
    throw 'Stage 7 freeze gate failed during 2-CPU stress'
}

Write-Host '3/4 Memory matrix 1/2/4 CPUs'
foreach ($cpu in 1,2,4) {
    python .\scripts\test-memory-qemu.py --cpus $cpu
    if ($LASTEXITCODE -ne 0) { throw "memory test failed on $cpu CPU(s)" }
}

Write-Host '4/4 Network matrix 1/2/4 CPUs'
foreach ($cpu in 1,2,4) {
    python .\scripts\test-network-qemu.py --cpus $cpu
    if ($LASTEXITCODE -ne 0) { throw "network test failed on $cpu CPU(s)" }
}

Write-Host '=== STAGE 7 FREEZE GATE: PASS ==='
Write-Host 'Next: remove temporary PROC/PAGER diagnostics, rerun the clean 1/2/4 matrix, then freeze Stage 7.0.'

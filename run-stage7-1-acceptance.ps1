$ErrorActionPreference = 'Stop'

Write-Host '=== WovenHat Stage 7.1 CPU Affinity Acceptance ==='

Write-Host '1/5 Build + lint + host tests'
cargo build
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
cargo clippy -p wovenhat-kernel -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'cargo clippy failed' }
cargo test
if ($LASTEXITCODE -ne 0) { throw 'cargo test failed' }

function Run-MemoryStress([int]$Cpu, [int]$Runs) {
    $logDir = ".\stage7-1-${Cpu}cpu-logs"
    Remove-Item $logDir -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Path $logDir -Force | Out-Null
    $failures = @()
    1..$Runs | ForEach-Object {
        Write-Host "=== STAGE 7.1 MEMORY $Cpu CPU RUN $_ OF $Runs ==="
        Get-Process qemu-system-x86_64 -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Milliseconds 300
        python .\scripts\test-memory-qemu.py --cpus $Cpu
        $code = $LASTEXITCODE
        $src = ".\target\memory-regression-$Cpu-debug\serial.log"
        $dst = Join-Path $logDir "run-$_.log"
        if (Test-Path $src) { Copy-Item $src $dst -Force }
        if ($code -ne 0) { $failures += $_ }
    }
    if ($failures.Count -ne 0) {
        Write-Host "FAILED $Cpu-CPU runs: $($failures -join ', ')"
        throw "Stage 7.1 memory stress failed on $Cpu CPU(s)"
    }
}

Write-Host '2/5 1-CPU baseline stress (20 runs)'
Run-MemoryStress 1 20

Write-Host '3/5 2-CPU affinity/SMP stress (100 runs)'
Run-MemoryStress 2 100

Write-Host '4/5 4-CPU affinity/SMP stress (50 runs)'
Run-MemoryStress 4 50

Write-Host '5/5 Network acceptance 1/2/4 CPUs'
foreach ($cpu in 1,2,4) {
    python .\scripts\test-network-qemu.py --cpus $cpu
    if ($LASTEXITCODE -ne 0) { throw "network test failed on $cpu CPU(s)" }
}

Write-Host '=== STAGE 7.1 ACCEPTANCE: PASS ==='
Write-Host 'CPU affinity foundation is cleared. Proceed to Stage 7.2 scheduler parking primitive.'

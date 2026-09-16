$ErrorActionPreference = 'Stop'

Write-Host '=== WovenHat Stage 7.2 Termination/Lifecycle Hardening Acceptance ==='

$artifactRoot = '.\stage7-2-artifacts'
Remove-Item $artifactRoot -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $artifactRoot -Force | Out-Null

function Invoke-Checked([string]$Label, [scriptblock]$Command) {
    Write-Host "`n=== $Label ==="
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$Label failed with exit code $LASTEXITCODE" }
}

function Save-Failure([int]$Cpu, [int]$Run, [string]$Suite) {
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $dir = Join-Path $artifactRoot "FAIL-$Suite-${Cpu}cpu-run-$Run-$stamp"
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    $serial = ".\target\memory-regression-$Cpu-debug\serial.log"
    if (Test-Path $serial) {
        Copy-Item $serial (Join-Path $dir 'serial.log') -Force
        Select-String -Path $serial -Pattern 'TERM|panic|assertion|page fault|#PF|exception|scheduler|Switching|Blocked|Sleeping|Ready|Running|terminated' |
            Out-File (Join-Path $dir 'diagnostic-summary.txt')
    }
    Write-Host "Failure artifacts preserved in: $dir"
}

function Run-MemoryStress([int]$Cpu, [int]$Runs) {
    $logDir = Join-Path $artifactRoot "memory-${Cpu}cpu"
    New-Item -ItemType Directory -Path $logDir -Force | Out-Null
    for ($run = 1; $run -le $Runs; $run++) {
        Write-Host "=== MEMORY $Cpu CPU RUN $run OF $Runs ==="
        Get-Process qemu-system-x86_64 -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Milliseconds 150
        python .\scripts\test-memory-qemu.py --cpus $Cpu
        $code = $LASTEXITCODE
        $src = ".\target\memory-regression-$Cpu-debug\serial.log"
        if (Test-Path $src) { Copy-Item $src (Join-Path $logDir "run-$run.log") -Force }
        if ($code -ne 0) {
            Save-Failure $Cpu $run 'memory'
            throw "Stage 7.2 memory test failed on $Cpu CPU(s), run $run"
        }
        if (-not (Select-String -Path $src -SimpleMatch '[TERM] scheduler-owned Ready-task termination + deferred reap: PASSED' -Quiet)) {
            Save-Failure $Cpu $run 'termination-marker'
            throw "Stage 7.2 termination smoke marker missing on $Cpu CPU(s), run $run"
        }
    }
}

function Run-Network([int]$Cpu) {
    Write-Host "=== NETWORK $Cpu CPU ==="
    python .\scripts\test-network-qemu.py --cpus $Cpu
    if ($LASTEXITCODE -ne 0) { throw "Stage 7.2 network test failed on $Cpu CPU(s)" }
}

Invoke-Checked '1/6 cargo build' { cargo build }
Invoke-Checked '2/6 cargo clippy -D warnings' { cargo clippy -p wovenhat-kernel -- -D warnings }
Invoke-Checked '3/6 cargo test' { cargo test }

# Stage 7.1 already passed the much larger 50/100/50 freeze matrix. Stage 7.2
# intentionally uses a smaller regression matrix plus a new deterministic
# termination marker so validation is focused rather than repetitive.
Write-Host "`n=== 4/6 termination + 1-CPU regression: 10 runs ==="
Run-MemoryStress 1 10
Write-Host "`n=== 5/6 SMP regression: 2 CPU x25, 4 CPU x10 ==="
Run-MemoryStress 2 25
Run-MemoryStress 4 10
Write-Host "`n=== 6/6 network regression: 1/2/4 CPUs ==="
foreach ($cpu in 1,2,4) { Run-Network $cpu }

Write-Host "`n=== STAGE 7.2 ACCEPTANCE: PASS ==="
Write-Host 'Scheduler-owned termination, deferred zombie cleanup, SMP regression, and network regression passed.'

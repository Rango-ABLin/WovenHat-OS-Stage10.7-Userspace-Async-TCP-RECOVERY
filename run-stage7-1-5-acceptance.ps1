$ErrorActionPreference = 'Stop'

Write-Host '=== WovenHat Stage 7.1.5 Scheduler/Pager Correctness Acceptance ==='

$artifactRoot = '.\stage7-1-5-artifacts'
Remove-Item $artifactRoot -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $artifactRoot -Force | Out-Null

function Invoke-Checked([string]$Label, [scriptblock]$Command) {
    Write-Host "`n=== $Label ==="
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Label failed with exit code $LASTEXITCODE"
    }
}

function Save-Failure([int]$Cpu, [int]$Run, [string]$Suite) {
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $dir = Join-Path $artifactRoot "FAIL-$Suite-${Cpu}cpu-run-$Run-$stamp"
    New-Item -ItemType Directory -Path $dir -Force | Out-Null

    $serial = ".\target\memory-regression-$Cpu-debug\serial.log"
    if (Test-Path $serial) {
        Copy-Item $serial (Join-Path $dir 'serial.log') -Force
        Select-String -Path $serial -Pattern 'PAGER-DIAG|YIELD-TRACE|panic|assertion|page fault|#PF|exception|scheduler|Switching|Blocked|Sleeping|Ready|Running' |
            Out-File (Join-Path $dir 'diagnostic-summary.txt')
    }

    Write-Host "`nFAILED: $Suite, CPU=$Cpu, run=$Run"
    Write-Host "Failure artifacts preserved in: $dir"
    Write-Host 'Do not rerun manually before inspecting that directory.'
}

function Run-MemoryStress([int]$Cpu, [int]$Runs) {
    $logDir = Join-Path $artifactRoot "memory-${Cpu}cpu"
    New-Item -ItemType Directory -Path $logDir -Force | Out-Null

    for ($run = 1; $run -le $Runs; $run++) {
        Write-Host "=== MEMORY $Cpu CPU RUN $run OF $Runs ==="
        Get-Process qemu-system-x86_64 -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Milliseconds 250

        python .\scripts\test-memory-qemu.py --cpus $Cpu
        $code = $LASTEXITCODE
        $src = ".\target\memory-regression-$Cpu-debug\serial.log"
        if (Test-Path $src) {
            Copy-Item $src (Join-Path $logDir "run-$run.log") -Force
        }

        if ($code -ne 0) {
            Save-Failure $Cpu $run 'memory'
            throw "Stage 7.1.5 memory stress failed on $Cpu CPU(s), run $run"
        }
    }
}

function Run-Network([int]$Cpu) {
    Write-Host "=== NETWORK $Cpu CPU ==="
    python .\scripts\test-network-qemu.py --cpus $Cpu
    if ($LASTEXITCODE -ne 0) {
        $dir = Join-Path $artifactRoot "FAIL-network-${Cpu}cpu"
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
        $candidateLogs = Get-ChildItem .\target -Recurse -Filter serial.log -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending |
            Select-Object -First 3
        foreach ($log in $candidateLogs) {
            Copy-Item $log.FullName (Join-Path $dir $log.Name) -Force
        }
        throw "Stage 7.1.5 network test failed on $Cpu CPU(s)"
    }
}

Invoke-Checked '1/6 cargo build' { cargo build }
Invoke-Checked '2/6 cargo clippy -D warnings' { cargo clippy -p wovenhat-kernel -- -D warnings }
Invoke-Checked '3/6 cargo test' { cargo test }

Write-Host "`n=== 4/6 1-CPU correctness stress: 50 runs ==="
Run-MemoryStress 1 50

Write-Host "`n=== 5/6 SMP stress: 2 CPU x100, 4 CPU x50 ==="
Run-MemoryStress 2 100
Run-MemoryStress 4 50

Write-Host "`n=== 6/6 network: 1/2/4 CPUs ==="
foreach ($cpu in 1,2,4) {
    Run-Network $cpu
}

Write-Host "`n=== STAGE 7.1.5 ACCEPTANCE: PASS ==="
Write-Host 'Scheduler/pager correctness gate passed. Keep artifacts as the freeze record.'

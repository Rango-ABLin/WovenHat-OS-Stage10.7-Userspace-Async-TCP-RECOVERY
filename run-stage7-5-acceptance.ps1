$ErrorActionPreference = 'Stop'

Write-Host '=== WovenHat Stage 7.5 Multicore Pager & File-I/O Ownership Acceptance ==='

$artifactRoot = '.\stage7-5-artifacts'
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
        Select-String -Path $serial -Pattern 'S7.3|S7.4|S7.5|PAGER|TERM|panic|assertion|page fault|#PF|exception|scheduler|Switching|Blocked|Sleeping|Ready|Running|terminated' |
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
            throw "Stage 7.5 memory test failed on $Cpu CPU(s), run $run"
        }
        if (-not (Select-String -Path $src -SimpleMatch '[TERM] scheduler-owned Ready-task termination + deferred reap: PASSED' -Quiet)) {
            Save-Failure $Cpu $run 'termination-marker'
            throw "Stage 7.5 regression: Stage 7.2 termination marker missing on $Cpu CPU(s), run $run"
        }
        if ($Cpu -eq 1) {
            if (-not (Select-String -Path $src -SimpleMatch '[S7.3] single-CPU baseline preserved; no AP Ring-3 probe required' -Quiet)) {
                Save-Failure $Cpu $run 'stage7-3-marker'
                throw "Stage 7.5 regression: Stage 7.3 single-CPU preservation marker missing on run $run"
            }
        }
        else {
            if (-not (Select-String -Path $src -SimpleMatch '[S7.3] pinned Ring-3 execution on every AP: PASSED' -Quiet)) {
                Save-Failure $Cpu $run 'stage7-3-marker'
                throw "Stage 7.5 regression: Stage 7.3 AP Ring-3 marker missing on $Cpu CPU(s), run $run"
            }
        }
        if ($Cpu -eq 1) {
            if (-not (Select-String -Path $src -SimpleMatch '[S7.4] single-CPU baseline preserved; no Ring-3 migration required' -Quiet)) {
                Save-Failure $Cpu $run 'stage7-4-marker'
                throw "Stage 7.5 regression: Stage 7.4 single-CPU preservation marker missing on run $run"
            }
        }
        else {
            if (-not (Select-String -Path $src -SimpleMatch '[S7.4] Ready-state Ring-3 migration + affinity: PASSED' -Quiet)) {
                Save-Failure $Cpu $run 'stage7-4-marker'
                throw "Stage 7.5 regression: Stage 7.4 migration/affinity marker missing on $Cpu CPU(s), run $run"
            }
        }
        if ($Cpu -eq 1) {
            if (-not (Select-String -Path $src -SimpleMatch '[S7.5] single-CPU pager/file-I/O baseline preserved' -Quiet)) {
                Save-Failure $Cpu $run 'stage7-5-marker'
                throw "Stage 7.5 single-CPU pager/file-I/O preservation marker missing on run $run"
            }
        }
        else {
            if (-not (Select-String -Path $src -SimpleMatch '[S7.5] pinned AP pager/file-I/O: PASSED cpu=' -Quiet)) {
                Save-Failure $Cpu $run 'stage7-5-marker'
                throw "Stage 7.5 pinned AP pager/file-I/O marker missing on $Cpu CPU(s), run $run"
            }
        }
    }
}

function Run-Network([int]$Cpu) {
    Write-Host "=== NETWORK $Cpu CPU ==="
    python .\scripts\test-network-qemu.py --cpus $Cpu
    if ($LASTEXITCODE -ne 0) { throw "Stage 7.5 network regression failed on $Cpu CPU(s)" }
}

Invoke-Checked '1/6 cargo build' { cargo build }
Invoke-Checked '2/6 cargo clippy -D warnings' { cargo clippy -p wovenhat-kernel -- -D warnings }
Invoke-Checked '3/6 cargo test' { cargo test }

# Stage 7.4 is the validated baseline. Stage 7.5 changes only the
# interrupt-enabled file-I/O preemption guard from global/BSP-specific state to
# per-CPU state, then pins the existing mmap/pager workload to an AP. Normal
# userspace placement and automatic balancing remain unchanged.
Write-Host "`n=== 4/6 Stage 7.4 preservation + 1-CPU regression: 10 runs ==="
Run-MemoryStress 1 10
Write-Host "`n=== 5/6 Stage 7.5 AP pager/file-I-O regression: 2 CPU x20, 4 CPU x10 ==="
Run-MemoryStress 2 20
Run-MemoryStress 4 10
Write-Host "`n=== 6/6 network regression: 1/2/4 CPUs ==="
foreach ($cpu in 1,2,4) { Run-Network $cpu }

Write-Host "`n=== STAGE 7.5 ACCEPTANCE: PASS ==="
Write-Host 'Stage 7.4 Ring-3 migration guarantees preserved; per-CPU file-I/O preemption guard and pinned AP pager/file-I/O execution passed.'

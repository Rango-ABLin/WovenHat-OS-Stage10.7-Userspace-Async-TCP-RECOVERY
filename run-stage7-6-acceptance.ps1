$ErrorActionPreference = 'Stop'

Write-Host '=== WovenHat Stage 7.6 Integrated Multicore Userspace Closure Acceptance ==='

$artifactRoot = Join-Path '.\stage7-6-artifacts' (Get-Date -Format 'yyyyMMdd-HHmmss-fff')
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
        Select-String -Path $serial -Pattern 'S7.3|S7.4|S7.5|S7.6|PAGER|TERM|panic|assertion|page fault|#PF|exception|scheduler|Switching|Blocked|Sleeping|Ready|Running|terminated' |
            Out-File (Join-Path $dir 'diagnostic-summary.txt')
    }
    Write-Host "Failure artifacts preserved in: $dir"
}

function Run-MemoryStress([int]$Cpu, [int]$Runs) {
    $logDir = Join-Path $artifactRoot "memory-${Cpu}cpu"
    New-Item -ItemType Directory -Path $logDir -Force | Out-Null
    for ($run = 1; $run -le $Runs; $run++) {
        Write-Host "=== MEMORY $Cpu CPU RUN $run OF $Runs ==="
        python .\scripts\test-memory-qemu.py --cpus $Cpu
        $code = $LASTEXITCODE
        $src = ".\target\memory-regression-$Cpu-debug\serial.log"
        if (Test-Path $src) { Copy-Item $src (Join-Path $logDir "run-$run.log") -Force }
        $qemuLog = ".\target\memory-regression-$Cpu-debug\qemu.log"
        if (Test-Path $qemuLog) { Copy-Item $qemuLog (Join-Path $logDir "run-$run-qemu.log") -Force }
        if ($code -ne 0) {
            Save-Failure $Cpu $run 'memory'
            throw "Stage 7.6 memory test failed on $Cpu CPU(s), run $run"
        }
        if (-not (Select-String -Path $src -SimpleMatch '[TERM] scheduler-owned Ready-task termination + deferred reap: PASSED' -Quiet)) {
            Save-Failure $Cpu $run 'termination-marker'
            throw "Stage 7.6 regression: Stage 7.2 termination marker missing on $Cpu CPU(s), run $run"
        }
        if ($Cpu -eq 1) {
            if (-not (Select-String -Path $src -SimpleMatch '[S7.3] single-CPU baseline preserved; no AP Ring-3 probe required' -Quiet)) {
                Save-Failure $Cpu $run 'stage7-3-marker'
                throw "Stage 7.6 regression: Stage 7.3 single-CPU preservation marker missing on run $run"
            }
        }
        else {
            if (-not (Select-String -Path $src -SimpleMatch '[S7.3] pinned Ring-3 execution on every AP: PASSED' -Quiet)) {
                Save-Failure $Cpu $run 'stage7-3-marker'
                throw "Stage 7.6 regression: Stage 7.3 AP Ring-3 marker missing on $Cpu CPU(s), run $run"
            }
        }
        if ($Cpu -eq 1) {
            if (-not (Select-String -Path $src -SimpleMatch '[S7.4] single-CPU baseline preserved; no Ring-3 migration required' -Quiet)) {
                Save-Failure $Cpu $run 'stage7-4-marker'
                throw "Stage 7.6 regression: Stage 7.4 single-CPU preservation marker missing on run $run"
            }
        }
        else {
            if (-not (Select-String -Path $src -SimpleMatch '[S7.4] Ready-state Ring-3 migration + affinity: PASSED' -Quiet)) {
                Save-Failure $Cpu $run 'stage7-4-marker'
                throw "Stage 7.6 regression: Stage 7.4 migration/affinity marker missing on $Cpu CPU(s), run $run"
            }
        }
        if ($Cpu -eq 1) {
            if (-not (Select-String -Path $src -SimpleMatch '[S7.5] single-CPU pager/file-I/O baseline preserved' -Quiet)) {
                Save-Failure $Cpu $run 'stage7-5-marker'
                throw "Stage 7.6 regression: Stage 7.5 single-CPU pager/file-I/O preservation marker missing on run $run"
            }
        }
        else {
            if (-not (Select-String -Path $src -SimpleMatch '[S7.5] pinned AP pager/file-I/O: PASSED cpu=' -Quiet)) {
                Save-Failure $Cpu $run 'stage7-5-marker'
                throw "Stage 7.6 regression: Stage 7.5 pinned AP pager/file-I/O marker missing on $Cpu CPU(s), run $run"
            }
        }
        if ($Cpu -eq 1) {
            if (-not (Select-String -Path $src -SimpleMatch '[S7.6] single-CPU integrated userspace baseline preserved' -Quiet)) {
                Save-Failure $Cpu $run 'stage7-6-marker'
                throw "Stage 7.6 single-CPU integrated-userspace marker missing on run $run"
            }
            if (-not (Select-String -Path $src -SimpleMatch '[S7.6] fork-affinity baseline preserved on single CPU' -Quiet)) {
                Save-Failure $Cpu $run 'stage7-6-fork-marker'
                throw "Stage 7.6 single-CPU fork-affinity marker missing on run $run"
            }
        }
        else {
            foreach ($marker in @(
                '[S7.6] automatic Ready Ring-3 rebalancing: PASSED',
                '[S7.6] least-loaded multicore Ring-3 placement: PASSED cpu=',
                '[S7.6] AP fork CPU/affinity inheritance: PASSED cpu=',
                '[S7.6] bounded integrated multicore userspace foundation: PASSED'
            )) {
                if (-not (Select-String -Path $src -SimpleMatch $marker -Quiet)) {
                    Save-Failure $Cpu $run 'stage7-6-marker'
                    throw "Stage 7.6 marker missing on $Cpu CPU(s), run $run : $marker"
                }
            }
        }
    }
}

function Run-Network([int]$Cpu) {
    Write-Host "=== NETWORK $Cpu CPU ==="
    python .\scripts\test-network-qemu.py --cpus $Cpu
    if ($LASTEXITCODE -ne 0) { throw "Stage 7.6 network regression failed on $Cpu CPU(s)" }
}

Invoke-Checked '1/6 cargo build' { cargo build }
Invoke-Checked '2/6 cargo clippy -D warnings' { cargo clippy -p wovenhat-kernel -- -D warnings }
Invoke-Checked '3/6 cargo test' { cargo test }

# Stage 7.5 is the validated baseline. Stage 7.6 integrates only the
# already-proven Ring-3 capabilities: opt-in multicore placement, Ready-only
# automatic userspace rebalancing, and fork CPU/affinity inheritance. Legacy
# userspace remains CPU0-owned unless explicitly spawned through the multicore API.
Write-Host "`n=== 4/6 Stage 7.5 preservation + 1-CPU regression: 10 runs ==="
Run-MemoryStress 1 10
Write-Host "`n=== 5/6 Stage 7.6 integrated userspace closure: 2 CPU x30, 4 CPU x20 ==="
Run-MemoryStress 2 30
Run-MemoryStress 4 20
Write-Host "`n=== 6/6 network regression: 1/2/4 CPUs ==="
foreach ($cpu in 1,2,4) { Run-Network $cpu }

Write-Host "`n=== STAGE 7.6 ACCEPTANCE: PASS ==="
Write-Host 'Stage 7.5 pager guarantees preserved; opt-in multicore Ring-3 placement, automatic Ready userspace rebalancing, fork affinity inheritance and network regression passed.'

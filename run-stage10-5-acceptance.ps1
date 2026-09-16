$ErrorActionPreference = 'Stop'
Write-Host '=== WovenHat Stage 10.5 Userspace Async VFS/File I/O Acceptance ==='
Write-Host ''
Write-Host '=== 1/2 Preserve the complete validated Stage 10.4 baseline ==='
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage10-4-acceptance.ps1
if ($LASTEXITCODE -ne 0) { throw "Stage 10.4 preservation acceptance failed with exit code $LASTEXITCODE" }
Write-Host ''
Write-Host '=== 2/2 Isolated Stage 10.5 Ring-3 VFS async I/O boots: 1/2/4 CPUs ==='
foreach ($cpu in 1,2,4) {
    python .\scripts\test-memory-qemu.py --cpus $cpu --stage10-5
    if ($LASTEXITCODE -ne 0) { throw "Stage 10.5 isolated boot failed on $cpu CPU(s)" }
    $log = ".\target\stage10-5-regression-$cpu-debug\serial.log"
    if (!(Select-String -Path $log -SimpleMatch '[S10.5] userspace async VFS read/write + fd pinning/cancel/teardown: PASSED' -Quiet)) {
        throw "Stage 10.5 marker missing on $cpu CPU(s): $log"
    }
    Write-Host "Stage 10.5 userspace async VFS I/O marker: PASS ($cpu CPU)"
}
Write-Host ''
Write-Host '=== STAGE 10.5 ACCEPTANCE: PASS ==='
Write-Host 'Stage 10.4 preserved; ordinary User-domain Ring-3 apps now submit positional async VFS reads/writes with pinned open-file descriptions, kernel-owned buffers, cancellation, retry-safe completion copyout, and deterministic teardown on 1/2/4 CPUs.'

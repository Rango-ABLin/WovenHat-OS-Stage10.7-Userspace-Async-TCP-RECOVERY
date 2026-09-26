$ErrorActionPreference = 'Stop'

cargo build --features stage13-8-test
if ($LASTEXITCODE -ne 0) { throw 'Build failed' }

cargo clippy -p wovenhat-kernel --target x86_64-unknown-none --features stage13-8-test -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'Kernel lint failed' }

$passes = 3
foreach ($cpu in 1,2,4) {
    foreach ($run in 1..$passes) {
        Write-Host "=== Stage 13.8H stress: CPU=$cpu run=$run/$passes ==="
        python .\scripts\test-stage10-runtime.py --stage 13.8 --cpus $cpu --timeout 120
        if ($LASTEXITCODE -ne 0) {
            throw "Stage 13.8H stress failed on $cpu CPU(s), run $run"
        }
    }
}
Write-Host '=== STAGE 13.8H HARDENING/STRESS: PASS (9/9 boots) ==='
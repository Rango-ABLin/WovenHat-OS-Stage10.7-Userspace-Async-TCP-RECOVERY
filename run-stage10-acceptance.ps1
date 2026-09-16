$ErrorActionPreference = 'Stop'
Write-Host '=== WovenHat Stage 10 aggregate acceptance ==='
foreach ($stage in '10.8','10.9') {
    foreach ($cpu in 1,2,4) {
        python .\scripts\test-stage10-runtime.py --stage $stage --cpus $cpu --timeout 60
        if ($LASTEXITCODE -ne 0) { throw "Stage $stage failed on $cpu CPUs" }
    }
}
Write-Host '=== STAGE 10 AGGREGATE ACCEPTANCE: PASS ==='

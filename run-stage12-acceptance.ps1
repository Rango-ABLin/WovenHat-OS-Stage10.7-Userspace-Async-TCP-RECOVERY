$ErrorActionPreference = 'Stop'
foreach ($stage in '12.1','12.2','12.3','12.4','12.5') {
    foreach ($cpu in 1,2,4) {
        python .\scripts\test-stage10-runtime.py --stage $stage --cpus $cpu --timeout 60
        if ($LASTEXITCODE -ne 0) { throw "Stage $stage failed on $cpu CPUs" }
    }
}
Write-Host '=== STAGE 12 ACCEPTANCE: PASS ==='

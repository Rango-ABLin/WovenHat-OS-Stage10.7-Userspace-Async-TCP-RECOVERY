$ErrorActionPreference = 'Stop'
foreach ($stage in '11.3','11.4','11.5') {
    foreach ($cpu in 1,2,4) {
        python .\scripts\test-stage10-runtime.py --stage $stage --cpus $cpu --timeout 60
        if ($LASTEXITCODE -ne 0) { throw "Stage $stage failed on $cpu CPUs" }
    }
}
Write-Host '=== STAGES 11.3-11.5 ACCEPTANCE: PASS ==='

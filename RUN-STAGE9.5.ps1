$ErrorActionPreference = "Stop"
$env:CARGO_NET_OFFLINE = "true"
Write-Host "=== WovenHat Stage 9.5 clean launcher ==="
Write-Host "Working directory: $(Get-Location)"
Write-Host "Running the complete Stage 9.5 acceptance chain..."
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-5-acceptance.ps1
if ($LASTEXITCODE -ne 0) { throw "Stage 9.5 acceptance failed with exit code $LASTEXITCODE" }

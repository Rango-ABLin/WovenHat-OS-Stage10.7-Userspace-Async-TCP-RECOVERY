$ErrorActionPreference = "Stop"
$env:CARGO_NET_OFFLINE = "true"
Write-Host "=== WovenHat Stage 10.1 clean launcher ==="
Write-Host "Working directory: $(Get-Location)"
Write-Host "Running the complete Stage 10.1 acceptance chain..."
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage10-1-acceptance.ps1
if ($LASTEXITCODE -ne 0) { throw "Stage 10.1 acceptance failed with exit code $LASTEXITCODE" }

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
Write-Host "=== WovenHat Stage 10.4 clean launcher ==="
Write-Host "Working directory: $PWD"
Write-Host "Running the complete Stage 10.4 acceptance chain..."
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage10-4-acceptance.ps1
if ($LASTEXITCODE -ne 0) { throw "Stage 10.4 acceptance failed with exit code $LASTEXITCODE" }

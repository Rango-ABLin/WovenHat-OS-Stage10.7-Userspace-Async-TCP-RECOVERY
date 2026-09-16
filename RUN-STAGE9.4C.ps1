$ErrorActionPreference = 'Stop'
$env:CARGO_NET_OFFLINE = 'true'
Write-Host '=== WovenHat Stage 9.4C clean launcher ==='
Write-Host "Working directory: $(Get-Location)"
Write-Host 'Running the complete Stage 9.4C acceptance chain...'
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-4c-acceptance.ps1
if ($LASTEXITCODE -ne 0) {
    throw "Stage 9.4C acceptance failed with exit code $LASTEXITCODE"
}

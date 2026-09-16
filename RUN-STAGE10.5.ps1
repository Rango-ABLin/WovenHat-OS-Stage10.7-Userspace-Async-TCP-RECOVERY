$ErrorActionPreference = 'Stop'
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage10-5-acceptance.ps1
if ($LASTEXITCODE -ne 0) { throw "Stage 10.5 acceptance failed with exit code $LASTEXITCODE" }

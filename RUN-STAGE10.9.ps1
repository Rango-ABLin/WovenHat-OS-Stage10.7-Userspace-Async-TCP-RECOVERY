$ErrorActionPreference = 'Stop'
$env:CARGO_BUILD_BUILD_DIR = Join-Path $PSScriptRoot '.cargo-build'
$env:CARGO_TARGET_DIR = Join-Path $PSScriptRoot 'target'
New-Item -ItemType Directory -Force -Path $env:CARGO_BUILD_BUILD_DIR,$env:CARGO_TARGET_DIR | Out-Null
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'run-stage10-9-acceptance.ps1')
if ($LASTEXITCODE -ne 0) { throw "Stage 10.9 acceptance failed with exit code $LASTEXITCODE" }

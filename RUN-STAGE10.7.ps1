$ErrorActionPreference = "Stop"

# Keep Cargo/bootloader intermediates inside the project. Windows cleanup of
# %TEMP% previously removed cargo-install* directories during the bootloader
# build and falsely failed the entire preservation chain.
$env:CARGO_BUILD_BUILD_DIR = Join-Path $PSScriptRoot ".cargo-build"
$env:CARGO_TARGET_DIR = Join-Path $PSScriptRoot "target"
New-Item -ItemType Directory -Force -Path $env:CARGO_BUILD_BUILD_DIR | Out-Null
New-Item -ItemType Directory -Force -Path $env:CARGO_TARGET_DIR | Out-Null

Write-Host "Cargo build dir: $env:CARGO_BUILD_BUILD_DIR"
Write-Host "Cargo target dir: $env:CARGO_TARGET_DIR"

& powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "run-stage10-7-acceptance.ps1")
if ($LASTEXITCODE -ne 0) { throw "Stage 10.7 acceptance failed with exit code $LASTEXITCODE" }

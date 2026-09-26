$ErrorActionPreference = 'Stop'
$env:CARGO_BUILD_BUILD_DIR = Join-Path $PSScriptRoot '.cargo-build'
$env:CARGO_TARGET_DIR = Join-Path $PSScriptRoot 'target'
Push-Location $PSScriptRoot
try {
    python .\scripts\test-stage13-10ac.py
    if ($LASTEXITCODE -ne 0) { throw 'Stage 13.10AC software acceptance failed; see retained evidence.' }
}
finally { Pop-Location }

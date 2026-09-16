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

$auditRoot = Join-Path $PSScriptRoot ('audit-artifacts/acceptance-' + (Get-Date -Format 'yyyyMMdd-HHmmss-fff'))
New-Item -ItemType Directory -Force -Path $auditRoot | Out-Null
Start-Transcript -Path (Join-Path $auditRoot 'acceptance.txt') | Out-Null
Push-Location $PSScriptRoot
try {
    # Windows PowerShell transcripts omit redirected native-child output.
    # Tee it explicitly; use the child's exit code as the failure authority.
    $ErrorActionPreference = 'Continue'
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "run-stage10-7-acceptance.ps1") 2>&1 |
        ForEach-Object { $_.ToString() } |
        Tee-Object -FilePath (Join-Path $auditRoot 'acceptance-output.txt') | Out-Host
    $gateExitCode = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($gateExitCode -ne 0) { throw "Stage 10.7 acceptance failed with exit code $gateExitCode" }
}
finally {
    Get-ChildItem -Path (Join-Path $PSScriptRoot 'target') -Directory -Filter '*regression*' | ForEach-Object {
        $destination = Join-Path $auditRoot $_.Name
        New-Item -ItemType Directory -Force -Path $destination | Out-Null
        Get-ChildItem -LiteralPath $_.FullName -File -Filter '*.log' | Copy-Item -Destination $destination
    }
    Pop-Location
    Stop-Transcript | Out-Null
}

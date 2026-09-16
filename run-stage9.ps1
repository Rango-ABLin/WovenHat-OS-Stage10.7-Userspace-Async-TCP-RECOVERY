$ErrorActionPreference = "Stop"

# Ask the normal Cargo target for its image; test VMs can change image mtimes.
$imagePath = & cargo run --quiet -- --print-image
if ($LASTEXITCODE -ne 0) { throw "Failed to build the normal WovenHat image." }
if (-not $imagePath -or -not (Test-Path -LiteralPath $imagePath)) {
    throw "The normal build did not report an existing UEFI image."
}
$img = Get-Item -LiteralPath $imagePath

$dataImage = Join-Path $PSScriptRoot "wovenhat-disk.img"
if (-not (Test-Path -LiteralPath $dataImage)) {
    & python (Join-Path $PSScriptRoot "scripts/create-fat32.py") $dataImage
    if ($LASTEXITCODE -ne 0) { throw "Failed to create the FAT32 data disk." }
}

Write-Host "Booting WovenHat OS 0.7.0 Stage 9: $($img.FullName)"

& "C:\Program Files\qemu\qemu-system-x86_64.exe" `
    -machine pc `
    -m 1024M `
    -smp 2 `
    -drive "if=pflash,format=raw,readonly=on,file=C:\Program Files\qemu\share\edk2-x86_64-code.fd" `
    -drive "if=pflash,format=raw,file=$PWD\OVMF_VARS.fd" `
    -drive "if=none,id=wovenhatboot,format=raw,file=$($img.FullName)" `
    -device virtio-blk-pci,drive=wovenhatboot,bootindex=1 `
    -drive "if=ide,index=0,format=raw,file=$dataImage" `
    -device virtio-net-pci,netdev=net0,disable-modern=on `
    -netdev user,id=net0 `
    -boot menu=on `
    -serial stdio `
    -no-reboot `
    -no-shutdown

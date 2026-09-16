param(
    [ValidateSet(1, 2, 4)][int]$Cpus = 4,
    [string]$Qemu = "C:\Program Files\qemu\qemu-system-x86_64.exe"
)
$ErrorActionPreference = "Stop"
Push-Location $PSScriptRoot
try {
    $releaseImage = & cargo run --quiet --release -- --print-image
    if ($LASTEXITCODE -ne 0) { throw "Release build failed." }
    $releaseDisk = Join-Path $PSScriptRoot "wovenhat-disk.img"
    & python (Join-Path $PSScriptRoot "scripts/create-fat32.py") $releaseDisk
    if ($LASTEXITCODE -ne 0) { throw "Data disk setup failed." }
    $releaseFirmware = Join-Path (Split-Path $Qemu) "share/edk2-x86_64-code.fd"
    & $Qemu -machine pc -m 1024M -smp $Cpus `
        -drive "if=pflash,format=raw,readonly=on,file=$releaseFirmware" `
        -drive "if=none,id=boot,format=raw,readonly=on,file=$releaseImage" `
        -device virtio-blk-pci,drive=boot,bootindex=1 `
        -drive "if=ide,index=0,format=raw,file=$releaseDisk" `
        -device virtio-net-pci,netdev=net0,disable-modern=on `
        -netdev user,id=net0 -serial stdio -no-reboot
} finally { Pop-Location }

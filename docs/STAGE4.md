# WovenHat OS 0.2.0 — Stage 4

Stage 4 keeps the shell-first boot path and adds two runtime capabilities without making either one fatal to boot:

- persistent FAT32-backed storage mounted at `/mnt`
- a persistent smoltcp socket set with a real UDP echo service

## Storage model

The UEFI boot image should be attached as a VirtIO block device. A separate QEMU `vvfat` IDE disk is used as the ATA primary master. The WovenHat ATA PIO driver mounts that disk into `/mnt`.

On Windows create the host directory once:

```powershell
New-Item -ItemType Directory -Force .\wovenhat-data | Out-Null
```

Files placed in that directory are visible to WovenHat through `/mnt` (subject to the current FAT 8.3 implementation). Kernel-shell writes can be persisted explicitly:

```
write /mnt/HELLO.TXT hello
persist /mnt/HELLO.TXT
sync
```

`persist` accepts files and directories. FAT 8.3 names are still required by the current writer.

## Networking

Stage 4 keeps the transitional VirtIO-net transport and now keeps a persistent `SocketSet`. Start the kernel UDP echo service with:

```
udpecho 7
netstat
```

The service listens at `10.0.2.15:<port>`. `network::poll()` services the socket continuously from the shell-first event loop.

This is a kernel networking service, not yet a Berkeley socket ABI for arbitrary userspace programs. Socket syscalls, DHCP, and DNS remain subsequent work.

## Recommended QEMU topology

Use the UEFI image as VirtIO block and the writable data directory as the ATA primary disk:

```powershell
$img = Get-ChildItem .\target -Recurse -Filter "wovenhat-os-uefi.img" |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

New-Item -ItemType Directory -Force .\wovenhat-data | Out-Null

& "C:\Program Files\qemu\qemu-system-x86_64.exe" `
    -machine q35 `
    -m 1024M `
    -smp 2 `
    -drive "if=pflash,format=raw,readonly=on,file=C:\Program Files\qemu\share\edk2-x86_64-code.fd" `
    -drive "if=pflash,format=raw,file=$PWD\OVMF_VARS.fd" `
    -drive "if=virtio,format=raw,file=$($img.FullName)" `
    -drive "if=ide,index=0,format=raw,file=fat:rw:$PWD\wovenhat-data" `
    -device virtio-net-pci,netdev=net0,disable-modern=on `
    -netdev user,id=net0 `
    -serial stdio `
    -no-reboot `
    -no-shutdown
```

If the local QEMU build rejects `fat:rw:` with this syntax, boot without the data drive; the RAM VFS and shell still work, and WovenHat logs the storage failure without halting.

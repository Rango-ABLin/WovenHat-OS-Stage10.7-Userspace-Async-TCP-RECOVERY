# WovenHat OS 0.3.0 — Stage 5 Userspace Networking

Stage 5 makes the Stage 3/4 VirtIO + smoltcp stack available to Ring-3 programs while preserving the shell-first recovery path and the separate writable FAT data disk mounted at `/mnt`.

## What Stage 5 adds

- Process-owned UDP and TCP sockets backed by smoltcp `SocketSet` handles.
- Automatic socket teardown when a process exits.
- Userspace ABI calls for socket open/bind/connect/send/receive/close and peer lookup.
- Network information ABI exposing IPv4 address, gateway, DNS, MAC, DHCP state and link state.
- DHCPv4 client with the known-good QEMU static topology as a recovery fallback.
- Asynchronous DNS A-record queries using the QEMU user-network DNS proxy (`10.0.2.3`).
- ICMP echo request/reply support with a five-second timeout.
- Built-in Ring-3 network probes: `/bin/ip`, `/bin/netstat`, `/bin/dns`, `/bin/udp`, `/bin/nc`, `/bin/ping`.
- Kernel shell command `dhcp on|off` plus expanded `net` and `netstat` diagnostics.

## Syscall ABI additions

| Number | Name | arg0 | arg1 | arg2 | Result |
|---:|---|---|---|---|---|
| 37 | Socket | kind (1 UDP, 2 TCP) | - | - | process-local socket id |
| 38 | Bind | socket id | port | - | 0 or error |
| 39 | Connect | socket id | packed IPv4 endpoint | - | 0 or error |
| 40 | NetSend | socket id | user buffer | length | bytes queued or error |
| 41 | NetRecv | socket id | user buffer | capacity | bytes, WOULD_BLOCK, or error |
| 42 | NetClose | socket id | - | - | 0 or error |
| 43 | NetInfo | user buffer | - | - | bytes copied |
| 44 | DnsStart | hostname pointer | hostname length | - | query id |
| 45 | DnsPoll | query id | four-byte IPv4 output | - | 0 pending, 1 ready, error |
| 46 | NetPeer | socket id | - | - | packed endpoint |
| 47 | Dhcp | 0 off / nonzero on | - | - | 0 or error |
| 48 | PingStart | IPv4 in low 32 bits | - | - | 0 or error |
| 49 | PingPoll | - | - | - | 0 pending, RTT+1, or error |

Packed endpoints store the IPv4 address in bits 0..31 (network byte order) and the port in bits 32..47.

## QEMU network topology

With QEMU user networking the fallback topology is:

- WovenHat: `10.0.2.15/24`
- Gateway: `10.0.2.2`
- DNS proxy: `10.0.2.3`

DHCP is enabled by default, but Stage 5 keeps this static configuration active until a valid lease arrives. If DHCP deconfigures, WovenHat automatically restores the fallback.

## Boot command

```powershell
$img = Get-ChildItem .\target -Recurse -Filter "wovenhat-os-uefi.img" |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

& "C:\Program Files\qemu\qemu-system-x86_64.exe" `
    -machine q35 `
    -m 1024M `
    -smp 2 `
    -drive "if=pflash,format=raw,readonly=on,file=C:\Program Files\qemu\share\edk2-x86_64-code.fd" `
    -drive "if=pflash,format=raw,file=$PWD\OVMF_VARS.fd" `
    -drive "if=none,id=wovenhatboot,format=raw,file=$($img.FullName)" `
    -device virtio-blk-pci,drive=wovenhatboot,bootindex=1 `
    -drive "if=ide,index=0,format=raw,file=fat:rw:$PWD\wovenhat-data" `
    -device virtio-net-pci,netdev=net0,disable-modern=on `
    -netdev user,id=net0 `
    -boot menu=on `
    -serial stdio `
    -no-reboot `
    -no-shutdown
```

## Test sequence

At the kernel recovery shell:

```text
version
userland
net
netstat
dhcp off
net
dhcp on
sh
```

`userland` should report `18/18 programs ready`.

Inside the Ring-3 shell execute the network probes by full path (the current userspace exec ABI does not yet implement PATH lookup):

```text
/bin/ip
/bin/netstat
/bin/dns
/bin/udp
/bin/nc
/bin/ping
```

`/bin/dns` deliberately resolves `example.com` in Stage 5 because the current exec syscall passes only the executable path as `argv[0]`. Argument-bearing exec/PATH lookup belongs to the next userspace ABI stage rather than being faked here.

## Design boundaries

Stage 5 provides a small WovenHat-native socket ABI, not a POSIX/Berkeley compatibility layer. TCP connect is asynchronous, receive operations can return WOULD_BLOCK, and socket ids are scoped to the owning process. This keeps the kernel implementation small enough to debug while establishing the ownership/lifetime model needed for a later libc/POSIX compatibility layer.

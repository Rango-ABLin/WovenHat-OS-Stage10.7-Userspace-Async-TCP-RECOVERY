# Userspace ABI

## Overview

WovenHat userspace is ring-3 code entered via `iretq` with a System V-style
stack. Programs are ELF images loaded by the kernel (`load_elf` /
`load_elf_with_argv`) or by `exec`.

## Syscall entry

- Vector: `int 0x80` (DPL 3)
- Args: `RAX` = number, `RDI`, `RSI`, `RDX`
- Return: `RAX` (`u64::MAX` on error)

| # | Name | Notes |
|---|------|--------|
| 0 | read | fd 0 = keyboard (non-blocking) |
| 1 | write | fd 1/2 = serial console |
| 2 | open | path, path_len → fd |
| 3 | exit | status |
| 4 | getpid | |
| 5 | waitpid | child pid; `-2` still running |
| 6 | close | |
| 7 | yield | |
| 8 | mmap | length, writable |
| 9 | munmap | |
| 10 | file_write | fd, buf, len |
| 11–12 | message_send / receive | IPC |
| 13–14 | getuid / getgid | |
| 15 | exec | path, path_len (replaces image) |
| 16 | fork | parent→child pid, child→0 |
| 17 | stat | packed metadata |
| 18 | readdir | path, len\|(index≪16), name buf |
| 19 | mkdir | |
| 20 | chdir | |
| 21 | getcwd | |
| 22 | dup | |
| 23 | pipe | read\|(write≪32) |
| 24 | dup2 | |
| 25 | getppid | |
| 26 | kill | 0/9/15 |
| 27 | lseek | fd seek |
| 28 | unlink | path, path_len |
| 29 | sleep | ticks |
| 30 | rename | old path, old_len|(new_len<<32), new path |
| 31 | getticks | monotonic ticks |
| 32 | sync | flush mounted files and device cache |
| 33 | ioctl | fd, request, argument |
| 34 | sigaction | signal handler/default/ignore |
| 35 | getpgrp | |
| 36 | setpgid | pid, pgid |
| 37 | socket | UDP/TCP descriptor |
| 38 | bind | socket, local port |
| 39 | connect | socket, packed endpoint |
| 40 | net_send | socket, buf, len |
| 41 | net_recv | socket, buf, capacity |
| 42 | net_close | socket |
| 43 | net_info | network status/config |
| 44 | dns_start | hostname query |
| 45 | dns_poll | query completion |
| 46 | net_peer | socket peer endpoint |
| 47 | dhcp | static/DHCP mode |
| 48 | ping_start | packed IPv4 |
| 49 | ping_poll | query id |
| 50 | exec_command | command line |
| 51 | env_get | key lookup |
| 52 | env_set | key/value update |
| 53 | env_count | |
| 54 | env_entry | indexed KEY=VALUE |
| 55 | process_count | |
| 56 | process_info | indexed process info |
| 57 | spawn_command | command line -> child pid |

## Initial stack

```text
  [ argv strings ]
  NULL          ← envp terminator (empty env)
  NULL          ← argv terminator
  argv[n-1] … argv[0]
  argc          ← RSP
```

## Default capabilities (userspace)

Console, FileRead, FileWrite, Ipc, ProcessCreate.

## Installed programs

| Path | Role |
|------|------|
| `/bin/sh` | Interactive shell |
| `/bin/init` | Banner then `exec /bin/sh` |
| `/bin/selftest` | Boot / regression self-test |

## Shell builtins (`/bin/sh`)

`help`, `echo`, `cat`, `ls`, `mkdir`, `cd`, `pwd`, `stat`, `write`, `persist`,
`rm`, `rename`, `sync`, `fs`, `blockio`/`iostat`, `mmaptest`, `msynctest`,
`exit`/`quit`. Any other line is treated as a path: `fork` -> `exec` -> parent
`waitpid`.

For `/mnt` short-name FAT32 paths, `mkdir`, `unlink`, and `rename` update the
on-disk directory entries and flush before returning success; cross-mount rename
and non-empty directory unlink are rejected.

## Boot flow

After kernel self-tests, boot schedules `init`, which replaces itself with
`/bin/sh`. The kernel diagnostic shell remains available via F1.


## Read-only private file mapping

Syscall 58 (`MmapFile`): fd, byte length, page-aligned file offset -> mapped
address or `u64::MAX`. Length is 1–65536 bytes and must fit inside the file.
The mapping is a read-only, NX private snapshot; syscall 9 unmaps it.
Existing anonymous mmap (8) is unchanged. See `file-mmap.md` for the contract
and run `mmaptest` in the diagnostic shell for the Ring-3 regression.

Syscall 59 (`MmapFileWritable`) takes the same fd, length, and file-offset
arguments as syscall 58. It creates a writable NX private snapshot; writes do
not modify the file. FileRead capability is sufficient. Fork uses copy-on-write
for these mappings. See `file-mmap.md` for bounds and lifecycle semantics.

Demand paging: syscall 60 (`MmapFileLazy`, read-only) and 61
(`MmapFileLazyWritable`, writable) take fd, length, and page-aligned offset.
They reserve private NX mappings and populate pages on first access, including
kernel buffer copies. The descriptor may be closed immediately. Absent pages
read the backing file when faulted; these are not creation-time snapshots.
Bounds and error sentinel match 58/59. See `file-mmap.md` for lifetime rules.

Shared mappings: syscall 62 (`MmapFileShared`) takes fd, length, page-aligned
offset and requires FileRead plus FileWrite. Length must be page aligned or
end at EOF. Aliases and fork children share writable NX physical file pages.
Syscall 63 (`Msync`) takes the mapping base and a length rounding to its full
allocation, requires FileWrite, and returns zero or the error sentinel. It
updates the RAM inode and persists named /mnt backing files through ATA flush.
Unlink retains referenced inodes. See `file-mmap.md` for truncation, reclaim,
writeback errors, and the current single-CPU concurrency boundary.

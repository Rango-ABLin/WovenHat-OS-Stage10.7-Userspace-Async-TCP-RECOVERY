# Userspace Syscall ABI

WovenHat exposes interrupt vector 0x80 to ring 3. Arguments use RDI, RSI, and RDX;
RAX selects the call and receives either a non-negative result or the all-ones error
sentinel. The assembly entry preserves general registers and returns with iretq.

| RAX | Call | Arguments |
| ---: | --- | --- |
| 0 | read | fd, user buffer, length |
| 1 | write | stdout/stderr fd, user buffer, length |
| 2 | open | user path, path length |
| 3 | exit | status |
| 4 | getpid | none |
| 5 | waitpid | child PID |
| 6 | close | fd |
| 7 | yield | none |
| 8 | mmap | length, writable flag |
| 9 | munmap | address, length |
| 10 | file_write | fd, user buffer, length |
| 11 | message_send | receiver PID, user buffer, length |
| 12 | message_receive | user buffer, capacity, sender output pointer |
| 13 | getuid | none |
| 14 | getgid | none |
| 15 | exec | user path, path length |
| 16 | fork | none |
| 17 | stat | user path, path length → packed kind/size/writable |
| 18 | readdir | user path, path_len|(index<<16), user name buffer → name_len|(kind<<8) |
| 19 | mkdir | user path, path length |
| 20 | chdir | user path, path length |
| 21 | getcwd | user buffer, capacity → length |
| 22 | dup | fd → new fd |
| 23 | pipe | → read_fd | (write_fd≪32) |
| 24 | dup2 | oldfd, newfd |
| 25 | getppid | |
| 26 | kill | pid, sig (0/9/15) |
| 27 | lseek | fd, offset, whence |
| 28 | unlink | user path, path length |
| 29 | sleep | ticks |
| 30 | rename | old path, old_len|(new_len<<32), new path |
| 31 | getticks | none |
| 32 | sync | none |
| 33 | ioctl | fd, request, argument |
| 34 | sigaction | signal, handler address/default/ignore |
| 35 | getpgrp | none |
| 36 | setpgid | pid, pgid |
| 37 | socket | kind: UDP=1, TCP=2 |
| 38 | bind | socket, local port |
| 39 | connect | socket, packed IPv4 endpoint |
| 40 | net_send | socket, user buffer, length |
| 41 | net_recv | socket, user buffer, capacity |
| 42 | net_close | socket |
| 43 | net_info | user NetInfo pointer |
| 44 | dns_start | hostname pointer, length |
| 45 | dns_poll | query id, user IPv4[4] buffer |
| 46 | net_peer | socket |
| 47 | dhcp | 0 static fallback, nonzero DHCP |
| 48 | ping_start | packed IPv4 |
| 49 | ping_poll | query id |
| 50 | exec_command | command-line pointer, byte length |
| 51 | env_get | key pointer, key length, value buffer |
| 52 | env_set | key pointer, packed key/value lengths, value pointer |
| 53 | env_count | none |
| 54 | env_entry | index, output pointer, capacity |
| 55 | process_count | none |
| 56 | process_info | index, user ProcessInfo pointer |
| 57 | spawn_command | command-line pointer, byte length |

Descriptor 0 reads the nonblocking PS/2 byte stream; descriptors 1 and 2 write to COM1. The reserved standard descriptors cannot be closed, and VFS handles begin at 3. See [standard streams](standard-streams.md).

All paths and I/O payloads have fixed upper bounds. Pointer-bearing calls translate and
validate each user page before copying. File and IPC calls additionally pass capability,
credential, descriptor, and VFS/queue checks.

The embedded ring-3 validation program exercises fork, exec, write, open, read, close, mmap,
munmap, yield, getpid, getuid, getgid, and exit. Boot also validates waitpid and process
reclamation from the kernel parent.

`exec` requires FileRead and ProcessCreate. The kernel copies the bounded path, reads and
validates the complete ELF into a fresh address space, and only then commits the process
and task records. It switches CR3 before reclaiming the previous image and anonymous
mappings, preserves the process ID, credentials, capabilities, and open descriptors, and
enters the new image directly. A failed load returns the error sentinel without changing
the caller.

`fork` requires ProcessCreate. It deep-copies the executable segments, user stack, and
anonymous mappings into a distinct CR3 root before publishing the child. The child
inherits the parent's credentials, capabilities, and descriptor snapshots. The syscall
returns the child PID to the parent and zero to the child by resuming the copied register
frame through the common interrupt-return epilogue. If cloning or table publication
fails, no child is exposed and the parent receives the error sentinel.

The boot fixture checks both return paths, child exit status 42, parent wait/reap, and
subsequent process-image cleanup. Fork and exec use bounded process, mapping, ELF, and
descriptor tables and do not overcommit memory.

## Current boundary

Fork uses copy-on-write page sharing; open-file descriptions are reference-counted so
offsets are shared across parent and child. There is no `argv`/environment transfer yet.
Directories are first-class VFS nodes; `stat`, `readdir`, `mkdir`, `unlink`,
`rename`, and `sync` are available. For `/mnt` short-name FAT32 paths, `mkdir`,
`unlink`, and `rename` update the on-disk directory entries and flush before
returning success; cross-mount rename and non-empty directory unlink are rejected.


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

# Stage complete — WovenHat POSIX-oriented userspace + FAT32 write path

## Done in this stage
- Kernel: preemptive tasks, processes, COW fork, capabilities, syscalls 0–33
- VFS: files/dirs, cwd, shared offsets, seek, rename, remove
- Pipes: blocking read/write, kill closes FDs and unblocks peers
- Shell: builtins, `|`, `>`, `>>`, `<`, `&` (background), redirects
- Multi-stage pipelines: up to 3 stages (`a|b|c`)
- /bin: sh, init, selftest, echo, cat, ls, sleep, true, false
- Mini-libc: syscall stubs, bump heap + freelist free, printf
- **ATA PIO write** (command 0x30) on the primary master
- **FAT32 write primitives**: `create_root_file` (allocate clusters, update FATs, write data + root dir entry)
- **VFS → disk persistence**: `persist_path("/mnt/…")` and `sync_all_mounted()`; `sys_sync` returns the number of files written

## Explicit limitations (not bugs)
1. **Root directory + 8.3 names only** — no subdirectory create/write yet; root dir is not extended if full
2. **No buffer/page cache** — every persist hits the device
3. **No automatic dirty tracking** — callers must `sync` (or call `persist_path`)
4. **No dynamic linker / full libc** — freestanding programs only
5. **No sockets, pthreads, termios, job process-groups**
6. **Signals** — kill/SIGTERM/SIGKILL only; no sigaction dispositions
7. **Builtin|program** — `echo`/`cat` as left of `|` work only as path programs under `/bin`

## Next stage (not this one)
- Subdirectory support: mkdir on the volume + path-aware create
- Simple buffer/page cache between VFS and block layer
- More /bin utilities and mini-libc growth
- sigaction + process groups / basic job control
- Networking (virtio-net + smoltcp)
- C toolchain + real libc

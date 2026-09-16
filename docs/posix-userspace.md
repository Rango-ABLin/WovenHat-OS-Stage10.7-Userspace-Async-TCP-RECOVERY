# POSIX Userspace — Honest Scope

## What “full POSIX” means

A full POSIX userspace is typically:

- ISO C / POSIX **libc** (thousands of functions)
- **Dynamic linker** (`ld.so`) and shared libraries
- **Core utilities** (POSIX shell, `ls`, `cp`, `sed`, … as separate binaries)
- **Signals**, process groups, sessions, job control
- **Pipes**, FIFOs, sockets, terminal (`termios`)
- **pthreads**, locale, wide char, and more

That is years of work for a production system. WovenHat does **not** claim a
complete POSIX userspace.

## What WovenHat provides now (POSIX-oriented subset)

### Process model
- `fork`, `exec`, `waitpid`, `exit`, `getpid`, **`getppid`**
- Minimal **`kill`** (signals 0, 9/SIGKILL, 15/SIGTERM)

### Files and directories
- `open`, `read`, `write` / `file_write`, `close`, `dup`, **`dup2`**
- `stat`, `readdir`, `mkdir`, `chdir`, `getcwd`

### Pipes
- **`pipe`** — anonymous pipe with read/write ends
- Non-blocking short reads/writes (no full sleep/wake queue yet)
- Ends are first-class FDs and survive `fork` with correct refcounts

### Memory
- `mmap` / `munmap` (anonymous)

### IPC (non-POSIX message API)
- `message_send` / `message_receive` (WovenHat-native)

### Userspace programs
- `/bin/sh` — interactive shell with builtins + fork/exec
- `/bin/init` — starts the shell
- `/bin/selftest` — kernel ABI exercise

### Capabilities
Userspace starts with: Console, FileRead, FileWrite, Ipc, ProcessCreate.

## Explicitly not done

| Area | Status |
|------|--------|
| libc (`printf`, `malloc`, …) | Not present — programs are freestanding asm/ELF |
| Dynamic linking | No |
| POSIX shell grammar (`|`, `&&`, redirects) | Single `|` pipeline for `path\|path` |
| Job control / process groups | No |
| Full signal disposition (`sigaction`) | No — kill marks exit only |
| Blocking pipes | Yes — readers/writers block and wake |
| Sockets / networking | No |
| pthreads | No |
| termios / TTYs | No — serial + PS/2 only |
| FIFOs, `unlink`, permissions, uid switch | No |

## Recommended path to “more POSIX”

1. Tiny freestanding **libc** (syscall stubs + `memcpy`/`strlen`/`printf` serial)
2. Blocking pipe wait-queues + multi-stage / builtin pipelines
3. `sigaction` + default dispositions
4. Port a small shell (e.g. toybox/ash subset) as real C
5. Only then grow toward IEEE Std 1003.1 conformance testing

This document is the contract: **subset is real; “full POSIX” is not claimed.**


## Minimal libc stubs

Symbols `wovenhat_sys_*` (read/write/open/close/exit/fork/exec/pipe/dup2/…)
are embedded as freestanding System V AMD64 stubs that issue `int 0x80`.
They are a seed for a real static libc, not a complete C library.


## Mini heap / puts

- `wovenhat_heap_init()` — `mmap` 64KiB, returns handle
- `wovenhat_malloc(size, heap)` — bump allocate (8-byte aligned); no real `free`
- `wovenhat_puts(cstr)` — write NUL-terminated string to stdout

Still not a C standard library; enough to write small freestanding programs.


## Shell redirects & kill (recent)

- `cmd > file` — stdout redirect (creates file if missing via open+O_CREAT-ish)
- `cmd < file` — stdin redirect
- `kill <pid>` — SIGTERM (15)
- Still single-stage `|` only (no `a|b|c` yet)

## Mini printf

`wovenhat_printf(fmt, ...)` supports `%s`, `%d`, `%c`, `%%` (up to 3 args).
`wovenhat_free` is a no-op (bump allocator).
`wovenhat_strlen` provided.

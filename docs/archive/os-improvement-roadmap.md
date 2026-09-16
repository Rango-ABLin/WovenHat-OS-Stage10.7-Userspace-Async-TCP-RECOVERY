# WovenHat OS Improvement Roadmap

**Version**: 0.0.8-dev  
**Last updated**: 2026-09-03  
**Status**: Living document — reflects actual codebase state

---

## Current State Summary (v0.0.7)

WovenHat has moved well beyond a minimal kernel demo. The following subsystems are implemented and largely self-tested:

### Core Kernel
- UEFI bootloader integration + framebuffer console
- Physical frame allocator + paging with mapping validation
- Kernel heap (256 KiB, fixed-slot tracking)
- GDT / TSS / IDT, PIC, timer, serial
- Structured panic path and serial diagnostics
- HAL: CPUID feature detection, ACPI table parsing (MADT, FADT, HPET, MCFG), PCI enumeration

### Process Model
- Preemptive multitasking with timer-driven switches
- Process table + task control blocks
- Ring-3 user mode entry (`wovenhat_enter_user_mode`)
- Credentials (UID/GID) + capability sets
- Fork (eager deep-copy of address space) + exec (ELF loader)
- Waitpid / zombie reaping
- User-process fault isolation (page fault, GPF, etc. terminate only the faulting process)

### Syscall ABI (vector 0x80)
| # | Call            | Status |
|---|-----------------|--------|
| 0 | read            | Done   |
| 1 | write           | Done   |
| 2 | open            | Done   |
| 3 | exit            | Done   |
| 4 | getpid          | Done   |
| 5 | waitpid         | Done   |
| 6 | close           | Done   |
| 7 | yield           | Done   |
| 8 | mmap            | Done   |
| 9 | munmap          | Done   |
|10 | file_write      | Done   |
|11 | message_send    | Done   |
|12 | message_receive | Done   |
|13 | getuid          | Done   |
|14 | getgid          | Done   |
|15 | exec            | Done   |
|16 | fork            | Done   |

### Storage & VFS
- Block layer + ATA IDENTIFY
- GPT and MBR partition parsing
- FAT32 chain reads
- VFS with open/read/write/close
- Standard streams (0/1/2) wired to keyboard / COM1

### Other
- Capability-gated IPC (bounded message queues)
- Kernel audit ring (64 records)
- Graphics primitives + early GUI/window scaffolding
- Benchmark counters (`BENCH` shell command)
- Extensive boot-time self-tests

### Resource Limits

Limits are now centralized in `kernel/src/config.rs` (Phase A1 started).

**Previous (v0.0.7) → Current**
```
MAX_TASKS              8  →  32
MAX_PROCESSES          8  →  32
MAX_FILE_DESCRIPTORS   8  →  16
MAX_IPC_ENDPOINTS      8  →  32
IPC_QUEUE_DEPTH        8  →  16
MAX_MESSAGE_SIZE      64  →  256 bytes
MAX_IO_SIZE          256  →  1024 bytes
MAX_PATH_SIZE         64  →  128 bytes
MAX_VFS_NODES         16  →  64
MAX_ELF_SEGMENTS       4  →  8
MAX_ANONYMOUS_MAPPINGS 8  →  16
MAX_DEVICES           16  →  32
MAX_HEAP_ALLOCATIONS 256  →  512
```

Further work in Phase A: freelist/slab allocation for process/task tables, refcounted open files, and COW fork.

---

## Strategic Goal

Turn the current solid prototype into a **usable, multi-process, single-user operating system** that can:

1. Run real userspace programs from a filesystem
2. Support a reasonable number of concurrent processes
3. Provide a practical shell and basic utilities
4. Serve as a clean platform for more advanced research (distributed, AI-native features, etc.)

The ambitious long-term vision (AI intent layer, time-travel debugging, multi-node clustering) remains valid, but it must be built on a stable multi-process foundation.

---

## Recommended Roadmap

### Phase A — Scalability & Correctness Foundations (highest priority)

**Goal**: Remove artificial limits and fix the most important semantic gaps so the system can actually host real workloads.

#### A1. Raise core limits and improve data structures
- Increase `MAX_PROCESSES` / `MAX_TASKS` to at least 32–64
- Replace fixed arrays for TCBs, process table, and open-file tables with a simple freelist or slab allocator
- Raise IPC endpoint and message limits proportionally
- Make key limits compile-time configurable

#### A2. Proper open-file semantics
- Introduce reference-counted open-file descriptions
- On `fork`, share the underlying open-file object (so file offsets are shared as POSIX expects)
- Keep per-process file-descriptor tables

#### A3. Copy-on-write fork
- Replace eager deep-copy of address spaces with COW page tables
- Page-fault handler promotes pages to private writable copies
- Dramatically reduces fork cost and memory pressure

#### A4. Demand paging & better memory management
- Zero-fill-on-demand for anonymous mappings
- Growable user stacks
- Stronger guard pages
- Expandable kernel heap (or at least a larger fixed heap + better tracking)

**Definition of done**
- Can create ≥ 32 processes without hitting hard limits
- `fork` + heavy memory usage no longer exhausts physical frames immediately
- File offsets behave correctly across parent/child

---

### Phase B — Usable Filesystem & Storage

**Goal**: Make the VFS actually useful for real programs.

#### B1. Directory support
- `readdir` / `getdents` style interface
- `stat` / `fstat`
- Proper handling of `.` and `..`
- Absolute vs relative path resolution

#### B2. Robust path handling
- Longer paths
- Component walking with correct permission checks
- Mount-point support (even if initially only one root)

#### B3. Real disk integration
- Mount the actual disk image supplied to QEMU
- Read real FAT32 volumes created on the host
- Write support that survives reboot (with care)

#### B4. Block layer improvements
- Simple page cache / buffer cache
- Multiple block devices
- Better error propagation

**Definition of done**
- Userspace can `open("/bin/sh")`, `readdir("/")`, and see real files from the disk image

---

### Phase C — Userspace Runtime & Tools

**Goal**: Stop living inside the kernel shell.

#### C1. Process startup
- Proper `crt0` / entry stub
- `argc` / `argv` / environment passing on `exec`
- Stack setup that matches what a normal compiler expects

#### C2. Minimal userspace libc
- Syscall wrappers
- Basic `malloc` (or bump allocator), `printf`, string functions
- File and process helpers

#### C3. Init + shell in userspace
- Simple `init` process that starts a shell
- Move the bulk of shell logic out of the kernel
- Basic builtins + external command execution

#### C4. Core utilities
- `ls`, `cat`, `echo`, `ps`, `mkdir` (as real userspace binaries)
- Simple package layout under `/bin`, `/etc`, etc.

**Definition of done**
- Boot ends by running a userspace `init` → userspace shell
- Can compile and run small C programs against the syscall ABI

---

### Phase D — Scheduler, SMP Preparation & Observability

#### D1. Better scheduling
- Priority levels or multilevel feedback
- Per-process CPU time accounting
- Sleep / timed wakeups that integrate cleanly with the scheduler

#### D2. SMP groundwork
- Per-CPU runqueues
- IPI support (ACPI MADT already parsed)
- CPU affinity hooks

#### D3. Observability
- Richer `ps` / `top`-style information
- Per-process memory and CPU stats
- Expand the existing `BENCH` infrastructure
- Optional kernel tracing buffer

---

### Phase E — Security Hardening & Polish

- Supplementary groups + `setuid`/`setgid` family
- Persistent audit log (or at least exportable)
- Stronger W^X, ASLR for userspace
- Comprehensive capability and pointer validation audit
- Signal delivery (minimal viable set)
- Cleaner device model and driver registration

---

### Phase F — Research Features (after the above)

Only after Phases A–C are solid:

- Networking (virtio-net + basic TCP/IP or even just loopback + UDP)
- Microkernel-style isolation of more services
- Deterministic record/replay (time-travel debugging foundation)
- Multi-node / distributed primitives
- AI-native intent layer and dynamic app generation

These remain part of the long-term vision but should not delay making the OS actually usable.

---

## Suggested Near-Term Order of Work

1. **Raise limits + freelist for processes/tasks/FDs** (A1)
2. **Reference-counted open files + correct fork semantics** (A2)
3. **Copy-on-write fork** (A3)
4. **Directory support + path walking in VFS** (B1/B2)
5. **Real disk mounting in QEMU** (B3)
6. **Userspace crt0 + argv + minimal libc** (C1/C2)
7. **Userspace init + shell** (C3)

This sequence produces the highest increase in real-world usefulness per unit of effort.

---

## Architecture Notes Going Forward

Continue the current hybrid direction:

- Kernel owns memory management, scheduling, interrupt routing, capability enforcement, and the VFS core
- Drivers and higher services should move toward isolated processes where practical
- All privileged operations remain capability-gated
- Prefer explicit IPC over shared mutable state

Keep the “no unbounded allocation on syscall paths” discipline that currently makes the system robust and easy to reason about.

---

## Success Metrics for the Next Major Milestone

A good v0.1.0 target would be:

- ≥ 32 concurrent processes
- Working COW fork
- Real FAT32 volume mounted from disk image
- Userspace init + shell
- Ability to run several small userspace utilities
- All existing self-tests still pass
- New integration tests that exercise multi-process file and IPC workloads

Once that bar is cleared, WovenHat stops being “an impressive kernel prototype” and becomes a small but genuine operating system.

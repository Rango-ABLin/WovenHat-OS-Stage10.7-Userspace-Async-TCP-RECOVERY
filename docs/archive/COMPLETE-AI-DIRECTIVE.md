# WovenHat OS: Complete AI Agent Directive & Implementation Guide

**Version**: 1.1  
**Date**: 2026-09-03  
**Status**: Active Development (v0.0.7 → v0.1.0 → v2.0.0)  
**Audience**: AI Agents, Developers, Project Stakeholders

---

## Quick Navigation

- [Executive Summary](#executive-summary)
- [Project Vision](#project-vision)
- [Current State Analysis](#current-state-analysis-v007)
- [Development Roadmap](#development-roadmap)
- [Architecture Specifications](#architecture-specifications)
- [AI Agent Instructions](#ai-agent-instructions)
- [Related Documents](#related-documents)

---

## Executive Summary

### What is WovenHat OS?

WovenHat is an **AI-native, capability-based operating system** designed to become:

1. **Intent-driven** — Users can eventually describe applications in natural language
2. **Capability-secure** — Fine-grained capabilities prevent privilege escalation
3. **Transparent scaling** — Single machine today, multi-node clustering later
4. **Debuggable** — Foundation for deterministic record/replay (time-travel debugging)
5. **Open and modern** — Written in Rust, fully open source

### Current Status (2026-09-03)

| Item                    | Status                                      |
|-------------------------|---------------------------------------------|
| Version                 | 0.0.7                                       |
| Architecture            | Hybrid x86_64 kernel (not yet microkernel)  |
| User mode               | Yes (ring 3)                                |
| Preemptive scheduling   | Yes                                         |
| Syscalls                | 17 (fork, exec, waitpid, mmap, IPC, …)      |
| Filesystem              | VFS + FAT32 + GPT/MBR                       |
| Capabilities + credentials | Yes                                      |
| IPC                     | Bounded message queues                      |
| Graphics / early GUI    | Present (primitives + window scaffolding)   |
| Networking              | Not started                                 |
| Userspace runtime       | Minimal (validation programs only)          |
| Hard limits             | 8 processes, 8 FDs, 64-byte messages, etc.  |

**Immediate goal**: Reach a usable single-user multi-process system (v0.1.0).  
**Long-term goal**: v2.0.0 with AI-native and distributed features.

---

## Project Vision

### Differentiation

| Aspect          | Linux / traditional | WovenHat (target)                  |
|-----------------|---------------------|------------------------------------|
| App creation    | Manual coding       | AI: intent → generated app         |
| Security model  | DAC / coarse        | Fine-grained capabilities          |
| Debugging       | Limited replay      | Deterministic time-travel          |
| Kernel style    | Monolithic          | Hybrid → microkernel-inspired      |
| Distribution    | Manual setup        | Built-in clustering                |
| Language        | C                   | Rust                               |

The vision remains ambitious. The practical path is to first make the system *usable*, then layer research features on a solid foundation.

---

## Current State Analysis (v0.0.7)

### What is already implemented

**Core**
- UEFI boot + framebuffer console
- Frame allocator, paging, 256 KiB kernel heap
- GDT/TSS/IDT, PIC, timer, serial
- Structured panic + serial diagnostics
- HAL: CPUID, ACPI (MADT/FADT/HPET/MCFG), PCI enumeration

**Process & security**
- Preemptive scheduler
- Process table + TCBs
- Ring-3 entry
- UID/GID credentials + capability sets
- Fork (eager copy) + exec (ELF) + waitpid
- User-process fault isolation
- 64-entry audit ring

**Syscall ABI (int 0x80)**  
read, write, open, close, exit, getpid, waitpid, yield, mmap, munmap, file_write, message_send, message_receive, getuid, getgid, exec, fork

**Storage**
- Block layer + ATA
- GPT + MBR
- FAT32 reads
- VFS (open/read/write/close)
- Standard streams 0/1/2

**Other**
- Capability-gated IPC
- Graphics primitives + early GUI
- Benchmark counters
- Extensive boot-time self-tests

### Critical limitations

```
MAX_TASKS / MAX_PROCESSES     = 8
MAX_FILE_DESCRIPTORS          = 8
IPC endpoints / messages      = 8 / 8 (64-byte max)
Path / I/O buffers            = 64 / 256 bytes
ELF load segments             = 4
Kernel heap tracking          = 256 slots
```

- Fork performs eager deep copy (no COW)
- Open-file offsets are not shared across fork
- No directory listing / stat
- No real disk mounting of host-created images
- Almost no real userspace (no libc, no init, shell still largely kernel-side)
- Single-core only
- No networking

---

## Development Roadmap

Aligned with `docs/os-improvement-roadmap.md`.

### Phase A — Scalability & Correctness (highest priority)
- Raise process/task/FD/IPC limits (target ≥ 32–64)
- Freelist / slab for core tables
- Reference-counted open-file descriptions
- Copy-on-write fork
- Demand paging + better heap

### Phase B — Usable Filesystem
- readdir / stat
- Proper path walking (`.` / `..`, absolute/relative)
- Mount real FAT32 volumes from disk images
- Simple buffer cache

### Phase C — Userspace Runtime
- crt0 + argc/argv/env
- Minimal libc
- Userspace init + shell
- Core utilities (`ls`, `cat`, `echo`, `ps`, …)

### Phase D — Scheduler & Observability
- Priorities / multilevel feedback
- CPU accounting
- SMP groundwork (per-CPU runqueues, IPIs)
- Richer process stats

### Phase E — Security & Polish
- Supplementary groups, setuid family
- Persistent / exportable audit
- Stronger W^X + ASLR
- Minimal signals
- Cleaner device/driver model

### Phase F — Research Features (after A–C)
- Networking (virtio-net)
- Further microkernel isolation
- Deterministic record/replay
- Multi-node primitives
- AI intent layer & dynamic app generation

### Near-term ordered work (recommended)
1. Raise limits + freelist
2. Refcounted open files
3. COW fork
4. Directory support + path walking
5. Real disk mounting
6. Userspace crt0 + libc
7. Userspace init + shell

### v0.1.0 Success Criteria
- ≥ 32 concurrent processes
- Working COW fork
- Real FAT32 volume mounted
- Userspace init → shell
- Several small userspace utilities
- Existing self-tests still pass
- New multi-process integration tests

---

## Architecture Specifications

### Capability Model (current)
Capabilities gate: console, timer, task inspect/control, device I/O, interrupt control, memory inspect, filesystem, IPC, process creation, etc.

New processes start with a restricted userspace set (UID/GID 1000). Kernel runs as 0:0.

### Syscall Conventions
- Vector: `0x80`
- Selector: RAX
- Arguments: RDI, RSI, RDX
- Return: RAX (or `u64::MAX` on error)
- User pointers are validated page-by-page against the process page tables

### Memory
- Kernel and user address spaces separated
- Anonymous mappings supported via mmap
- Current fork = full copy; target = COW

### IPC
- One endpoint per process
- Bounded FIFO messages (currently 64 bytes)
- Capability + credential checks on send

### Storage Stack
```
Userspace
    ↓ syscalls
VFS
    ↓
FAT32 / (future FS)
    ↓
Block layer
    ↓
ATA / (future virtio-blk)
```

---

## AI Agent Instructions

### How to use this document
1. Treat `docs/os-improvement-roadmap.md` as the authoritative near-term plan.
2. Prefer incremental, testable changes that keep all existing self-tests green.
3. Respect the “no unbounded allocation on syscall paths” rule.
4. When adding features, update the relevant docs (`syscall-api.md`, `security.md`, etc.).
5. Prefer capability-gated, explicit interfaces over ambient authority.

### Coding standards
- Keep `unsafe` confined to HAL, interrupt entry, context switch, and carefully reviewed memory paths
- Prefer fixed-size or explicitly bounded structures on hot paths
- Every new subsystem should ship with boot-time or unit self-tests
- Update serial/console diagnostics so failures are obvious

### Build & test
```bash
cargo build --release
# Run under QEMU with serial output captured
# Prefer adding checks that print clear OK / FAILED lines
```

### When implementing a phase
1. Read the current code in the relevant modules (`task.rs`, `vfs.rs`, `syscall.rs`, `userspace.rs`, …)
2. Make the smallest change that advances the phase
3. Keep or extend self-tests
4. Update this document and the roadmap if the plan changes

---

## Related Documents

| Document                        | Purpose                                      |
|---------------------------------|----------------------------------------------|
| `os-improvement-roadmap.md`     | Detailed near-term phases and priorities     |
| `syscall-api.md`                | Current syscall ABI                          |
| `security.md`                   | Capabilities, credentials, audit             |
| `ipc-api.md`                    | Message-passing details                      |
| `fault-handling.md`             | Exception policy                             |
| `storage.md` / `partition-tables.md` | Block / GPT / MBR                       |
| `performance.md`                | Benchmark counters                           |
| `memory-protection.md`          | Paging / isolation notes                     |

---

**This directive supersedes the previous v1.0 content.**  
Focus first on making WovenHat a small but genuine multi-process OS (v0.1.0). The AI-native and distributed vision remains the long-term destination.

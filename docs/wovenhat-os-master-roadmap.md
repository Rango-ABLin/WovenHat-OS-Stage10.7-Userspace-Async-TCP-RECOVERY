# WovenHat OS — Master Roadmap to a Fully-Fledged, Best-in-Class OS

> Historical roadmap. The user-supplied [Stage 10.7–36 roadmap](master-development-roadmap.md)
> now defines development order. Consult [stage status](stage-status.md) for actual acceptance.

**Prepared**: September 2026
**Based on**: direct review of the current source tree (`kernel/`, `docs/`, CI config) — not the aspirational docs
**Supersedes**: `docs/os-improvement-roadmap.md` and `docs/COMPLETE-AI-DIRECTIVE.md` (archived to `docs/archive/`), which describe an earlier stage of the project than what actually exists in the repo today

**2026-09-04 correction**: an audit pass (`docs/AUDIT-2026-09-04.md`) found this document itself had drifted from the code by the time it was written — it claimed networking was "Absent" and guard pages "not found implemented," neither of which was true. Table below corrected accordingly. Treat the code as ground truth over this document; if you find another mismatch, fix the table in the same change rather than leaving it stale.

---

## 0. How to read this document

This plan has two tracks that should stay architecturally separate:

- **Track A — Kernel & System OS**: memory, scheduling, drivers, filesystems, networking, security, userspace. This is real systems engineering with no shortcuts.
- **Track B — Platform Differentiators**: the things that would make WovenHat *distinctive* rather than "one more hobby kernel" — AI-assisted app creation, time-travel debugging, federated app sharing, adaptive UI, transparent clustering.

Track B should be built **on top of** Track A as userspace services talking to a stable syscall ABI — never folded into kernel code. Every "best ever" feature below is written with that boundary in mind. Mixing them is the single most common way ambitious OS projects stall.

---

## 1. Honest Current State (September 2026)

Corrected against source, not the stale docs:

| Subsystem | Status | Evidence |
|---|---|---|
| Physical memory / paging | Solid — checked-arithmetic frame allocator, per-page permission validation on all user copies | `memory.rs`, `paging.rs` |
| Scheduling | **Preemptive**, timer-interrupt driven, task states, PID/parent-child, wait/exit | `task.rs`, `interrupts.rs:199` |
| User mode + syscalls | Working ring-3 execution with an expanded `int 0x80` ABI for files, processes, IPC, networking, and mmap; capability-gated | `syscall.rs`, `gdt.rs` |
| Capability model | 11 capability bits, bootstrap vs userspace default sets | `capability.rs` |
| Filesystem | FAT32 read/write/mutation path for short-name `/mnt` files and directories, GPT partition parsing, and malformed-volume hardening | `fat32.rs`, `gpt.rs`, `vfs.rs`, `storage.rs` |
| IPC | Fixed-size mailbox model, capability + allow-list gated | `ipc.rs` |
| ELF loading | Validating loader: segment overlap checks, W^X, address-limit clamping | `elf.rs` |
| Audit | Privileged-syscall audit log | `audit.rs` |
| GUI | Framebuffer + basic widget/window primitives, **no compositor, no real desktop shell** | `graphics.rs`, `gui.rs` |
| Networking | **Present** — virtio-net transport + smoltcp (DHCP, DNS, ICMP ping, UDP/TCP sockets) behind a capability-gated syscall ABI; corrected from an earlier "Absent" claim in this doc, see 2026-09-04 note above | `virtio_net.rs`, `network.rs` |
| SMP | **Stage-6 bounded SMP maturity implemented**: 1-4 CPUs, Ready-task migration for audited kernel jobs, per-CPU load accounting, automatic rebalancing, reschedule IPIs, LAPIC/IOAPIC and acknowledged TLB shootdowns; userspace/I/O remain BSP-owned | `smp.rs`, `task.rs`, `interrupts.rs`, `docs/stage-6-smp-maturity.md` |
| Drivers | Hard-coded to QEMU (serial, PS/2, PIT, ATA/PIO) | `ata.rs`, `keyboard.rs`, `timer.rs` |
| CI | `cargo clippy -D warnings` + QEMU boot + exit-code validation | `.github/workflows/kernel.yml` |
| Self-tests | 31 boot-time subsystem self-tests gating kernel continuation | `main.rs` |

**Bottom line**: this is a real microkernel-leaning OS with production-grade defensive coding habits, not a toy. The gap to "fully fledged OS" is breadth (SMP, networking, real drivers, real filesystem write path, package ecosystem) — not fundamentals.

---

## 2. Track A — Path to a Fully-Fledged OS

### Phase A1: Multiprocessor Support (SMP)

0.8.0 implements a bounded foundation; see [release notes](release-0.8.0.md)
and the [ownership/lock audit](smp-lock-audit.md).

- [x] AP trampoline and 1-4 CPU startup.
- [x] Per-CPU GDT/TSS/stacks and current-task state (indexed by APIC identity).
- [x] Calibrated LAPIC timers, IPIs and spurious handling.
- [x] IOAPIC keyboard routing with ISA overrides; legacy fallback.
- [x] CPU-owned runnable sets and least-loaded placement for audited kernel jobs.
- [x] Ready-only task migration for explicitly audited migratable kernel jobs.
- [x] Per-CPU runnable-load accounting excluding idle tasks.
- [x] Conservative automatic rebalancing with dedicated reschedule IPIs.
- [x] Rebalancing/reschedule/migration stress tests across the 1/2/4 CPU matrix.
- [x] Acknowledged cross-core TLB shootdowns and stale-translation regression.
- [x] Bounded ACPI SRAT CPU-domain discovery with domain-local placement and
      rebalancing; see `docs/audit-stage6-numa-2026-09-16.md`.
- [x] x2APIC MSR register/ICR support with xAPIC fallback.
- [x] Ownership audit defining which legacy services remain BSP-only.
- [ ] General multicore userspace, task migration and concurrent I/O service calls.
- [ ] Measured near-linear speedup, wider hardware coverage, NUMA and hotplug.

### Phase A2: Real Driver Model & Hardware Portability
Current drivers are QEMU-specific. A fully-fledged OS needs a driver framework, not one-off hacks.

- [ ] Formalize `device.rs` into a real driver trait (`probe`, `attach`, `interrupt`, `ioctl`-equivalent) and a device registry keyed by PCI vendor/device ID (you already have PCI enumeration in `hal/pci.rs` — wire it up)
- [ ] AHCI driver to replace/augment legacy ATA/PIO (`ata.rs`) — PIO mode won't scale past a toy filesystem
- [ ] NVMe driver (this is what real hardware ships with in 2026)
- [ ] USB stack (xHCI controller driver → HID class driver) so keyboard/mouse aren't PS/2-only
- [ ] Driver isolation: run drivers as unprivileged/capability-scoped tasks talking to the kernel via IPC where feasible, consistent with your microkernel-leaning design — don't let this quietly become monolithic as you add drivers
- **Definition of done**: boots and functions on real hardware (or a second, differently-configured VM/hypervisor) without code changes, not just the one QEMU profile in CI.

### Phase A3: Filesystem Maturity
- [x] FAT32 short-name `/mnt` create/overwrite/delete/rename path, including directory creation, cluster allocation/freeing, subtree-safe directory rename, full-directory cluster extension, FSInfo free-space hint updates, safe mount/sync/unmount lifecycle state, `df`/`fscheck` diagnostics, safer overwrite rollback, shell/syscall integration, sector-cache flush, and regression coverage.
- [ ] FAT32 write-path hardening: long filename creation, journaling or crash-safe metadata ordering.
- [ ] Journaling or copy-on-write filesystem option (ext-like or a from-scratch CoW design) — FAT32 alone is not a serious modern filesystem (no permissions, no journaling, 4 GB file cap)
- [x] Replaced the fixed `fat32::MAX_READ_CLUSTERS` file-read ceiling with size/media-bounded streaming reads; directory metadata scans retain a bounded corruption guard.
- [x] Persistent ATA sector buffer cache with bounded LRU replacement, dirty writeback, failure retry, and diagnostics; see `buffer-cache.md`.
- [x] Demand-loaded clean FAT32 file pages with bounded LRU caching and invalidation; see `file-page-cache.md`.
- [x] Read-only private file snapshots via syscall 58, with Ring-3 mmaptest; see `file-mmap.md`.
- [x] Writable private snapshots (59) and demand-paged private mappings (60/61), including sparse fork and lifecycle tests.
- [x] Physical mapping-cache aliases, unpinned LRU reclamation, shared writable mappings and msync (62/63), retained unlinked inodes, and interrupt-enabled BSP fault I/O.
- [x] Bounded scheduler-level pager worker for lazy file faults, with Ring-3 QEMU regression coverage.
- [x] Conservative live mapped-page eviction/refault for safe file-backed pages.
- [x] Dirty private-page tracking with bounded swap-slot eviction/refault for writable private lazy mappings.
- [x] Disk-backed swap policy over reserved raw ATA sectors, with generation-checked handles and RAM fallback.
- [x] Bounded scheduler-level block-I/O completion worker for primary ATA sector reads/writes/flushes, with QEMU queue/wake/completion regression coverage; ATA remains PIO-polled.
- [ ] Hardware interrupt/DMA-backed storage completion and full multicore execution with TLB shootdowns.
- [ ] File permissions tied into your existing UID/GID syscalls (`Getuid`/`Getgid` exist; nothing currently enforces per-file access control)
- **Definition of done**: can build and store a real userspace toolchain's output on-disk, survive unclean shutdown without corruption, and enforce per-user file permissions.

### Phase A4: Networking
Virtio-net and the IPv4/socket stack already exist. Broader drivers, interrupt-driven networking and packet-filter policy remain.

- [ ] NIC driver (virtio-net for VM/CI parity, e1000 or similar for broader hardware)
- [ ] Either integrate `smoltcp` (no_std-friendly Rust TCP/IP stack) or write a minimal one: ARP, IPv4, ICMP, UDP, TCP
- [ ] Socket syscalls (`socket`, `bind`, `connect`, `send`, `recv`, `listen`, `accept`) layered onto your existing capability model (a new `Capability::NetworkIo` bit — you already reserved this exact idea in earlier docs)
- [ ] DNS resolution (userspace, not kernel)
- [ ] Basic firewall/packet-filter hooks tied into `audit.rs` so network capability grants are auditable like file/IPC access already is
- **Definition of done**: two WovenHat instances can exchange TCP traffic; a userspace program can fetch a resource over HTTP.

### Phase A5: Security & Isolation Maturity
The capability model is a strong foundation; it needs to grow from "static per-boot bitset" to a real security architecture.

- [ ] **Capability delegation & revocation** — right now capabilities are fixed at process creation (`kernel_bootstrap()` / `userspace()`). A mature model needs capabilities to be passed via IPC (classic seL4-style endpoint capabilities), narrowed, and revoked.
- [ ] Per-file/per-resource capabilities instead of the current coarse `FileRead`/`FileWrite` bits — otherwise any process with `FileWrite` can write *any* file it can open.
- [ ] Address Space Layout Randomization (ASLR) for user segments — your ELF loader already validates addresses carefully; randomizing `mapping_start` within the user address range is a natural next step. As of 2026-09-04, `kernel/src/entropy.rs` provides a real RDRAND-backed random source (previously RDRAND was detected but never used anywhere) — this was the missing prerequisite, not the ELF/mmap validation logic itself. See `docs/CHATGPT-MILESTONE-PROMPT.md` for a scoped implementation plan.
- [x] User stack guard pages — implemented (`UserStack::guard_base` in `userspace.rs`, verified by a boot self-test). **Correction from an earlier version of this doc**, which claimed these weren't found in the code.
- [ ] Kernel stack guard pages — user stacks have them (above); the kernel's own double-fault/privilege stacks (`gdt.rs`) do not yet. See `docs/CHATGPT-MILESTONE-PROMPT-2.md` Part 1.
- [x] W^X enforcement — **already correct, and now regression-tested** for both paths: ELF loading rejects writable+executable segments at parse time (`elf::parse`, covered by `elf_loader_self_test`), and `sys_mmap`/`map_anonymous` hard-code every anonymous mapping to non-executable regardless of the writable flag (covered by the new `mmap_w_xor_x_self_test`, added 2026-09-04). **Correction from an earlier version of this doc**, which incorrectly claimed nothing stopped a writable+executable mmap.
- [ ] Formal threat model document: what does WovenHat protect against (malicious userspace app, compromised driver, physical access, network attacker)? Write this down before Phase A4/A2 driver isolation decisions are finalized, not after.
- [ ] Secure boot chain: measured boot via TPM if targeting real hardware, or at minimum a signed-kernel + signed-initrd verification step.
- **Definition of done**: a compromised, capability-limited userspace process cannot escalate privilege, exfiltrate another process's file/IPC data, or execute injected code in a writable page — verified by an actual internal red-team pass, not just code review.

### Phase A6: Toolchain & Package Ecosystem
An OS isn't "fully fledged" until people can build and install software on it.

- [ ] Native userspace libc-equivalent (`libwoven`) wrapping the syscall ABI cleanly — currently userspace code calls syscalls almost raw (`userspace.rs`)
- [ ] Cross-compilation target so third-party Rust/C code can target WovenHat
- [ ] Package format + package manager (signed packages, dependency resolution, install/remove/rollback) — this is also the natural landing spot for the "federated app sharing" vision in your original doc, done as a real userspace service rather than a kernel feature
- [ ] Standard utility set: shell beyond `shell.rs`'s current command set, coreutils equivalents, a text editor, a simple compiler toolchain port if feasible
- **Definition of done**: a third party can write a program against a published SDK, compile it, package it, and install it on a running WovenHat instance without kernel source access.

### Phase A7: Observability, Testing, and Release Engineering
You already do more here than most hobby OSes (31 boot self-tests, clippy-strict CI, QEMU boot validation). Extend it to match the growth above.

- [ ] Scheduled CI run against `nightly` (not just the pinned toolchain) to catch upstream breakage early — you depend on unstable features (`abi_x86_interrupt`, `alloc_error_handler`)
- [ ] Fuzz testing for all untrusted-input parsers: FAT32 boot sector/directory entries, GPT headers, ELF headers — you have hand-written hardening already; fuzzing will find what manual review misses
- [ ] Multi-core CI boot test once Phase A1 lands (boot with `-smp 4`, not just single-core)
- [ ] Real hardware CI runner or at minimum a second hypervisor profile (Bochs, VirtualBox, or bare-metal on a lab machine) so "works in QEMU" stops being the only signal
- [ ] Kernel-level tracing/telemetry (ring buffer of scheduling/interrupt/syscall events) exposed to a userspace debugger — this is also the substrate Track B's "time-travel debugging" will need, so build it here first as a general tool, not a special-case feature
- [ ] Semantic versioning discipline for the syscall ABI once Phase A6 ships third-party software — breaking changes become expensive the moment external packages exist

---

## 3. Track B — What Would Make This "The Best OS Ever"

These are the differentiators. Build them **as userspace services** consuming the stable ABI from Track A — this keeps the kernel small, auditable, and portable, which is itself a competitive advantage (a bloated AI-laden kernel is not what makes an OS good).

### B1. Capability-Native Security UX (your actual strongest differentiator)
Most OSes bolt permissions on top of a DAC (user/group) model. You already have capabilities at the kernel level — lean into this as the headline feature rather than "AI app generation," which every OS vendor is chasing right now and none has cracked. A genuinely fine-grained, auditable, revocable capability system that's *visible and understandable to the user* (not just developers) would be a real first. Concretely:
- Per-app capability manifests, shown to the user at install time in plain language ("this app can: read your documents, use the network — it cannot: see other apps' files")
- Runtime capability request/grant flow (like mobile OS permission prompts, but backed by kernel enforcement instead of app-level honor system)
- A capability audit trail exposed as a first-class system feature (`audit.rs` already exists — surface it in the shell/GUI as a real "what has my system done" log)

### B2. Deterministic Replay / Time-Travel Debugging
Feasible *because* you have a small, auditable syscall surface — this is much harder to retrofit onto Linux/Windows-scale ABIs. Build it as:
- A recording mode where all non-deterministic inputs (syscall results, interrupt timing, IPC message order) are logged to the trace ring buffer from Phase A7
- A replay mode that re-executes a process deterministically from that log
- Step forward/backward through replayed execution in a userspace debugger UI
This is a legitimately novel, achievable differentiator for a small-ABI OS — much less achievable for a general-purpose Linux-scale kernel.

### B3. Adaptive, Minimal-Chrome UI
Rather than "AI themes the UI," start smaller and shippable:
- A compositor (missing today — `gui.rs` has windows/widgets but no compositor) with real damage-tracking and vsync
- Context-aware window layout (not AI-driven at first — just genuinely good tiling/focus-follows-task defaults)
- Only once the compositor and layout engine are solid, layer a local (on-device, privacy-preserving) model that suggests layout/theme adjustments — as an optional userspace service the user can disable, not a kernel dependency

### B4. AI-Assisted App Creation — Done Right
Your original vision ("users command apps into existence") is ambitious and worth keeping, but sequence it correctly:
- This must be a sandboxed userspace runtime, not a kernel feature. Generated code should run with the *narrowest* capability grant the declared app manifest requests (ties directly back to B1).
- Start with a code generator that emits WovenHat-native programs against your `libwoven` SDK (Phase A6) and requires human review/approval of the generated capability manifest before first run.
- Treat this as the last thing you build, once A1–A6 and B1–B3 exist — an AI code generator targeting a kernel that doesn't yet have networking, a package format, or enforced per-file capabilities has nothing solid to generate code against.

### B5. Transparent Clustering
Also legitimately hard and legitimately differentiating if you get there — but it strictly requires Phase A1 (SMP) and A4 (networking) as prerequisites, plus a location-transparent IPC layer (extend `ipc.rs`'s current single-machine mailbox model to route across a network transport). Don't start design work here until those exist; a distributed capability/IPC model designed against a single-machine IPC system will need a rewrite otherwise.

### B6. Reproducible, Verifiable Builds
An underrated "best ever" claim that's cheap to earn and hard for large OSes to retrofit: bit-for-bit reproducible kernel builds, a signed build provenance chain, and a public build log — so any user can verify the binary they're running matches the published source. Given your toolchain is already pinned (`rust-toolchain.toml`) and CI-gated, this is close to free to add now, and gets much harder to add later once the build graph grows.

---

## 4. Suggested Sequencing

```
Now → SMP (A1) → Driver model + real block/NIC drivers (A2) → Networking (A4)
                → in parallel: FS write path + journaling (A3)
                → in parallel: Security maturity — delegation, ASLR, mmap W^X (A5)
    → Toolchain + package ecosystem (A6)
    → Observability/fuzzing/multi-core CI hardening (A7, ongoing throughout)

Once A1/A3/A5 solid:
    → Compositor + real desktop shell (B3, first half)
    → Capability-native security UX surfaced to users (B1)
    → Deterministic replay / time-travel debugging (B2) — reuses A7's trace buffer

Once A4 + A1 solid:
    → Transparent clustering groundwork (B5)

Once A6 + B1 solid:
    → AI-assisted app creation, sandboxed and capability-scoped (B4)

Ongoing, cheap to start now:
    → Reproducible/verifiable builds (B6)
    → Keep docs (this file included) in sync with actual source state every phase boundary
```

## 5. What to Avoid

- **Don't let Track B leak into the kernel.** The moment an LLM runtime, a UI theming engine, or clustering logic needs a new *kernel* syscall beyond generic primitives (IPC, capability grant/revoke, mmap, network sockets), stop and ask whether that belongs in userspace instead.
- **Don't chase every phase in parallel.** SMP touches nearly every global lock in the kernel; land it before networking and drivers multiply the number of places that need to be re-audited for cross-core safety.
- **Don't let the constants (`MAX_IO_SIZE`, `MAX_MESSAGE_SIZE`, `MAX_DIRECTORY_CLUSTERS`, `MAX_ENDPOINTS`) silently become permanent ABI.** Revisit them explicitly as part of A3/A4 rather than discovering three years from now that real software depends on 256-byte syscall reads.
- **Don't let this document go stale like the last one did.** Update the status table in Section 1 at every phase boundary — an inaccurate roadmap actively costs you (or an AI agent) rework.

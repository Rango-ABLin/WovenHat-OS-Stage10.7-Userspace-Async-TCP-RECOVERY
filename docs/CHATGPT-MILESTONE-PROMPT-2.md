You are working in the WovenHat OS repository, a from-scratch x86_64
kernel written in Rust (`no_std`, `no_main`), booted via `bootloader_api`,
built for target `x86_64-unknown-none` on `nightly-2026-07-10`. It has
preemptive scheduling, ring-3 userspace with an `int 0x80` syscall ABI,
FAT32/GPT storage, a capability-based security model, and a smoltcp-backed
network stack. `cargo clippy --workspace -- -D warnings` must pass and the
QEMU boot self-test suite (`main.rs`, ~31 subsystem self-tests gating
kernel continuation) must still all pass — do not weaken or delete a
self-test to make the build green.

This is one PR-sized milestone with **five** parts. Do them in the order
listed — Part 0 (the crash) is diagnostic and may reveal a bug that some
of the later hardening work should not paper over, so investigate it
first even though it's not "next" on the roadmap.

---

## Part 0: Investigate a captured triple-fault crash (do this first)

`qemu-debug.log` in the repo root has a real crash trace already captured
from a run of the OS. The relevant excerpt (interrupt trace frames, most
recent QEMU `-d int` style dump) is:

```
   121: v=20 e=0000 i=0 cpl=0 IP=0038:000000003dd03cb0 pc=000000003dd03cb0 SP=0030:000000003fe551c0 env->regs[R_EAX]=000000000000006a
   [... normal state: GDT=000000003f5dc000/0x47, CR3=000000003f801000 ...]
   Servicing hardware INT=0x20
check_exception old: 0xffffffff new 0xe
   122: v=0e e=0002 i=0 cpl=0 IP=0008:0000010000028721 pc=0000010000028721 SP=0010:0000018000000fd8 CR2=0000018000000fd8
   [... GDT=00000000001b9000/0x17 (only 3 entries!), CR3=0000000000101000 — both DIFFERENT from frame 121 ...]
check_exception old: 0xe new 0xe
   123: v=08 e=0000 i=0 cpl=0 IP=0008:0000010000028721 pc=0000010000028721 SP=0010:0000018000000fd8
   [... same tiny GDT, same low CR3 ...]
check_exception old: 0x8 new 0xe
   [log ends here — triple fault, QEMU reset]
```

Read the full excerpt yourself at `qemu-debug.log` lines 2660–2729 for
every register.

**What this shows**: between frame 121 (normal kernel context: GDT at
`0x3f5dc000` with 9 entries, CR3 `0x3f801000`) and frame 122, mid-interrupt
servicing, execution ends up under a *different* GDT (`0x1b9000`, only 3
entries) and a *different* CR3 (`0x101000`), at `RIP = 0x10000028721` —
and `RSP` exactly equals the faulting `CR2` (`0x18000000fd8`), i.e. the
stack pointer itself points at unmapped memory. That immediately
page-faults (v=0e). The page-fault handler re-faults on entry (0xe→0xe,
which the CPU escalates to a double fault, v=08). The double-fault
handler *also* faults on entry (0x8→0xe) — a true triple fault, hence
QEMU resets and the log simply stops.

**Working hypothesis, to verify, not assume**: this looks like a
task/process context switch (fork/exec, since the symptom is reported as
"shell errors" and the shell is what forks/execs) that loaded a bad CR3
and/or jumped to a bad entry point — and separately, the double-fault
handler's own IST stack is not correctly mapped under whatever address
space was active at the time, since it can't even take the fault cleanly.
Two candidate root causes to check, in this order:

1. **`gdt.rs`**: is the double-fault handler's IST (Interrupt Stack Table)
   entry pointing at a stack that's valid under *every* address space the
   kernel switches into, not just the one active at boot? If task/process
   switching changes CR3 without the IST stack being globally mapped
   (kernel-space, present in every page table), the double-fault handler
   itself will fault the moment it's entered under a different CR3 — which
   is exactly frame 123's symptom.
2. **`task.rs`**: wherever a new process's CR3/entry point get loaded
   (fork/exec path), confirm the entry RIP and initial RSP are validated
   *before* the switch — not just that the target address space's page
   tables are internally consistent, but that the specific RIP/RSP values
   being loaded came from a correctly-initialized process struct and
   weren't read from stale/zeroed/partially-initialized memory. A GDT
   pointer of `0x1b9000` with only 3 entries is suspiciously small — cross-
   reference it against wherever GDTs get allocated per-process (if they
   do) or confirm the kernel is supposed to have exactly one GDT shared
   across all tasks, in which case a per-task GDT existing at all would
   itself be the bug.

**Deliverable for this part**: a root-cause fix (not a workaround — don't
just widen a bounds check to avoid the fault if the actual bug is a wrong
CR3/RIP being loaded), plus a new boot self-test that exercises the
fork/exec path enough times, or with enough load, to have reliably
reproduced this before the fix (document how you confirmed it reproduces
pre-fix and doesn't post-fix — QEMU's `-d int,cpu_reset` output is your
tool here).

---

## Part 1: Kernel stack guard pages (extends existing user-stack work)

You already have this for user stacks: `userspace.rs`'s `UserStack`
tracks a `guard_base`, and `main.rs` has a boot self-test asserting
unmapped user stack guards. Kernel stacks don't have the equivalent
protection. Add it:

- Wherever kernel stacks are allocated (check `task.rs`, `gdt.rs` for TSS
  stack setup, and any IST stack allocation touched in Part 0), leave an
  unmapped guard page immediately below each kernel stack's lowest valid
  address, following the exact same pattern `UserStack` already uses
  (don't invent a second convention).
- Add a boot self-test, matching the existing style, that confirms kernel
  stack guard pages are present and unmapped — sibling to the existing
  user-stack guard test, not a replacement for it.
- If Part 0's root cause turns out to be exactly "an IST/kernel stack
  wasn't guarded/mapped correctly," make sure this part's fix doesn't
  duplicate or fight with that fix — Part 0 takes priority; this part
  should build on top of whatever Part 0 established.

## Part 2: Per-file capabilities

Today `capability.rs` gates `FileRead`/`FileWrite` per-process: any
process holding `FileWrite` can write *any* file it can open, not just
files it was actually granted. Narrow this:

- Extend the capability model so file access capabilities can be scoped
  to a specific path or file handle, not just a coarse read/write bit.
  Look at how `ipc.rs` already does capability + allow-list gating for
  message passing — reuse that pattern's shape if it fits, rather than
  inventing an unrelated mechanism.
- Wire the narrowed check into wherever `vfs.rs` / `fat32.rs` currently
  defer to the coarse `FileWrite` bit.
- Preserve backward compatibility for existing syscall behavior: a
  process that legitimately holds broad file capabilities today should
  still work identically; the new mechanism should let *future* process
  creation scope things more narrowly, not silently break existing
  processes.
- Add a boot self-test: a process with a capability scoped to file A must
  be able to write file A and must fail (not panic, not silently
  succeed) to write file B.

## Part 3: W^X regression test

You already do the right thing: `userspace::map_anonymous` hard-codes the
executable flag to `false` for every anonymous mapping, and ELF loading
enforces W^X at load time. Nothing currently locks this in as a
regression test, so a future refactor could silently reintroduce a
writable+executable mapping path without CI noticing.

- Add a boot self-test that attempts to create a writable+executable
  user mapping through every path that creates user mappings (anonymous
  mmap, ELF segment mapping) and asserts each one is rejected or silently
  downgraded to non-executable — whichever the existing code actually
  does, don't change the behavior, just pin it down with a test.
- If you find a path that *isn't* covered (e.g. some future stack/heap
  growth mechanism), flag it in a comment rather than silently expanding
  scope — this part is a regression test, not a redesign.

## Part 4: Docs cleanup

- Delete or archive (your choice — if archiving, move to a `docs/archive/`
  subdirectory) the superseded roadmap docs: `os-improvement-roadmap.md`,
  `COMPLETE-AI-DIRECTIVE.md`, `more-steps.md`, `ten-steps.md`. Keep
  `wovenhat-os-master-roadmap.md` as the single live roadmap, and update
  it to reflect whatever Parts 0–3 actually changed (don't leave it
  claiming these gaps still exist after you've closed them).
- Consolidate the 15 `STAGE9-*-FIX.md` / `STAGE*-FIX.md` incident docs
  into a single `docs/CHANGELOG.md` with one dated entry per fix (a few
  sentences each, not a full copy of each doc) and remove the originals.
  If any contain information not safe to summarize away (e.g. still-open
  caveats), keep that detail in the changelog entry rather than dropping
  it.
- Update `docs/security.md` to mention the Part 1–3 changes (kernel stack
  guard pages, per-file capabilities, W^X regression coverage).

---

## Explicit constraints (apply to all parts)

- Do not implement SMP, networking changes, or filesystem write-path work
  — out of scope for this milestone regardless of what you find while
  investigating Part 0.
- Do not change the syscall ABI (`syscall.rs` `Number` enum values or
  argument order) unless Part 2's per-file capability work strictly
  requires a new syscall — if so, add a new number, never renumber an
  existing one.
- Do not remove or weaken any existing safety check to make a test pass.
  If a self-test fails, fix the underlying code, not the test's
  assertion.
- Every new/changed function needs doc comments at the level already
  present in `elf.rs`/`userspace.rs` — this codebase documents *why*, not
  just *what*.
- `cargo clippy --workspace -- -D warnings` must pass, and all existing
  boot self-tests plus every new one from Parts 0–3 must pass.

## Definition of done

- Part 0's crash no longer reproduces, with a documented root cause (not
  a symptom-level patch) and a self-test that would have caught it.
- Kernel stacks have guard pages, verified by a self-test.
- File write capability is enforceable per-file, verified by a self-test
  showing both the allow and deny case.
- A W^X regression self-test exists and passes.
- `docs/` has one live roadmap doc, one changelog, and no duplicate/
  superseded planning docs.

Work through this as a single focused change-set, one part at a time in
the order given. If you hit a design decision not covered above, make the
most conservative choice that preserves every existing invariant, note
the choice and reasoning in a comment at the decision point, and
continue — don't stop to ask.

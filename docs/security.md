# Kernel Security Model

WovenHat combines per-task capabilities with process credentials. Kernel execution uses
UID 0 and GID 0. Every newly loaded ring-3 process starts with UID 1000 and GID 1000
and the restricted userspace capability set; this prevents a kernel-created process from
implicitly inheriting root identity.

## Authorization

- Capabilities gate console output, file access, IPC, process creation, task control, device I/O,
  interrupt control, and memory inspection.
- Capability grant requires both TaskControl and the capability being delegated.
- Capability revoke requires TaskControl.
- IPC send requires the Ipc capability and either root identity, a matching UID, or a
  matching GID.
- File writes require FileWrite and are also constrained by the VFS node write policy.
- Fork and exec require ProcessCreate; exec also requires FileRead.
- Syscall user pointers are translated page by page and rejected when unmapped or when
  the requested access conflicts with page permissions.

Boot validates root override, same-UID access, same-GID access, cross-identity denial,
capability grant/revoke behavior, and the credentials of two real ring-3 processes.

## Audit ring

A bounded 64-record kernel audit ring stores a monotonic sequence number, timer tick,
actor process ID, action, target, and allow/deny result. Capability grant/revoke, IPC
send, file-write, process-fault, fork, and exec outcomes are recorded. When full, the ring overwrites its oldest
record instead of allocating or blocking. Boot verifies wraparound and confirms the
capability delegation test produced ordered audit evidence.

## Current boundary

Credentials currently contain one UID and one primary GID; supplementary groups,
credential-changing syscalls, executable ownership metadata, and persistent audit
storage are not implemented. Audit records remain kernel-resident and are not yet
exposed to unprivileged processes.

## W^X (write XOR execute)

Anonymous mmap (`map_anonymous` in `userspace.rs`) hard-codes the executable flag to
`false` for every mapping it creates, regardless of the caller-requested writable flag,
so `sys_mmap` can never produce a writable+executable user page. The ELF loader
separately rejects any segment that is both writable and executable at parse time
(`elf::parse`). Both invariants are covered by boot self-tests: `elf_loader_self_test`
exercises the ELF-parse rejection path, and `mmap_w_xor_x_self_test` exercises the
anonymous-mmap path. Neither invariant currently extends to a real `mprotect`-style
syscall, because none exists yet — if one is added, it must preserve the same guarantee
and gain its own self-test rather than relying on these two.

## Entropy

`kernel/src/entropy.rs` provides `random_u64()`, backed by hardware RDRAND when
available (`hal::cpu::detect_features` already detects RDRAND support at boot) with a
documented, explicitly non-cryptographic SplitMix64 fallback for platforms without it.
As of 2026-09-04 this is used to seed the network stack's TCP initial-sequence-number
generation (`network.rs`), replacing a fixed compile-time constant that made every boot's
TCP sequence numbers predictable to a network attacker. It is not yet used for ASLR —
see `docs/wovenhat-os-master-roadmap.md` and `docs/CHATGPT-MILESTONE-PROMPT.md` for that
follow-on work — or for any cryptographic purpose (the fallback path is not suitable
for key material).

## Known gaps (unchanged by the above)

- File write authorization is still process-scoped (`FileWrite` capability), not
  per-file: a process holding `FileWrite` can write any file it can open, not only
  files it was specifically granted. See `docs/CHATGPT-MILESTONE-PROMPT-2.md` Part 2.
- Kernel stacks (as opposed to user stacks, which already have guard pages via
  `UserStack::guard_base`) do not yet have guard pages. See
  `docs/CHATGPT-MILESTONE-PROMPT-2.md` Part 1.
- No ASLR yet for ELF load base, mmap base, or user stack base.

# VFS path semantics

The VFS accepts canonical absolute UTF-8 paths up to 128 bytes, with components
up to 64 bytes (the directory-entry ABI limit). Root is valid. Empty paths,
relative paths, NUL bytes, empty components, dot components, and trailing
slashes on non-root paths return `InvalidPath`. Shell and task path resolvers
continue to resolve relative paths and `.` / `..` before calling the VFS.

Renaming a directory moves every descendant using a slash-boundary prefix
match. For example, `/home/anthony/docs/a.txt` becomes
`/home/user/docs/a.txt` when `/home/anthony` is renamed to `/home/user`.
`/home/anthony2` is unaffected. File data, metadata, and node indices remain
unchanged, so existing open-file descriptions remain valid.

The destination parent must exist and be a directory (`NotFound` otherwise).
An occupied destination returns `AlreadyExists`; replacement is not supported.
Moving a directory beneath itself returns `InvalidPath`. Renaming root or
renaming onto root returns `ReadOnly`. Renaming an existing non-root path to
itself succeeds without changes; a missing source returns `NotFound`.

All descendant path lengths are checked before any mutation. If a destination
path would exceed the path capacity, rename returns `InvalidPath` and leaves
the tree unchanged. The registry lock covers validation and the complete move.
The operation uses the existing node array without allocating another tree.

Removing a non-empty directory returns `NotEmpty`, separately from capacity
errors (`Full`). The diagnostic shell displays `rm: directory not empty`, and
the task layer preserves `NotEmpty`. The syscall ABI still uses its existing
generic failure return. Stored process/shell cwd strings are not rewritten by VFS rename; this
milestone does not add inode-based cwd tracking. FAT32 short-name `/mnt`
renames are now durable: the storage layer updates the on-disk directory entry
before VFS paths are moved, and VFS backing paths for descendants are rewritten
with the same prefix rule.

Regression checks are part of `vfs::self_test()` in the QEMU boot suite. They
cover the complete subtree example, prefix boundaries, file identity and data,
live descriptors, destination failures, rollback on path overflow, exact path
capacity, invalid paths, non-empty removal, and moves across parent directories.

## Validation — 2026-09-07

- `cargo build`: passed with the normal warning policy.
- `cargo build --features qemu-test`: blocked by existing unused-code/import
  errors in the test-feature configuration. For this validation only, built with
  PowerShell `$env:RUSTFLAGS='-A dead_code -A unused_imports'`.
- QEMU q35, 256 MiB, OVMF pflash, `isa-debug-exit`: serial output reports
  `[VFS] read/write and path semantics: PASSED`. This includes the live-handle
  and complete subtree regression checks. The full boot suite subsequently
  panics at `kernel/src/task.rs:1838` with `scheduler not initialized`, so it
  does not reach the all-validations-passed marker or successful exit code 33.
- `cargo clippy --workspace -- -D warnings`: blocked by pre-existing lint
  failures in unrelated files and host-target kernel panic-handler conflicts.
- `git diff --check`: passed.

The VFS scratch registry previously exceeded the 1 MiB boot stack. It now uses
static mutex-protected test storage, reset one node at a time; this allowed the
VFS boot tests to complete without overflowing the stack.

The updated normal kernel also passed the QEMU diagnostic-shell baseline from
`vfs-qemu-baseline.md`, including its negative cases. Removing the populated
`/home/anthony` directory correctly printed `rm: directory not empty` and
preserved the directory. Target-specific Clippy (`cargo clippy -p
wovenhat-kernel --target x86_64-unknown-none -- -D warnings`) reported 16 existing
errors outside the VFS changes and no VFS diagnostics.

## Diagnostic-shell rename command

The diagnostic shell now accepts `rename <old> <new>` and `mv <old> <new>`.
Both resolve paths relative to cwd and require FileWrite capability. Successful
calls print `renamed`; failures report an error without claiming success.
Previously the VFS implementation and userspace `mv` existed, but the diagnostic
shell used for baseline testing had neither command wired in.

End-to-end QEMU check (2026-09-07): created `/home/anthony/docs/a.txt` with
`write ... hello`, then executed `rename /home/anthony /home/user`.
Both old directory `stat` calls returned `stat: not found`, and reading the old
file returned `cat: open failed`. Both new directory paths resolved and
`cat /home/user/docs/a.txt` printed `hello`. Normal `cargo build` passed.
A running QEMU instance must be restarted to load the updated kernel image.

# Stage 10.5 Audit — Userspace Asynchronous VFS/File I/O

## Security boundary
Stage 10.5 does not grant `StorageIo` to ordinary applications. File requests are authorized through the existing `FileRead` / `FileWrite` capabilities and WovenGuard file-scope policy.

## Stable object identity
Submission resolves the process fd once and clones the underlying refcounted `OpenFileId`. The request therefore survives `close(fd)` and cannot be redirected if the same descriptor number is reused. The acceptance probe closes the descriptor immediately after both write and read submission and requires the correct file contents to be produced.

## Pointer lifetime
No userspace pointer is retained. Write bytes are copied into a bounded kernel request buffer before queue publication. Reads stay in a kernel buffer until the owner collects the completed operation. Completion/data copyout occurs before queue/handle consumption, preserving retry safety.

## Offset semantics
The first API is intentionally positional. `vfs::read_at` and new `vfs::write_at` avoid races on the shared open-file seek position and make concurrent operations deterministic.

## Cancellation and teardown
Pending or completed cancellation releases the pinned file immediately. If the worker already owns a copied request, cancellation/owner teardown detaches the generic completion but leaves the queue slot and VFS reference alive until the worker finishes; the worker then releases the reference without signalling a stale async handle.

## SMP validation
The Stage 10.5 probe runs only under `stage10-5-test`, after the complete Stage 10.4 acceptance is preserved. Dedicated 1/2/4 CPU boots validate read/write, fd pinning, stale-handle rejection, cancellation, owner teardown, and final request-table drain.

## Preservation-build dead-code correction

The first external acceptance attempt stopped during the ordinary Stage 10.4 preservation build because Stage 10.5-only telemetry (`Queue::active`, `Stats`, and `stats`) remained compiled while their only consumers were correctly isolated behind `stage10-5-test`. With `-D warnings`, Rust therefore rejected those three unused items before any QEMU boot occurred.

The correction keeps the isolation model intact: those telemetry definitions are now themselves compiled only with `stage10-5-test`. No warning suppression (`allow(dead_code)` / `expect(dead_code)`) is used, and no Stage 10.5 worker or probe is enabled in legacy/preservation boots. Runtime async-file submission, cancellation, teardown, VFS pinning, and WovenGuard behavior are unchanged.

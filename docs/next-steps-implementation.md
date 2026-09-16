# Recommended Next Steps — Implementation Update

This revision implements the five follow-up areas requested after the initial FAT32 write path.

## 1. FAT32 subdirectories and path-aware persistence

- `fat32::create_path_file()` walks multi-component paths and creates missing parent directories.
- `fat32::mkdir_path()` creates directory chains on disk.
- New directories receive FAT32 `.` and `..` entries.
- `storage::persist_path()` now accepts `/mnt/dir/file.ext` instead of root-only names.
- `task::mkdir_path()` persists newly created `/mnt/...` directories to the mounted FAT32 volume.
- Components remain FAT 8.3 for now; long-file-name (LFN) creation is still a later extension.

## 2. Block/page cache

- New `block_cache.rs` provides a fixed-size, allocation-free write-back sector cache.
- Reads are cached and dirty sectors are coalesced until `flush()`.
- Updated 2026-09-07: the ATA device now owns a persistent 64-sector cache shared by mounting, reads, and persistence. Persist operations explicitly flush and retain clean entries; see `buffer-cache.md`.
- Hit/miss counters and a RAM-disk self-test are included.

## 3. Userspace utilities and mini-libc

New executable images installed in `/bin`:

- `/bin/pwd`
- `/bin/mkdir`
- `/bin/rm`

The mini-libc gains:

- `wovenhat_sys_sigaction`
- `wovenhat_sys_getpgrp`
- `wovenhat_sys_setpgid`
- `wovenhat_memcpy`
- `wovenhat_memset`
- `wovenhat_strcmp`

These extend the existing read/write/open/fork/exec/pipe/printf/malloc helpers.

## 4. Signals and process groups

- Per-process signal disposition table for signals 1–31.
- `SIG_DFL` (0), `SIG_IGN` (1), and userspace handler addresses.
- `SIGKILL` cannot be caught or ignored.
- `kill()` now supports an individual PID, caller process group (`pid == 0`), and negative process-group targeting.
- Forked/spawned children inherit process groups appropriately.
- New syscalls: `sigaction` (34), `getpgrp` (35), `setpgid` (36).
- A caught signal is delivered on syscall return: handler address becomes RIP, signal number is passed in RDI, and a normal `ret` resumes the interrupted instruction.

This is intentionally a compact first signal ABI. It does not yet implement masks, alternate stacks, `SA_RESTART`, or a full `sigreturn` frame.

## 5. VirtIO-net + smoltcp

- Added `smoltcp 0.14` as a no-std dependency with Ethernet/IPv4/ICMP/UDP/TCP features.
- VirtIO PCI probe recognizes vendor `0x1af4`, legacy network ID `0x1000`, and modern network ID `0x1041`.
- Added bounded Ethernet RX/TX frame queues and backpressure behavior.
- Added a smoltcp `Device` adapter with Ethernet capabilities, checksum policy, a locally administered MAC, and link-local IPv4 defaults.
- Boot-time self-tests validate the packet adapter and report whether VirtIO-net PCI hardware is present.

The queue-facing transport boundary is isolated in `virtio_net.rs`. The next hardware-specific networking task is binding these queues to real VirtIO PCI DMA descriptor rings/interrupts; the smoltcp-facing API will not need to change.

## Validation note

The supplied execution environment did not include the Rust/Cargo toolchain, so a full `cargo check` could not be run here. The code was structured against the repository's existing no-std patterns and smoltcp's current `Device`/token API. On the development Windows host, regenerate `Cargo.lock` and validate with the repository's normal nightly build before committing.

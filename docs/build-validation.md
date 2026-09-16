# Build and Boot Validation

## Local static gates

Run these from the repository root:

    cargo clippy -p wovenhat-kernel --target x86_64-unknown-none -- -D warnings
    cargo clippy -p wovenhat-os -- -D warnings
    cargo build --release

The root build script consumes the freestanding x86_64 kernel artifact and creates a
UEFI disk image named wovenhat-os-uefi.img under the corresponding Cargo build output
directory.

## QEMU validation image

The qemu-test feature preserves every normal boot validation but, after the breakpoint
handler test and the serial marker shown below, writes 0x10 to the isa-debug-exit port:

    [BOOT] ALL VALIDATIONS PASSED

Build it with:

    cargo build --features qemu-test

With isa-debug-exit configured at I/O port 0xf4, QEMU returns host status 33 for this
success value. The normal feature set does not access that port and continues into the
desktop and diagnostic-shell loop.

## Continuous integration

The workflow runs `scripts/test-release.py` with the pinned toolchain. This
checks kernel/host lint separately, host regressions, and complete 1/2/4-core
QEMU boots both with and without disposable ATA disks. It also tests optimized
four-core images. Every boot requires exact CPU-count and SMP regression
markers in addition to exit status 33. Logs are retained as CI artifacts.

QEMU is available on this Windows development host at
`C:\Program Files\qemu\qemu-system-x86_64.exe`.

See [0.8.0 release instructions](release-0.8.0.md) for the complete gate,
normal-shell keyboard smoke test, and interactive launcher.

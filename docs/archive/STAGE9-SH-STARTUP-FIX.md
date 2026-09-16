# Stage 9 userspace shell startup fix

`/bin/sh` previously called the mmap syscall before printing its banner or prompt.
Other Ring-3 commands could work while `sh` appeared stuck at
`starting userspace shell...` if the anonymous-mapping path did not complete.

The shell now reserves a 4 KiB writable line/scratch buffer directly from its
already-mapped 8 KiB userspace stack before printing the banner. This removes
mmap from the shell startup path while keeping mmap available for later tests.

The terminal `clear()` wrapper is retained and marked `#[allow(dead_code)]`
so `warnings = "deny"` remains clean.

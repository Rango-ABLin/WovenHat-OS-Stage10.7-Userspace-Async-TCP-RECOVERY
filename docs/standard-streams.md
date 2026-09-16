# Standard streams

Every userspace process has three reserved descriptor numbers:

| Descriptor | Stream | Kernel endpoint |
| ---: | --- | --- |
| 0 | standard input | PS/2 keyboard |
| 1 | standard output | COM1 serial console |
| 2 | standard error | COM1 serial console |

Descriptors 0, 1, and 2 are kernel endpoints rather than VFS handles and cannot be
closed. VFS descriptors begin at 3. Fork preserves the standard endpoints and copies
the process's bounded VFS descriptor table; exec preserves both.

Standard-input reads are nonblocking. They return the bytes currently available, up to
the requested bounded length, and return zero when no key is queued. Printable PS/2
set-1 keys produce ASCII bytes; Enter, Backspace, and Tab produce `\n`, byte 8, and
`\t`. Interface-only keys such as F1 are not exposed to userspace. Reading stdin and
writing stdout or stderr require the Console capability, and every userspace pointer is
validated page by page before data crosses the syscall boundary.

Boot validation injects two deterministic keyboard scancodes and has both ring-3 parent
processes read and verify `a` from descriptor 0. The same programs already exercise
stdout, so successful boot proves all three reserved descriptor paths are installed.

Current limitations: stdin has no blocking wait queue, canonical line discipline,
terminal ownership, echo policy, UTF-8 composition, or per-process controlling terminal.
Standard output and error currently share COM1 and are not independently redirected.

# Stage 9 standard file-descriptor stability fix

The Stage 9 runtime previously initialized each process with an empty descriptor table,
but `read(0)` was permanently interpreted as keyboard stdin. As a result, the first
successful `open()` returned descriptor 0. `/bin/cat /etc/version` then read from the
keyboard rather than the opened file and appeared to hang. The same behavior could
consume shell keystrokes and make the interactive shell appear randomly stuck.

This patch establishes the normal standard-descriptor invariant:

- fd 0 = stdin
- fd 1 = stdout
- fd 2 = stderr
- normal open/dup/pipe allocations begin at fd 3

`dup2()` may still intentionally place a file or pipe on descriptors 0, 1, or 2.
The read/write syscalls now detect those redirected standard descriptors and route I/O
through the process file table. Without a redirect, fd 0 remains keyboard input and
fd 1/2 remain the framebuffer/serial console.

This fixes the common descriptor path used by `cat` and stabilizes shell input after
file operations. It also makes future redirection and pipeline behavior internally
consistent.

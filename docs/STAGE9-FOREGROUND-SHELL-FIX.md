# Stage 9 foreground shell and explicit /bin path fix

Two runtime issues were isolated after normal external commands were confirmed to run.

1. `/bin/sh` was given foreground ownership and the kernel yielded only once.
   The kernel task could immediately resume and compete with or starve the interactive
   userspace shell. `cmd_sh()` now follows the proven foreground child pattern used by
   working commands: it repeatedly yields while the shell process remains alive.

2. The kernel command fallback always prefixed `/bin/`, so typing an explicit path such
   as `/bin/echo hello` became `/bin//bin/echo`. Explicit paths are now used verbatim;
   bare commands still resolve under `/bin`.

The userspace shell banner write length was also corrected to the exact 26 bytes, and
the scratch-buffer comment now reflects that the buffer is stack-backed.

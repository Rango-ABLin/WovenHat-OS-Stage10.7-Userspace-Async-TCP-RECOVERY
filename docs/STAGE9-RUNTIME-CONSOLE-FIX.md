# Stage 9 runtime console / userspace-shell fix

This patch resolves two architecture-level issues found during Stage 9 runtime testing.

## 1. Interactive `/bin/sh` blocked the kernel main loop

`cmd_sh()` previously waited for the shell process to exit. The command itself is
executed from the kernel's main event loop, so this synchronous wait prevented the
outer loop from continuing normal scheduler/network/syscall service work.

The shell is now started asynchronously:
- foreground ownership is assigned to the userspace PID;
- the kernel yields once so the new task can run;
- `cmd_sh()` returns immediately to the main loop;
- the main loop already avoids consuming keyboard input while a userspace PID owns
  the foreground.

## 2. Kernel Console and userspace Terminal had independent cursors

The diagnostic kernel `Console` and global userspace `terminal` render into the same
framebuffer but kept separate cursor coordinates. A userspace command such as
`/bin/echo hello` could therefore render at an old screen position and appear to
produce no output.

Both interfaces now expose cursor synchronization helpers. Before a one-shot
userspace command, the terminal cursor is aligned with the kernel console cursor.
After the process exits, the console cursor is updated from the userspace terminal.

This keeps `/bin/echo`, `/bin/ls`, `/bin/cat`, `env`, `ps`, and other external
program output visually continuous with the diagnostic shell.

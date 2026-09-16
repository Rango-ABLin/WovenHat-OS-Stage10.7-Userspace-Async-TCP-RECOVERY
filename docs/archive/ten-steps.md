# Ten-step expansion batch

1. **lseek (27)** — file offset for append/positioned I/O
2. **unlink (28)** — remove file/empty dir from VFS
3. **sleep (29)** — block current task for N timer ticks
4. **open O_CREAT-ish** — missing files created when FileWrite held
5. **Shell `rm`** — userspace unlink
6. **Shell `sleep`** — sleep N ticks
7. **Shell `getpid`** — print process id
8. **Redirect `>>`** — append via open + lseek-to-end + dup2
9. **`printf %x`** — hex formatting in mini-libc
10. **`/bin/echo`** — real argv-based echo executable

Also retained: `|`, `>`, `<`, `kill`, builtins, blocking pipes.

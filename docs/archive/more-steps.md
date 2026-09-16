# More steps batch

1. Shell `wait <pid>`
2. Shell `test -f path` / `test -d path`
3. Shell `:` null command
4. `/bin/ls` executable
5. `/bin/sleep` executable
6. Boot install for ls + sleep
7. Syscall `ioctl` (33) no-op placeholder
8. Help text updated
9. This documentation
10. Continued growth of /bin and shell POSIX surface

Syscall surface: **0–33**.

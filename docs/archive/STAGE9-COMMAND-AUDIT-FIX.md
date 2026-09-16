# WovenHat OS 0.7.0 Stage 9 — Command Audit Fix

This consolidated patch supersedes the earlier Stage 7/8/9 command patches.

## Execution-path fixes
- Ring-3 CS/SS selectors use RPL3.
- User processes enter with the prepared argv stack (`program.image.stack_top`).
- Ordinary external commands use syscall 57 (`SpawnCommand`) instead of forcing every command through fork/COW first.
- Fork remains for pipeline/redirection paths that need descriptor wiring.
- Foreground handoff no longer clears the framebuffer, so diagnostics and prompts remain visible.
- Kernel shell falls back to installed `/bin/<command>` programs, so common userland commands can also be launched directly from the kernel prompt.

## Command-specific fixes
- `/bin/ls` now uses writable stack buffers instead of attempting writes into RX program pages.
- `dns` has a 5-second userspace timeout instead of waiting forever for a resolver response.
- `ping` has a bounded userspace timeout in addition to the kernel ICMP timeout.
- `/bin/en` is provided as a friendly alias for `/bin/env`.
- `/bin/bin` lists the installed userland command names.
- Userland readiness is now 24/24 programs.

## Expected behavior
`tcpd` is a service and intentionally remains running while foregrounded; launch it with `tcpd 8080 &` from the userspace shell when you want the prompt back immediately.

## Smoke test
From the kernel shell:

```
userland
bin
echo hello
pwd
ls /
cat /etc/version
env
en
ps
uptime
ping
dns example.com
sh
```

Then from the userspace shell:

```
bin
echo hello
pwd
ls /
cat /etc/version
env
en
ps
uptime
ping
dns example.com
/bin/echo direct
/bin/ls /
/bin/cat /etc/version
exit
```

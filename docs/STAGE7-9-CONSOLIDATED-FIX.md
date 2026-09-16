# WovenHat OS Stages 7-9 Consolidated Fix

This release consolidates Stages 7, 8, and 9 on top of the clean Stage 6 baseline and fixes the external `/bin` execution regression.

## Root cause fixed

The ELF loader correctly created a C-style userspace stack containing `argc` and `argv`, storing the resulting stack pointer in `UserImage::stack_top`. However, both the initial userspace spawn path and the exec path entered Ring 3 using `UserStack::top` instead. That skipped the prepared argument frame and made argument-driven programs observe an invalid or zero `argc`.

The task runtime now enters userspace with `program.image.stack_top` in both:

- `task::spawn_user_process()`
- `task::exec_current()`

This restores the intended ABI for `/bin/echo`, `/bin/cat`, `/bin/ls`, `/bin/sleep`, `/bin/mkdir`, `/bin/rm`, `/bin/ip`, `/bin/netstat`, `/bin/dns`, `/bin/udp`, `/bin/nc`, `/bin/ping`, `/bin/env`, `/bin/ps`, `/bin/uptime`, and `/bin/tcpd`.

## Included stages

- Stage 7 / 0.5.0: inherited process environment and `/bin/env`
- Stage 8 / 0.6.0: process snapshot ABI, `/bin/ps`, `/bin/uptime`
- Stage 9 / 0.7.0: Ring-3 TCP echo daemon `/bin/tcpd [port]`

The final consolidated tree identifies itself as WovenHat OS 0.7.0 Stage 9 and retains `warnings = "deny"`.

## Smoke test

After `cargo build` and boot:

```text
version
userland
sh
/bin/echo hello wovenhat
/bin/ls /
/bin/cat /etc/version
env
ps
uptime
dns example.com
ping 10.0.2.2
```

For the TCP daemon use `run-stage9-tcp.ps1`, then in WovenHat:

```text
tcpd 8080
```

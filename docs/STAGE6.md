# WovenHat OS 0.4.0 — Stage 6 Clean Build + Argument-Bearing Exec

Stage 6 is a release-hardening and userspace ABI stage. It keeps the Stage 5 VirtIO/smoltcp networking and persistent `/mnt` data disk, removes the compiler warnings reported during the Stage 5 build, fixes stale release banners, and adds a real argument-bearing external-command execution path.

## Stage 6 changes

- Release/package version is now `0.4.0` for both `wovenhat-os` and `wovenhat-kernel`.
- Boot banner, kernel shell and `/etc/version` report `WovenHat kernel 0.4.0 Stage 6`.
- The deliberate shell-first infinite loop is explicitly marked so the exhaustive qemu-test suite can remain compiled without producing the normal-build `unreachable_code` warning.
- The freestanding libc assembly comment no longer produces `unused_doc_comments`.
- Stage 5's IPv4 API fix is retained (`Ipv4Addr::octets()` / `from_octets()`).
- DNS result handling remains IPv4-only without an unreachable match arm.
- New syscall **50 — ExecCommand** accepts a whitespace-delimited command line, constructs `argc/argv`, resolves bare command names beneath `/bin`, loads the target ELF, and replaces the current process.
- The Ring-3 shell uses ExecCommand for normal commands, redirections, two-stage pipelines and three-stage pipelines. Existing syscall 15 (`exec(path,len)`) remains available for compatibility and init bootstrap.
- `/bin/dns [hostname]` now consumes `argv[1]`; if omitted it resolves `example.com` as the deterministic fallback.

## ExecCommand ABI

```
rax = 50
rdi = userspace pointer to command-line bytes
rsi = command-line byte length
return = does not return on success; u64::MAX on failure
```

The Stage 6 parser is deliberately bounded and simple:

- at most 8 argv entries;
- at most 512 command-line bytes;
- ASCII whitespace separates arguments;
- no shell quoting, escaping, environment expansion or globbing yet;
- a command containing `/` is used as a path;
- a bare command such as `dns` resolves to `/bin/dns`.

This means these forms now reach external Ring-3 programs with arguments:

```
dns example.com
/bin/dns example.com
/bin/cat /mnt/HELLO.TXT
/bin/mkdir /tmp/TEST
/bin/rm /tmp/TEST
```

Commands implemented as shell built-ins continue to be handled by the shell before external execution.

## Build

```powershell
cargo clean
cargo build
```

A successful normal build should finish without compiler errors or warnings from the WovenHat sources. If the compiler reports a new warning/error on the pinned nightly toolchain, treat it as a release blocker before advancing further.

## QEMU

Use `run-stage6.ps1` or the command below. The system/boot image stays on VirtIO with boot priority 1. The writable host-backed FAT directory stays on IDE so WovenHat's ATA PIO driver can mount it at `/mnt`.

## Stage 6 smoke test

At the kernel recovery prompt:

```
version
userland
net
netstat
sh
```

Inside `/bin/sh`:

```
/bin/echo hello stage6
/bin/ls /mnt
dns example.com
/bin/cat /etc/version
exit
```

Expected version: `WovenHat kernel 0.4.0 Stage 6`.

## Deferred to Stage 7

Quoting/escaping, environment variables, a mutable PATH, TCP stream command UX, ICMP target parsing from argv, and a persistent PID-1 service manager remain explicit follow-on work rather than being hidden behind hard-coded behavior.

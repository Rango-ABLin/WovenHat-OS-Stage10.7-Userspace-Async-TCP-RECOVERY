# Bounded path ABI expansion audit — 2026-09-16

The kernel path ABI now accepts canonical UTF-8 paths up to 512 bytes and
components up to 255 bytes, matching the bounded FAT32 long-name codec. VFS
nodes, task working directories, the kernel shell cwd, syscall path buffers,
and the Ring-3 `ls` directory-name buffer were updated together. VFS boundary
regressions now exercise the expanded component and total-path limits.

The ABI remains bounded and allocation-free at syscall boundaries. Other
generated Ring-3 command stubs still use their own smaller scratch buffers for
command parsing; they cannot emit arbitrarily long command lines until the
userspace command ABI is expanded separately.

# VFS QEMU baseline — 2026-09-07

Built the unchanged kernel with `cargo build` (passed). Booted the resulting
UEFI image in QEMU q35, 1024 MiB RAM, two CPUs, using a temporary copy of
OVMF variables and a read-only boot image. Tested the diagnostic shell via
QMP-injected PS/2 keys and visually inspected framebuffer screenshots.
The filesystem under test was the RAM VFS; no persistent data disk was attached.

| Commands | Observed result |
| --- | --- |
| `pwd` | `/` |
| `mkdir /home/anthony`, `mkdir /home/anthony/docs` | Both return `ok` |
| `ls /home` | Directory `anthony` |
| `cd /home/anthony`, `pwd` | `/home/anthony` |
| `ls` | Directory `docs` |
| `stat docs` | `/home/anthony/docs`, directory, size 0, writable yes |
| `cd docs`, `pwd` | `/home/anthony/docs` |
| `cd ..`, `pwd` | `/home/anthony` |
| `stat /etc/motd` | File, size 24, writable no |
| `mkdir /home/anthony` | `mkdir: exists` |
| `mkdir /missing/child` | `mkdir: parent missing` |
| `cd /missing`, `pwd` | Fails; cwd remains `/home/anthony` |
| `cd /etc/motd`, `pwd` | Fails; cwd remains `/home/anthony` |
| `stat /missing` | `stat: not found` |
| `ls /missing` | `ls: not found` |

All baseline checks passed. Screenshots and launch/input scripts are local
artifacts under `target/vfs-baseline*`; these are not committed.
This validates diagnostic-shell behavior, not the separate Ring-3 shell or
FAT32 persistence. The exhaustive `qemu-test` suite was not run in this baseline.

Next milestone: rename directories and all descendants atomically, validate
the destination parent, introduce `NotEmpty`, strengthen path validation,
and add regression self-tests. In particular, renaming `/home/anthony` to
`/home/user` must preserve the entire `docs/a.txt` subtree under the new prefix.
Buffer/Page Cache work follows that milestone.

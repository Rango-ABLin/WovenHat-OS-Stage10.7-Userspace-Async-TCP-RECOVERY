# FAT32 ownership metadata audit — 2026-09-16

FAT32 persistence now stores bounded VFS ownership metadata in four reserved
sectors immediately before the first FAT. Each sector has a `WMD1` header and
up to 21 fixed-size records, for 84 records per volume. Records contain a
canonical-path FNV-1a key, an independent 32-bit reverse-path discriminator,
uid, gid, mode, and a validity marker. Updates reuse an existing key pair or a
free slot; removal clears the validity marker. Existing records with a zero
discriminator remain readable for compatibility.

Mounted directory import reads the sidecar after entries are created in VFS and
restores uid/gid/mode. Disk-backed file persistence writes the file data first,
then updates the matching metadata record and flushes the device. The FAT32
structural regression covers write, read, remove, and Unicode-name keys; the
storage mutation gate passes on 1, 2, and 4 CPU QEMU profiles.

File delete removes its sidecar record, and file rename relocates the record to
the new canonical path. Directory subtree metadata relocation is still a
separate bounded transaction because the sidecar stores keys rather than full
path strings.

This is a bounded compatibility sidecar, not a complete production metadata
system. The bounded key pair makes accidental collisions substantially less
likely, but collision handling and a larger identity table remain open. The
update is not journaled or ordered for crash
recovery, and volumes with fewer than four usable reserved sectors cannot host
the table.

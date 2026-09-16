# Stage 12 metadata and rollback follow-up — 2026-09-16

WovenFS metadata now carries permission mode and four bounded extended-
attribute slots. Snapshot records expose restore state (generation and root
checksum) before deletion, enabling a rollback coordinator to restore a
validated generation. These additions are independent of the existing VFS
open-file lifetime rules.

Focused Clippy and QEMU tests passed on 1, 2, and 4 CPUs for WovenFS metadata
and snapshots. Native on-disk WovenFS replay and full data rollback remain
separate gaps in the production completion audit.

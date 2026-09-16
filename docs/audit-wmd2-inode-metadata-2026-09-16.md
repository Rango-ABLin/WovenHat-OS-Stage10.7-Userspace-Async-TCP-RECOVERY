# WMD2 inode metadata migration audit — 2026-09-16

FAT32 ownership metadata now has a parallel `WMD2` table keyed by the file's
first data cluster, which remains stable across file and directory renames.
`WMD1` path-hash records remain readable for compatibility. During mounted
import, a file with a valid first-cluster identity is restored from WMD2; a
legacy WMD1 record is copied into WMD2 and its old path record is retired.

New persisted files write WMD1 as the pre-data pending record, then resolve the
published FAT entry and write WMD2 before retiring the WMD1 record. This keeps
crash recovery possible for new files while making committed metadata
rename-stable. The WMD2 table has four bounded reserved sectors and is covered
by the FAT32 structural regression and 1/2/4 CPU QEMU suite.

Files with no allocated first cluster continue using the WMD1 fallback. A
future format can add directory-entry-slot identities for those legacy cases
and garbage-collect stale WMD2 records after deletion.

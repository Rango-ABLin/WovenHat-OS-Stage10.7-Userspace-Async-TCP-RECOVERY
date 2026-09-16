# FAT32 long-filename codec audit — 2026-09-16

The FAT32 layer now has a bounded, checksum-correct ASCII LFN record encoder
and decoder for standard 13-character UTF-16 directory slots. It validates
printable input, emits reverse ordinal records, and resolves existing LFN names
through path lookup. The codec and lookup paths are covered by the Stage 1–5
structural gate on 1/2/4 CPUs.

Directory allocation and long-name file creation now use generated collision
checked aliases plus contiguous LFN records. Long-name delete now resolves the
display name, removes the short entry and its preceding LFN records, and
verifies slot reuse in the in-memory FAT32 regression. Rename and VFS listing
display integration now use the decoded name during mounted-directory import;
rename relocates the bounded LFN sequence with rollback before deleting the old
sequence. The compatibility `list_directory` API remains unchanged, while the
new bounded `list_directory_named` API exposes validated display names. Long-
name allocation now extends the directory chain by up to two bounded clusters
and rolls those links back on failed publication. The codec intentionally
accepts bounded printable ASCII rather than the complete Unicode LFN space.

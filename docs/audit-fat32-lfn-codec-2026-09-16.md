# FAT32 long-filename codec audit — 2026-09-16

The FAT32 layer now has a bounded, checksum-correct ASCII LFN record encoder
and decoder for standard 13-character UTF-16 directory slots. It validates
printable input, emits reverse ordinal records, and resolves existing LFN names
through path lookup. The codec and lookup paths are covered by the Stage 1–5
structural gate on 1/2/4 CPUs.

Directory allocation and long-name file creation now use generated collision
checked aliases plus contiguous LFN records. Long-name delete/rename and VFS
listing display integration remain the next implementation step; this batch
does not yet close the full long-filename production requirement.

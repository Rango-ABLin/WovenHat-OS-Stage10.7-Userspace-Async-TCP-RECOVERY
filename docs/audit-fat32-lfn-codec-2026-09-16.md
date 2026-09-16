# FAT32 long-filename codec audit — 2026-09-16

The FAT32 layer now has a bounded, checksum-correct ASCII LFN record encoder
and decoder for standard 13-character UTF-16 directory slots. It validates
printable input, emits reverse ordinal records, and resolves existing LFN names
through path lookup. The codec and lookup paths are covered by the Stage 1–5
structural gate on 1/2/4 CPUs.

Directory allocation, name-aware lookup, rename, and VFS listing integration
remain the next implementation step; this codec alone does not close the full
long-filename production requirement.

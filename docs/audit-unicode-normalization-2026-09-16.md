# Unicode normalization audit — 2026-09-16

FAT32 long-name creation and lookup now normalize each component to Unicode
NFC using the no-std `unicode-normalization` implementation. A composed name
and its canonically decomposed equivalent resolve to the same bounded on-disk
name. Normalization occurs before UTF-16 LFN encoding and before lookup alias
matching, while the existing 255-byte UTF-8 bound remains enforced.

The regression includes `café` versus `cafe` plus combining acute and passes
with the host FAT32 test and the 1/2/4 CPU QEMU storage suite. VFS paths remain
byte-preserving until they cross the FAT32 name boundary; a future versioned
VFS canonical-path ABI can extend normalization to every in-memory node.

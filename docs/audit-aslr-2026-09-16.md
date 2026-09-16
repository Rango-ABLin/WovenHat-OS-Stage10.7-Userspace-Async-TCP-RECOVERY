# Userspace ASLR audit — 2026-09-16

Production ELF loads now apply a page-aligned randomized relocation delta and
recheck segment bounds and overlap after relocation. User stacks and per-process
anonymous/file mapping arenas receive independent randomized page-aligned bases;
the stack guard-page invariant and W^X checks remain unchanged. The `qemu-test`
feature returns deterministic zero offsets for reproducible 1/2/4-CPU gates.

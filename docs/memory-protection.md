# Memory Protection

WovenHat creates a distinct level-4 page table for every userspace process. Kernel
mappings are inherited, while the process user-region top-level entry is cleared and
rebuilt from validated ELF segments, anonymous mappings, and a private stack.

## Mapping policy

- Every userspace page must carry USER_ACCESSIBLE.
- ELF segment permissions are applied after loading; writable-executable segments are
  rejected by the ELF parser.
- Anonymous mappings are writable only when requested and are always non-executable.
- Fork eagerly clones every user page into independently owned physical frames while preserving its writable/executable policy.
- Each two-page user stack is writable and non-executable.
- One page immediately below every user stack is reserved and remains unmapped.
- Syscall copies reject unmapped pages and enforce user/write permissions per translated
  page, including buffers crossing page boundaries.

The loader queries the completed page tables and fails process creation unless the guard
is unmapped and every stack page has the expected user, writable, and NX flags. Boot
repeats these checks for two separate process address spaces and verifies distinct page
table roots.

## Current boundary

Kernel task stacks and the privilege-transition/IST stacks still use statically allocated
memory. They are isolated by ownership and dedicated storage, but they do not yet have
unmapped virtual guard pages. Adding a kernel virtual-memory allocator is required before
those stacks can be remapped with guards.

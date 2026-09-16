# Fault Handling

WovenHat classifies synchronous CPU exceptions by their originating privilege level.
Every handled exception emits the interrupt frame, error code when present, control
registers, stack pointers, flags, and timer state to the serial diagnostic channel.

## Recovery policy

Ring-3 faults are isolated to the current process:

| Exception | Vector | Exit code |
| --- | ---: | ---: |
| Divide error | 0 | -8 |
| Invalid opcode | 6 | -4 |
| General protection fault | 13 | -13 |
| Page fault | 14 | -11 |

Before termination, the kernel records a denied ProcessFault audit event containing the
process ID and exception vector. Process termination follows the normal lifecycle path:
the process becomes a zombie, anonymous mappings and its address space are reclaimed,
the IPC endpoint remains until the parent waits, and the scheduler switches to another
ready task.

Kernel-origin faults remain fatal. They retain the full diagnostic dump and halt instead
of attempting to continue with potentially corrupted privileged state. Double faults
always run on the dedicated IST stack and halt.

## Validation and boundary

Boot validates ring-3 and ring-0 selector classification after loading the IDT. Existing
two-process lifecycle checks validate normal address-space reclamation and zombie
reaping. A deliberate faulting ring-3 integration program should be added to emulator CI
when QEMU runtime tooling is available; compilation alone cannot prove the CPU exception
frame transition.

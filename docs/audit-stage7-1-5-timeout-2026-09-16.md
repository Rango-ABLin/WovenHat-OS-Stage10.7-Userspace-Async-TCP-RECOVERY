# Stage 7.1.5 pager watchdog correction — 2026-09-16

The first repeated Stage 7.1.5 run stopped at 2-CPU run 14 after the AP-owned
pager workload had loaded `/bin/mmaptest` but before process exit was observed.
The preserved serial log contained no panic, fault, or failed subsystem marker;
the 200-tick watchdog expired while QEMU was under host contention.

The pager wait remains bounded but now allows 1,000 timer ticks. The timeout
still halts the acceptance run and reports queued/completed pager work, so a
real scheduler deadlock cannot be hidden by an unbounded wait.

After the change, a focused 20-run two-CPU memory/pager stress passed 20/20,
with every run exiting QEMU with status 33 and all required markers. The
termination lifecycle probe is now decoupled from the heavy mmap ELF loader: it
uses the existing minimal `/bin/true` image, while the mmap image continues
through its own dedicated pager probe. One fresh memory/boot run on each of 1,
2, and 4 CPUs passed after this correction. The full Stage 7.1.5 matrix remains
the final acceptance gate.

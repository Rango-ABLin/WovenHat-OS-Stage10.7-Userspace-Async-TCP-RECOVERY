# Stage 7.1.5 pager watchdog correction — 2026-09-16

The first repeated Stage 7.1.5 run stopped at 2-CPU run 14 after the AP-owned
pager workload had loaded `/bin/mmaptest` but before process exit was observed.
The preserved serial log contained no panic, fault, or failed subsystem marker;
the 200-tick watchdog expired while QEMU was under host contention.

The pager wait remains bounded at 200 timer ticks. The timeout still halts the
acceptance run and reports queued/completed pager work, so a real scheduler
deadlock cannot be hidden by an unbounded wait.

The termination lifecycle probe is now decoupled from the heavy mmap ELF
loader: it uses the existing blocking shell image, while the mmap image
continues through its own dedicated pager probe. A focused 50-run one-CPU
stress and 40-run two-CPU stress passed after that correction, with fresh 1/2/4
CPU memory suites also passing. Lifecycle trace points distinguish the
pre-kill check, kill publication, post-kill observation, and wait/reap
completion. The bounded wait now yields until the target is retired before
reaping, so a scheduler hand-off cannot be reported as a false wait failure.

The complete Stage 7.1.5 acceptance gate then passed on 2026-09-16: 50 one-CPU
memory runs, 100 two-CPU runs, 50 four-CPU runs, and live DHCP/DNS/ICMP plus
host-verified UDP/TCP round trips on 1, 2, and 4 CPUs. The retained serial logs
under `target/memory-regression-*` and `target/network-regression-*` are the
freeze evidence for this gate.

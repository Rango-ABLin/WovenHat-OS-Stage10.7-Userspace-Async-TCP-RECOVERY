WovenHat OS Stage 7.5 — Multicore Pager & File-I/O Ownership
============================================================

Baseline
--------
This candidate is built directly from the validated Stage 7.4 tree.

Scope
-----
Stage 7.5 validates file-backed Ring-3 page faults and the existing pager
handshake when the faulting process executes on an application processor.

Implemented changes
-------------------
1. FILE_IO_DEPTH is now per CPU rather than one BSP-oriented global counter.
2. preempt_from_interrupt() suppresses local task preemption only on the CPU
   that currently owns an interrupt-enabled file-I/O critical region.
3. The existing /bin/mmaptest pager workload is pinned to the highest online
   AP on SMP boots. On a 1-CPU boot it follows the validated Stage 7.4 CPU0
   path unchanged.
4. Existing remote pager wake/reschedule logic is reused. No new pager worker
   topology is introduced.

Deliberately NOT enabled in Stage 7.5
-------------------------------------
- unrestricted automatic userspace load balancing
- migration of Running/Blocked/Sleeping/Switching userspace tasks
- general concurrent shell/VFS/network workloads on arbitrary CPUs
- fork-affinity inheritance changes
- per-CPU pager workers
- NUMA/hotplug/x2APIC work

Acceptance
----------
Run:

  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage7-5-acceptance.ps1

The final required line is:

  === STAGE 7.5 ACCEPTANCE: PASS ===

Important runtime markers:
- [TERM] scheduler-owned Ready-task termination + deferred reap: PASSED
- [S7.3] pinned Ring-3 execution on every AP: PASSED        (SMP)
- [S7.4] Ready-state Ring-3 migration + affinity: PASSED    (SMP)
- [S7.5] pinned AP pager/file-I/O: PASSED cpu=N             (SMP)
- [S7.5] single-CPU pager/file-I/O baseline preserved       (1 CPU)

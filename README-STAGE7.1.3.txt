WovenHat OS Stage 7.1.3

Includes:
- Stage 7.1 CPU affinity foundation
- Stage 7.1.1 AP-online / affinity publication race fix
- Stage 7.1.2 fork first-dispatch scheduler hand-off fix
- Stage 7.1.3 idle scheduler contention fix

Stage 7.1.3 changes idle_task() from a tight yield_now() loop to
preemption_point() + HLT. This prevents idle APs from continuously contending
on the global scheduler spinlock. Timer interrupts and reschedule IPIs already
wake halted CPUs and run preempt_from_interrupt(), preserving prompt scheduling.

Stage 7.1 remains unfrozen until the full acceptance matrix passes.

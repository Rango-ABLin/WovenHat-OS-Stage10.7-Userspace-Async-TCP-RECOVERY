# Kernel performance snapshots

The kernel shell exposes an allocation-free `BENCH` command for comparing
scheduler and memory activity between two points in time.

The first invocation captures a baseline and prints `BENCHMARK BASELINE
CAPTURED`. Each later invocation prints the changes since the previous sample
and then advances the baseline.

The report contains:

- `TICKS`: timer ticks elapsed.
- `SWITCHES`: scheduler context switches.
- `PREEMPTIONS`: context switches initiated by timer preemption.
- `IDLE`: idle-task heartbeat count.
- `FRAMES`: signed change in allocated physical frames.
- `HEAP_BYTES`: signed change in live heap bytes.
- `ALLOCS`: heap allocations performed during the interval.

Reading the snapshot requires both the `TaskInspect` and `MemoryInspect`
capabilities. Capturing and formatting a snapshot use fixed-size values and do
not allocate, so the measurement does not increment its own heap counters.
Counter deltas saturate instead of wrapping. Memory gauges are signed so that
released frames and bytes remain visible.

At boot, a deterministic arithmetic self-test validates scheduler deltas and
positive and negative memory changes. A successful check prints `BENCHMARK
DELTAS: VALIDATED` and is included in the QEMU validation path.

Current limitations: ticks are not converted to wall-clock nanoseconds, CPU
time is not attributed per process, and the counters describe the single-core
scheduler rather than SMP activity.

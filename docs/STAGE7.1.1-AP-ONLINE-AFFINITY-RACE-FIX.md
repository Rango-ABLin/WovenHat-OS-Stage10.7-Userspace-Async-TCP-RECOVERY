# Stage 7.1.1 — AP Online/Affinity Publication Race Fix

## Failure observed

On a 2-CPU QEMU acceptance run the kernel panicked in `task.rs` with:

`live task affinity contains an offline CPU` (`left: 2`, `right: 0`).

The 1-CPU suite passed 20/20. The failure appeared as soon as SMP was enabled.

## Root cause

`ap_main()` created the AP's permanent idle task before publishing `ONLINE[cpu] = true`.
That task immediately had an affinity mask of `1 << cpu`. Concurrently, the BSP timer
could enter the scheduler and run `validate_affinity_invariants()`. The Stage 7.1 code
computed the online affinity mask from the aggregate `COUNT`, which remains `1` until
the BSP has finished starting all APs. Thus CPU1's valid `0b10` affinity looked offline.

This was a bootstrap publication race, not a memory-corruption failure and not an SMP
deadlock.

## Fix

1. `smp::cpu_is_online(cpu)` exposes the per-CPU online publication state.
2. `smp::online_mask()` snapshots the actual published `ONLINE[]` bits.
3. The scheduler affinity mask now derives from `online_mask()` rather than aggregate
   `online_count()`.
4. The live-task owner invariant checks `cpu_is_online(task.cpu)` directly.
5. `ap_main()` publishes `ONLINE[cpu] = true` with Release ordering before inserting
   the CPU-owned idle task into the global scheduler table.

This preserves the strict affinity invariant. No assertion was removed or weakened.

## Required verification

Run:

```powershell
cargo build
cargo clippy -p wovenhat-kernel -- -D warnings
cargo test
.\run-stage7-1-acceptance.ps1
```

Acceptance still requires the complete 1/2/4 CPU and network suite to pass.

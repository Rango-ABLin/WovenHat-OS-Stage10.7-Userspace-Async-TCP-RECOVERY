# Stage 6 NUMA/topology placement audit - 2026-09-16

The scheduler now consumes bounded ACPI SRAT CPU-affinity records. Each MADT
processor entry is assigned a proximity domain when a matching SRAT processor
or x2APIC affinity entry is present. The SMP bring-up publishes the domain for
each online logical CPU. Initial placement and the conservative one-task
rebalancer prefer a same-domain CPU before comparing runnable load; systems
without SRAT retain the deterministic single-domain fallback.

The parser validates entry lengths, bounds, enabled flags, duplicate-safe CPU
matching, and the bounded entry count. ACPI's structural self-test covers a
synthetic local-APIC affinity record. The QEMU evidence below exercises the
runtime publication and scheduler path:

| CPUs | Result | Evidence |
| ---: | --- | --- |
| 1 | PASS | `target/memory-regression-1-debug/serial.log`, domains=1, mask=0x1 |
| 2 | PASS | `target/memory-regression-2-debug/serial.log`, domains=1, mask=0x3 |
| 4 | PASS | `target/memory-regression-4-debug/serial.log`, domains=1, mask=0xf |

Static validation passed with `cargo build -p wovenhat-kernel
--target x86_64-unknown-none` and warning-denying freestanding Clippy. The
QEMU matrix remains a single-domain topology, so multi-node latency and page
placement still require hardware or a NUMA-capable VM. Those are tracked as a
separate Stage 6 gap rather than inferred from this fallback result.

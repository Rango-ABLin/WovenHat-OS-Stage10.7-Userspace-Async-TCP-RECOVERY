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

The same SMP layer now supports x2APIC register access through MSR 0x800+
register windows, 64-bit destination ICR writes, and APIC-ID discovery from
the x2APIC ID register. It selects x2APIC when firmware has enabled it or
when the discovered topology contains an APIC ID above 255, and otherwise
retains xAPIC MMIO. The 1/2/4 CPU logs record `APIC mode: xAPIC`; the x2APIC
branch requires a host or VM that exposes x2APIC and is therefore not claimed
as hardware-qualified by this QEMU run.

After these changes, `python scripts/test-release.py` passed all release gates:
warning-denying kernel/host lint, standalone host regressions, 1/2/4 CPU debug
memory/storage/network, legacy-PIC fallback, 4 CPU release memory/storage/
network, release build, and shell/SMP smoke. The current freeze report is
`target/release-validation/results.json`.

Asynchronous block, file, and network workers now use the audited
`spawn_io_service` path. They can migrate between online CPUs while Ready;
their queue, VFS, ATA, and smoltcp state remains protected by global locks and
they retain no CPU-local ownership across event waits. A fresh 4-CPU memory
boot and live 4-CPU network round trip passed after this change. This closes
the scheduler-side service execution gap; unrestricted concurrent device
throughput still depends on DMA-capable drivers and remains open. The full
release matrix was rerun after this migration and passed; see
`target/release-validation/results.json`.

# Interrupt-controller topology

WovenHat discovers interrupt-controller topology from the ACPI Multiple APIC
Description Table (MADT). The table is only consumed after its SDT bounds and checksum
have been validated.

The bounded parser records:

- the 32-bit local APIC base and any 64-bit address override;
- enabled or online-capable local APIC and x2APIC processor entries;
- I/O APIC count;
- interrupt-source override count; and
- total parsed MADT entries, capped at 256.

Every entry must have a nonzero header length, remain inside the containing MADT, and
meet the minimum size for its known type. Unknown entry types are skipped by their
declared length. Malformed known entries reject ACPI discovery rather than exposing
partial topology. Boot serial output reports the resulting CPU, I/O APIC, override, and
LAPIC-address summary.

The deterministic parser fixture covers an enabled local APIC processor, an I/O APIC,
an interrupt-source override, a local APIC address override, and rejection of a
truncated processor entry.

As of 0.8.0, the bounded topology drives xAPIC CPU startup, per-CPU LAPIC timers
and keyboard IOAPIC routing (including polarity/trigger overrides). The PIC
remains a fallback when usable ACPI topology is absent. The current supported
configuration has at most four CPUs and one IOAPIC; x2APIC, multiple IOAPICs and
CPU hotplug remain unsupported. See [release notes](release-0.8.0.md).

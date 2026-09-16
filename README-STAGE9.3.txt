WovenHat OS — Stage 9.3 WovenGuard Security Audit Ledger

Stage 9.3 upgrades the existing audit ring into a structured, bounded, IRQ-safe
security ledger suitable for later Security Center and policy diagnostics.

Properties:
- allocation-free 128-event ring
- globally monotonic sequence numbers across CPUs
- kernel tick + CPU attribution
- actor/action/target/detail/outcome fields
- deterministic oldest-event overwrite accounting
- chronological bounded readout into caller-owned buffers
- IRQ-safe locking for exception/task/SMP writers
- existing audit callers remain source-compatible
- lineage issue/derive events now carry capability-rights detail

Validation:
  $env:CARGO_NET_OFFLINE = "true"
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-3-acceptance.ps1

Stage 9.3 is not considered validated until the unchanged acceptance command
passes the inherited Stage 9.2E matrix and the new marker on 1/2/4 CPUs.

WovenHat OS — Stage 7.6 Integrated Multicore Userspace Closure Candidate

BASELINE
  Validated Stage 7.5 tree only.

WHAT 7.6 ADDS
  - opt-in general multicore Ring-3 spawn API
  - least-loaded initial CPU placement inside a hard affinity mask
  - automatic Ready-state rebalancing for explicitly migratable Ring-3 tasks
  - dedicated userspace rebalance accounting
  - fork inheritance of CPU owner / affinity / migratability
  - Stage 7.5 AP mmap/pager workload routed through the new multicore API

WHAT 7.6 DOES NOT DO
  - it does not make legacy userspace automatically migratable
  - it does not migrate Running/Switching/Blocked/Sleeping tasks
  - it does not add NUMA, CPU hotplug, x2APIC, PCID or >4 CPU claims

RUN
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage7-6-acceptance.ps1

DO NOT FREEZE UNTIL
  === STAGE 7.6 ACCEPTANCE: PASS ===

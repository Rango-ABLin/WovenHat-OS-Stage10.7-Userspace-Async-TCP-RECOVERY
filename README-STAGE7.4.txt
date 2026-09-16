WovenHat OS Stage 7.4 — Ready-state Ring-3 Migration & Affinity

BASELINE
  This tree was created directly from the validated Stage 7.3 source tree.
  It does not include the abandoned all-at-once Stage-7-complete candidates.

WHAT CHANGED
  - Normal userspace remains CPU0-owned.
  - Stage 7.3 hard-pinned AP Ring-3 probes remain unchanged.
  - Added a narrow migratable Ring-3 probe class for already-audited /bin/true.
  - Ready userspace probes may use the existing scheduler migration primitive.
  - Ready userspace probes may use the existing hard-affinity primitive.
  - Added explicit CPU0 -> AP first-dispatch migration proof.
  - Added affinity-driven CPU ownership-transfer proof.
  - No shell/VFS/network/pager workload migration is enabled yet.
  - No running/switching/blocked/sleeping task migration is enabled.

TEST
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage7-4-acceptance.ps1

SUCCESS
  === STAGE 7.4 ACCEPTANCE: PASS ===

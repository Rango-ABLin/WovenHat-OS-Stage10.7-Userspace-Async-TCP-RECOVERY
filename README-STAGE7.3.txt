WovenHat OS Stage 7.3 — Pinned Ring-3 Multicore Foundation

BASELINE
  This tree was created directly from the validated Stage 7.2 source tree.
  It does not include the abandoned Stage-7-complete candidate changes.

WHAT CHANGED
  - Normal userspace remains CPU0-owned.
  - Added a narrow pinned userspace spawn path for one online CPU.
  - Added a minimal /bin/true Ring-3 AP probe.
  - Added sequential AP execution/reap proof.
  - No general userspace migration or concurrent I/O is enabled yet.

TEST
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage7-3-acceptance.ps1

SUCCESS
  === STAGE 7.3 ACCEPTANCE: PASS ===

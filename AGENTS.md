# WovenHat development rules

The user-authorized development sequence and requirements are in
`docs/master-development-roadmap.md`. Follow stages in order; do not start a
later stage until the current acceptance gate passes. Use `docs/stage-status.md`
for current status and `docs/architecture-and-codebase-guide.md` for architecture.

- Inspect repository instructions, source relationships and relevant previous
  acceptance before editing. Preserve established ABI and accepted behavior.
- Prefer safe Rust. Document and minimize unsafe code; use assembly only where
  CPU architecture requires it. Do not broaden capabilities to satisfy tests.
- Validate userspace pointers and copy or pin data before asynchronous use.
- Review owner teardown, cancellation, generation reuse, bounded capacity,
  lock order, local preemption and cross-CPU races for every new kernel object.
- Use event-driven blocking. Never sleep or switch tasks under an IRQ lock.
- Treat test failures as defects to diagnose, not reasons to weaken assertions,
  disable tests, suppress warnings or increase timeouts.
- Run build, warning-denying host/kernel Clippy, host tests and all relevant
  prior-stage/QEMU acceptance on 1/2/4 CPUs. Preserve failure and serial logs.
- Do not terminate unrelated QEMU processes or delete historical evidence.
- Update architecture, audit and status documents for each stage. Commit each
  accepted stage separately, including results, limits and next prerequisites.
- Source integration or a focused pass is not full stage acceptance. Report
  incomplete or failed gates explicitly.

Stage 10.7 full Windows gate:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\RUN-STAGE10.7.ps1
```

The launcher sets project-local Cargo build/target directories. Reuse those
environment settings for focused tests to avoid duplicate build trees. Run only
one acceptance chain at a time; the harnesses use fixed host ports and output
paths. Audit artifacts and generated build output are intentionally Git-ignored.

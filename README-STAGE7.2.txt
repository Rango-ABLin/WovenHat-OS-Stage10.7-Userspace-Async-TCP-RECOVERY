WovenHat OS Stage 7.2 — Scheduler-Owned Termination & Process Lifecycle Hardening

Purpose
-------
Stage 7.1 froze the validated SMP scheduler/pager baseline. Stage 7.2 closes the
known process-termination race before WovenHat expands userspace concurrency.

Core changes
------------
1. TaskControlBlock now carries a termination_signal request.
2. kill_process() no longer marks arbitrary live tasks Dead or publishes the
   process as Exited while that task may still execute.
3. Ready/Blocked/Sleeping targets can be retired immediately under SCHEDULER,
   because they are not physically executing.
4. Running/Switching targets retain the request until finalize_switch(), after
   the physical stack handoff is complete.
5. Self-kill is acknowledged at the syscall return boundary before iret to ring3.
6. ProcessState::Exited is published only after scheduler retirement.
7. Signal-terminated address spaces are kept as zombie resources until waitpid;
   wait_process() then destroys mappings/address space only after task retirement.
8. Existing lock order remains SCHEDULER -> PROCESS_TABLE.
9. Stage 7.2 boot acceptance includes a deterministic Ready-task SIGTERM smoke
   test and verifies exit status 143 plus deferred reap.

Why this matters
----------------
The old kill path could set another task Dead and release process resources from
outside the target CPU. That is unsafe once userspace can execute concurrently:
a target might still be using its stack, page tables, descriptors, or mappings.
Stage 7.2 makes termination a scheduler-owned state transition.

Validation
----------
Run one command:

  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage7-2-acceptance.ps1

The matrix is deliberately smaller than Stage 7.1's freeze matrix because Stage
7.1 already passed 50x1CPU + 100x2CPU + 50x4CPU. Stage 7.2 adds a deterministic
termination test and runs 10x1CPU + 25x2CPU + 10x4CPU plus network 1/2/4.

Do not freeze Stage 7.2 unless the script ends with:

  === STAGE 7.2 ACCEPTANCE: PASS ===

# WovenHat OS Stage 10.2 SMP Runtime Fix

## Trigger
The strict Stage 10.2 acceptance run passed host tests, all ten 1-CPU boots,
and 29 of 30 2-CPU boots. Run 30 timed out. The failure is intermittent and
therefore consistent with an SMP scheduling/wakeup race rather than a build or
lint defect.

## Root concurrency defects addressed

### 1. Global `IN_WORKER` was not task-local
`block_io::should_queue()` used a global AtomicBool set while the block-I/O
worker was performing ATA I/O. On SMP, another task on another CPU could
observe that flag and incorrectly take the direct-I/O path even though it was
not the worker. This breaks the intended serialization model and can expose
rare cross-CPU timing and device-access contention.

The global flag is removed. Queue bypass now depends on identity: the current
task bypasses queueing only when it is the actual registered block-I/O worker
(or when queueing is unsafe because interrupts are disabled / no normal task is
running).

### 2. Producer-to-worker wake had an enqueue-vs-sleep race
The worker previously did:

    process_one_primary() == false
    -> sleep_current(16)

A producer did:

    enqueue request
    -> wake_task(worker)

If enqueue happened after the worker's empty-queue observation but before the
worker published Sleeping, `wake_task` could legally return false because the
worker was still Running. The worker would then sleep despite queued work.
Repeated unlucky timing under SMP stress could stretch progress sufficiently
to hit the boot acceptance timeout.

The worker now uses the Stage 8.3 lost-wakeup-safe scheduler event protocol:

    worker: drain queue -> wait_for_event()
    producer: enqueue -> signal_event(worker)

If the signal arrives before the worker blocks, `event_pending` is latched. If
it arrives after Blocked is published, the signal makes the worker Ready. No
third lost-wakeup state exists.

## Acceptance policy
No timeout, stress count, marker, or preservation check was weakened. Run the
same command:

    powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\RUN-STAGE10.2.ps1


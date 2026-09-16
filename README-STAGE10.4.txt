WovenHat OS Stage 10.4 — Userspace Asynchronous Block I/O Foundation
=====================================================================

PURPOSE
-------
Stage 10.4 connects the validated Stage 10.3 Ring-3 async completion ABI to the
real Stage 10.1/10.2 block worker. It deliberately does NOT give ordinary
applications raw disk authority. Raw block submission is a privileged service
surface protected by WovenGuard StorageIo.

NEW RING-3 SYSCALLS
-------------------
69 AsyncBlockRead(lba)
   -> generation-tagged owner-bound Block handle.

70 AsyncBlockWrite(lba, user_sector_ptr)
   -> copies exactly 512 bytes into a kernel bounce buffer BEFORE queueing and
      returns a generation-tagged owner-bound Block handle.

71 AsyncBlockPoll(handle, BlockCompletion*)
   -> 0 while pending; 1 after successful copyout and consumption; -1 on error.

72 AsyncBlockWait(handle, BlockCompletion*)
   -> event-driven wait; 1 after successful copyout and consumption; -1 on error.

BlockCompletion ABI is 528 bytes:
  offset 0   i32 status (0 success, -1 device/bounds failure)
  offset 4   u32 reserved
  offset 8   u64 request id
  offset 16  u8 sector[512]
Read data is exposed only when the device operation succeeds. Write/failure
payloads are zero-filled.

SECURITY MODEL
--------------
* Ordinary SecurityDomain::User remains unchanged and does not receive StorageIo.
* Stage 10.4 adds an explicit Ring-3 SystemService spawn boundary with a caller-
  supplied capability set that must be a subset of the SystemService domain.
* The acceptance service receives only StorageIo.
* Generic AsyncCreate/Poll/Wait remain Service-class user operations. Hardware-
  backed Block handles are minted only by dedicated block submission syscalls.
* Every submit/collect operation rechecks WovenGuard Storage device authority.
* Cancellation is owner-only and remains available even after policy tightening
  so a process can always tear down authority it previously acquired.

MEMORY/LIFETIME MODEL
---------------------
The block worker NEVER keeps a Ring-3 pointer. Reads and writes live in the
existing bounded kernel request table using 512-byte kernel-owned bounce buffers.
Writes copy from userspace before the request becomes visible to the worker.
Reads are copied to Ring 3 only after completion. A failed copyout does not
consume the queue entry or generic async slot, so the owner can retry safely.

CANCELLATION / EXIT RACES
-------------------------
Requests now record their owning TaskId. Cancellation or process termination
removes the matching queue entry before releasing the generation-tagged async
slot. A worker that already copied an InProgress WorkItem may finish its hardware
transaction, but Queue::finish validates both slot and request id, so it cannot
write into a reused queue slot. Its later generic completion observes a stale
handle and cannot complete a future occupant.

ACCEPTANCE PROBE
----------------
A dedicated Ring-3 storage-service image (not the legacy userspace/SMP probes):
1. queues a real asynchronous LBA-0 read and waits event-driven;
2. confirms the consumed handle becomes stale;
3. queues a write to impossible LBA -1, proving 512-byte copy-in + real worker
   completion without modifying media (ATA rejects bounds before issuing write);
4. submits and explicitly cancels a block request, then verifies stale rejection;
5. abandons one queued read and exits, proving queue bounce-buffer + generic async
   owner teardown;
6. boot-side telemetry requires >=4 new queued requests, >=2 completed requests,
   zero live block requests, zero live generic async handles and owner-reap growth.

VALIDATION
----------
Run from the extracted source directory:

powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\RUN-STAGE10.4.ps1

The runner first executes the complete Stage 10.3 preservation chain, including
build, clippy -D warnings, host tests, 1/2/4 CPU memory/SMP stress and network
regression. It then requires the Stage 10.4 marker independently in all 1/2/4 CPU
production serial logs.

Expected final line:
  === STAGE 10.4 ACCEPTANCE: PASS ===

NEXT ARCHITECTURAL STEP
-----------------------
Stage 10.5 should build asynchronous VFS/file operations above this substrate so
ordinary applications use file-descriptor authority and filesystem sandbox scope
rather than raw StorageIo/block LBAs.


FINAL SUBMISSION-PATH FIX
- Ring-3 async block submission no longer calls the synchronous `should_queue()` predicate.
  int 0x80 enters with IF clear by design; asynchronous submission may safely copy and enqueue
  kernel-owned request state with IF=0, then return or block later through AsyncBlockWait.
- Stage 10.4 acceptance failures exit QEMU immediately with isa-debug-exit failure status instead
  of halting until the host timeout, so future faults are reported directly rather than disguised
  as hangs.

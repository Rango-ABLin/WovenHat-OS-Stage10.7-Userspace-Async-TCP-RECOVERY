WovenHat OS Stage 8.5 - Capability Handle Transfer Through IPC

Baseline
--------
This candidate is built directly on the Stage 8.4 bootstrap-stack-fix tree that
passed the complete Stage 8.4 acceptance suite.

What Stage 8.5 adds
-------------------
* One capability may be attached to a handle-addressed IPC message.
* The queue stores kernel object identity + reduced rights (escrow), never the
  sender's process-local numeric handle.
* The sender must own TRANSFER authority and may only reduce rights.
* On receive, the kernel installs a fresh generation-tagged handle in the
  receiver's own handle table and returns it through Message::transferred_handle().
* Sender closure after enqueue is safe because the queued escrow owns one object
  reference until receive or queue destruction.
* Receive is transactional: if the receiver handle table is full, the message
  stays queued and can be retried after a slot is released.
* Enqueue is transactional: if the endpoint queue is full, the temporary escrow
  retain is rolled back.
* Destroying an endpoint releases all escrow references in abandoned messages.
* Endpoint-to-endpoint capability edges are cycle-checked before enqueue so the
  refcount object graph remains reclaimable without a tracing garbage collector.
* Existing Stage 8.2/8.3 send/receive APIs remain compatible. Ordinary messages
  return no transferred handle; blocking receive inherits transfer installation
  automatically because it uses the same receive path.

No new userspace syscall ABI is exposed in this stage. Stage 8.5 validates the
kernel capability-passing semantics first; later service/syscall work can expose
that contract without changing the underlying lifetime and rights rules.

Acceptance
----------
Run from PowerShell in the repository root:

  $env:CARGO_NET_OFFLINE = "true"
  powershell.exe -NoProfile -ExecutionPolicy Bypass `
      -File .\run-stage8-5-acceptance.ps1

The script preserves the full validated Stage 8.4 suite and then requires:

  [S8.5] capability handle transfer through IPC: PASSED

on 1, 2 and 4 CPU production boots.

WovenHat OS — Stage 8.4 Shared Memory Foundation
================================================

Status in this package
----------------------
Implementation candidate only. It must pass run-stage8-4-acceptance.ps1 on the
user's Windows/QEMU environment before Stage 8.4 is declared validated.

What Stage 8.4 adds
-------------------
1. Shared-memory objects are first-class objects in the existing process-local
   opaque handle namespace. There is no second handle system.
2. Bounded, zeroed 4 KiB backing pages:
      MAX_SHARED_MEMORY_OBJECTS = 32
      MAX_SHARED_MEMORY_PAGES = 16
      MAX_SHARED_MEMORY_MAPPINGS = 32 per object
3. Shared-memory rights:
      SHM_READ
      SHM_WRITE
      SHM_MAP
   plus the existing TRANSFER and INSPECT rights.
4. Kernel APIs for:
      create_shared_memory
      read_shared_memory
      write_shared_memory
      map_shared_memory
      unmap_shared_memory
      shared_memory_info
5. Kernel-mediated rights-reducing grant_handle works for both endpoint and
   shared-memory objects. General userspace handle transfer remains Stage 8.5.
6. Shared mappings hold their own object reference. Closing all handles while a
   mapping exists does not destroy the backing frames.
7. Unregister rejects a namespace with live owned mappings (MappingBusy),
   preventing orphaned mapping metadata.
8. Mapping is NX. Writable PTEs require SHM_WRITE; read-only peer mappings are
   validated as read-only in the page tables.
9. Backing frames use the existing bounded frame-reference/COW ownership table.
   The object owns the original frame reference; every user mapping owns an
   additional reference.

Runtime proof
-------------
The Stage 8.4 boot probe creates a two-page object and two independent user page
tables. It grants the peer READ+MAP+INSPECT but not WRITE. It proves:

- wrong object rights cannot be granted;
- peer kernel writes are denied;
- peer writable mapping is denied;
- owner gets RW+NX mapping;
- peer gets RO+NX mapping;
- both page tables map the exact same physical frames;
- a write crossing the 4 KiB page boundary in the owner is visible in peer;
- direct kernel object writes are visible through the peer mapping;
- explicit unmap removes only the selected mapping;
- remap succeeds;
- closing every handle does not kill live mappings;
- namespace unregister is rejected while a mapping is live;
- final address-space teardown releases mapping references and backing frames;
- physical allocated-frame count returns exactly to its baseline.

Expected marker
---------------
[S8.4] shared-memory objects + cross-address-space mappings: PASSED

Acceptance
----------
PowerShell:

  $env:CARGO_NET_OFFLINE = "true"
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage8-4-acceptance.ps1

The acceptance script first reruns the complete validated Stage 8.3 acceptance
(build, strict Clippy, host tests, 1 CPU x10, 2 CPU x30, 4 CPU x20, network
1/2/4), then verifies the Stage 8.4 marker in the 1/2/4 CPU production logs.

Deliberately not Stage 8.4
--------------------------
- no new userspace syscall ABI yet;
- no general userspace handle transfer (Stage 8.5);
- no named service discovery (Stage 8.6);
- no heavy concurrent shared-memory/IPC stress campaign (Stage 8.7);
- no demand-paged shared memory, huge pages, NUMA placement or VM overcommit;
- no executable shared mappings.

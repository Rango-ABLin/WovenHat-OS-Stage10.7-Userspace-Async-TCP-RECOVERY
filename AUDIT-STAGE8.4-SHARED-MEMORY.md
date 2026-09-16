# Stage 8.4 Audit — Shared Memory Foundation

## Objective

Introduce capability-governed shared physical memory without weakening the
validated Stage 8.1–8.3 endpoint/message/wait contracts.

## Object model

The existing generation-tagged `Handle` remains the sole process-local
capability token. `HandleEntry` now carries a private object reference with an
object kind (`Endpoint` or `SharedMemory`). This preserves stale-handle
protection and avoids parallel authority namespaces.

Endpoint IDs retain the public `EndpointObjectId` wrapper for ABI/source
compatibility. Internally the ID namespace is shared across object kinds.

## Rights model

Endpoint rights remain unchanged:

- SEND
- RECEIVE
- TRANSFER
- INSPECT

Shared memory adds:

- SHM_READ
- SHM_WRITE
- SHM_MAP

Shared owner authority is `TRANSFER | INSPECT | SHM_READ | SHM_WRITE |
SHM_MAP`. `grant_handle` first determines the source object kind, then rejects
rights outside that kind's allowed mask and still requires the requested rights
to be a subset of the source handle.

## Physical ownership

Each shared object allocates zeroed 4 KiB backing frames. The object owns the
initial physical reference. `paging::map_shared_frame` reuses the existing
bounded frame-reference table used by shared file pages/COW. Therefore:

- object only: refcount logically 1;
- first user mapping: physical refcount 2;
- second user mapping: physical refcount 3;
- unmap/destroy drops mapping references;
- final object release drops the original reference and frees the frame.

The runtime probe requires `memory::stats().allocated_frames` to return exactly
to its pre-probe baseline.

## Mapping transaction

`map_shared_memory` does not keep the IPC lock while modifying page tables.
Instead it:

1. validates object kind and rights under IPC lock;
2. takes a temporary object reference, pinning backing frames;
3. drops IPC lock;
4. maps each physical frame into the destination page table;
5. rolls back already-mapped pages on failure;
6. records the mapping under IPC lock; the temporary pin becomes the mapping
   lifetime reference.

`unmap_shared_memory` removes the mapping record transactionally, performs the
page-table unmap without the IPC lock, restores the record if paging fails, and
only then drops the mapping reference.

## Lifetime rules

Handle references and mapping references are independent. A process may close
all handles after mapping; the mapped pages remain valid. The object and backing
frames are destroyed only after the final handle *and* final mapping reference
are gone.

`unregister` rejects `MappingBusy` if the namespace still owns mappings. This
prevents the current Stage 8.4 kernel-only API from producing orphaned mapping
records. A future userspace VM syscall layer must unmap shared regions as part
of process-address-space teardown before unregistering IPC state.

## Runtime acceptance proof

The boot probe uses two fresh, independent user address spaces and a two-page
shared object. It checks:

- shared object creation and introspection;
- rights-reducing grant;
- rejection of endpoint SEND authority on a shared object;
- rejection of SHM_WRITE from a read/map-only peer;
- rejection of a writable peer mapping;
- RW+NX owner mapping and RO+NX peer mapping;
- physical-frame identity across both address spaces;
- cross-page data coherence;
- kernel-object write coherence;
- explicit unmap/remap;
- object survival after every handle closes;
- MappingBusy on premature unregister;
- exact frame-count recovery after address-space destruction.

## Locking

The Stage 8.3 `IPC -> scheduler` nested-lock hazard remains avoided. Stage 8.4
mapping operations pin objects and release the IPC state lock before page-table
work. No scheduler lock is acquired from the shared-memory object layer.

The final backing-frame release occurs from object reference release and enters
the paging/frame reference layer. No reverse `paging -> IPC` call path was added
in Stage 8.4.

## Non-goals

Stage 8.4 does not add the final userspace syscall ABI. This mirrors Stage 8.2's
kernel-first sequencing. Stage 8.5 will add general handle transfer, Stage 8.6
service discovery, and Stage 8.7 sustained SMP IPC/shared-memory stress.

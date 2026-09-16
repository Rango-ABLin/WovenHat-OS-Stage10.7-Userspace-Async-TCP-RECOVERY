# Stage 9.2B SMP TLB Liveness Fix

## Failure observed
The full Stage 9.2B acceptance passed 23 consecutive 2-CPU boots and timed out on run 24. The preserved serial log showed Stage 8.7, Stage 9.1, Stage 9.2A and Stage 9.2B had already passed. The final marker was `[PAGER-DIAG] load mmap ELF: BEGIN`; the matching `DONE` was never emitted.

## Root cause
The ELF loader builds a brand-new user address space before publishing it to the scheduler. `map_user_range_in` and `protect_user_range_in` nevertheless performed synchronous acknowledged global TLB shootdowns for every page-table edit. Likewise `destroy_user_address_space`, used after a scheduler-retired process is reaped or for unpublished rollback address spaces, performed global shootdowns while tearing down a CR3 that no CPU may legally be running.

Those shootdowns cannot invalidate useful translations for an unpublished/retired CR3. They only introduce a cross-CPU acknowledgement dependency into loader construction and reap-time teardown. Under SMP timing this created an intermittent liveness window, observed after the termination/reap test and during the immediately following second ELF load.

## Fix
- Preserve the existing conservative public mapping/protection APIs for potentially-live address spaces.
- Add `map_user_range_in_inactive` and `protect_user_range_in_inactive` for brand-new unpublished address spaces.
- Refactor mapping/protection through shared internal helpers with an explicit `synchronize_tlb` policy.
- Make the ELF loader use the inactive-address-space variants for executable segments and the initial stack.
- Remove global TLB shootdowns from `destroy_user_address_space`; document its retired/unpublished CR3 contract.
- Keep live `unmap_user_range_in`, COW, file mappings and other runtime paths unchanged.

## Why this is safe
Before first dispatch, the new CR3 has never executed on any CPU, so no CPU can have TLB entries tagged to that address space. After scheduler-owned retirement, a reaped process is off every CPU before destruction. PTE writes are therefore visible when that CR3 is later installed, and teardown needs no cross-CPU invalidation.

## Validation requirement
Do not weaken timeouts or reduce the established acceptance matrix. Re-run the existing Stage 9.2B acceptance unchanged. The fix is accepted only if build, strict Clippy, tests, 1CPU x10, 2CPU x30, 4CPU x20, network 1/2/4, and all inherited security markers pass.

# Stage 8.4 bootstrap-stack regression fix

## Observed failure
The corrected Stage 8.4 candidate compiled, passed strict Clippy and host tests, then the first 1-CPU QEMU boot double-faulted immediately after the pre-scheduler file-mmap regression marker. The exception reported a fault address immediately below the current bootstrap RSP, which is the characteristic signature of crossing the bootstrap stack guard.

## Root-cause class
`ipc::self_test()` and `ipc::handle_object_self_test()` historically created `let mut state = State::new()` on the kernel stack. Stage 8.4 enlarged `State` by adding bounded shared-memory objects, backing-frame arrays, and mapping registries. The bootloader kernel stack is intentionally bounded at 1 MiB. Keeping a complete production registry as a local self-test value therefore became an unsafe stack-footprint pattern; code-layout/inlining changes can move the overflow point earlier than the apparent call site.

## Fix
Both early IPC self-tests now exercise the real global `STATE` under its existing lock using reserved synthetic owners. They require the registry to be pristine before testing and clean both namespaces on every success/failure path. No second full `State` is created on the bootstrap stack.

## What was deliberately NOT changed
- kernel/boot stack size remains 1 MiB;
- no warning or fault was suppressed;
- no QEMU timeout was increased;
- Stage 8.4 shared-memory rights, mapping, object lifetime, page ownership, or IPC event semantics were changed;
- prior acceptance counts remain unchanged.

## Validation requirement
Run the unchanged full `run-stage8-4-acceptance.ps1`. Stage 8.4 remains unvalidated until the complete Stage 8.3 preservation chain and S8.4 markers pass on 1/2/4 CPUs.

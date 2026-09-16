# Stage 9.4C — Filesystem / Resource Sandbox

## Objective
Make `FileRead` and `FileWrite` coarse capability gates rather than global filesystem authority. A TCB-bound `SandboxProfile` now carries an allocation-free `FilePolicy` that determines which filesystem scopes may be read or written.

## Policy model
Effective filesystem operation requires:

`capability authority ∩ SecurityDomain ceiling ∩ SandboxProfile capability ceiling ∩ FilePolicy scope permission`

Stable scope buckets are Root, System, Temporary, Mounted, Home, and Other. Classification occurs only after task-side path resolution/normalization.

## Open-descriptor closure
Checking only `open()` would be insufficient: a process could open a resource, then retain access after its sandbox is tightened. Stage 9.4C therefore stores `FileScope` in `FdKind::File`; dup/dup2/fork preserve it, while read/write/seek re-check the current sandbox profile. Close remains teardown-safe.

## File mappings
The process table now retains an optional FileScope alongside each memory mapping. New file-backed mmap operations check current policy and `msync` re-checks current write scope before flushing. Anonymous mappings carry no file scope. Unmap clears both mapping and scope metadata.

## Path operations
Scope gates are applied to open, stat, readdir, chdir, mkdir, unlink, and both source/destination paths of rename. O_CREAT additionally requires write permission for that scope.

## Audit ledger
`SandboxFileRead` and `SandboxFileWrite` records capture actor, scope, a stable path hash, and allow/deny outcome without storing path strings or allocating in the ledger.

## Runtime proof
The Stage 9.4C marker binds a restricted idle TCB to a temporary profile that allows System/Temporary reads and only Temporary writes, verifies permitted/denied scope decisions, tightens to `FilePolicy::NONE`, proves denial, checks the WovenGuard ledger, restores the original profile, and validates representative path classification.

## Scope boundary
Stage 9.4C gates new filesystem operations, descriptor use, new file-backed mappings, and `msync`. It does not synchronously tear down already resident mapped pages when policy changes; active mapping recall belongs with later resource/memory capability enforcement where TLB and mapping revocation can be coordinated safely.

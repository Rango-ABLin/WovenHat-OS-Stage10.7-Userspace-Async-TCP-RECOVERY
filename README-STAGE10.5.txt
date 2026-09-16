WovenHat OS Stage 10.5 - Userspace Asynchronous VFS/File I/O

This stage lifts the asynchronous substrate from privileged raw block access to ordinary User-domain files.

New syscalls:
  73 AsyncFileRead(fd, AsyncFileRequest*)
  74 AsyncFileWrite(fd, AsyncFileRequest*)
  75 AsyncFilePoll(handle, Completion*, data*)
  76 AsyncFileWait(handle, Completion*, data*)

AsyncFileRequest is three u64 fields: offset, buffer, length. Write payloads are copied into kernel-owned memory at submission. Read destinations are supplied only at collection, so no raw Ring-3 pointer survives an async boundary.

Correctness/security properties:
- AsyncClass::File uses generation-tagged owner-bound handles.
- Submission rechecks FileRead/FileWrite plus WovenGuard filesystem scope.
- The exact refcounted OpenFileId is cloned/pinned; fd close/reuse cannot redirect in-flight I/O.
- Operations are positional and do not mutate shared fd offsets.
- Completion collection rechecks current WovenGuard scope authority.
- Completion is copied before consumption; bad user pointers are retry-safe.
- Cancellation does not require retained authority and is safe against an in-progress worker.
- Process teardown reclaims pending/complete requests and detaches in-progress work safely.
- Ordinary User capability sets are unchanged; raw StorageIo remains privileged.

Validation:
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\RUN-STAGE10.5.ps1

WovenHat OS Stage 9.4C — Filesystem / Resource Sandbox

Stage 9.4C adds a bounded filesystem-scope policy beneath the existing
FileRead/FileWrite capabilities and Stage 9.4A SandboxProfile.

Filesystem scopes:
- Root
- System (/etc, /bin)
- Temporary (/tmp)
- Mounted (/mnt)
- Home (/home)
- Other

Security properties:
- Existing default profiles remain FilePolicy::ALL for compatibility.
- A sandbox may independently restrict readable and writable filesystem scopes.
- Canonical resolved paths are checked before open/stat/readdir/chdir/mkdir/unlink/rename.
- File descriptors retain their FileScope across dup, dup2 and fork.
- Reads/writes/seeks re-check the CURRENT sandbox profile, so profile tightening
  blocks an already-open descriptor on subsequent use.
- File-backed mmap records its FileScope. New file mappings are policy checked,
  and msync re-checks current write policy before flushing a mapped file.
- Close/unmap teardown remains available after policy tightening.
- SandboxFileRead/SandboxFileWrite decisions are recorded in the Stage 9.3 ledger.

Acceptance:
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\run-stage9-4c-acceptance.ps1

Production marker:
  [S9.4C] WovenGuard filesystem/resource sandbox: PASSED

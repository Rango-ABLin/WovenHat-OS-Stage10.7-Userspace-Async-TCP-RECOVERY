# Stage 9.4C running-code correction

This package supersedes the prior Stage 9.4C BUILD-FIX archive.

Corrections:
- Verified the `FdKind::File` match arms in `dup_current` and `dup2_current` end with commas.
- Added explicit commas to the following `PipeRead` and `PipeWrite` block arms in both matches, eliminating parser ambiguity.
- Preserved Stage 9.4C filesystem/resource sandbox semantics and all prior acceptance scripts.
- Added `RUN-STAGE9.4C.ps1`, a clean one-command launcher that sets the working directory and `CARGO_NET_OFFLINE` automatically.

Important: PowerShell prompt text (`PS C:\...>`) and prior command output must not be pasted as commands. Run only the launcher command shown with this package.

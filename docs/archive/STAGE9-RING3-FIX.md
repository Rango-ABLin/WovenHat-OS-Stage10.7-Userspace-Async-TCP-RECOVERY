# WovenHat OS 0.7.0 Stage 9 — Ring-3 Transition Fix

## Symptom
`sh` cleared the diagnostic shell and appeared to hang. Earlier `/bin/*` programs also failed to become interactive.

## Root cause
The GDT user code/data descriptors are DPL3, but the selectors supplied to the user-mode `iretq` path retained RPL0. A CPL0 -> CPL3 return requires user selectors whose requestor privilege level is 3.

## Fix
`task::prepare_user_context()` now sets the low two selector bits for both CS and SS/DS:
- user CS = `code_segment.0 | 3`
- user data = `data_segment.0 | 3`

This applies centrally to both fresh userspace spawns and `exec_current()`, so it fixes `/bin/sh` and all other Ring-3 programs.

The previous Stage 9 fixes remain present:
- prepared argv stack uses `program.image.stack_top`
- malformed literal `\\n` source separators are removed
- only one `process_count()` implementation remains
- `warnings = "deny"` stays enabled

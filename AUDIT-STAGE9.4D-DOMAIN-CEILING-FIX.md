# Stage 9.4D Domain-Ceiling Runtime Fix

## Failure
The first 1-CPU QEMU preservation run timed out after build, clippy, and host tests passed.

## Root cause
Stage 9.4D workers are created by `spawn_on`, whose TCB initialization assigns `SecurityDomain::SystemService`. The final non-resurrection phase attempted to bind `SandboxProfile::KERNEL_TRUSTED`. WovenGuard correctly rejects that profile because it exceeds the SystemService domain ceiling. The Stage 9.4D probe therefore returned false and the boot failure path halted, which appeared externally as a QEMU timeout.

## Fix
The final re-broadening phase now binds `SandboxProfile::SYSTEM_SERVICE`, the broadest default profile valid for the worker's actual domain. Workers expect that same profile. The security property remains unchanged: `DeviceIo` was destructively removed during the first tightening and must remain absent after re-broadening.

No timeout, stress count, warning policy, or acceptance criterion was weakened.

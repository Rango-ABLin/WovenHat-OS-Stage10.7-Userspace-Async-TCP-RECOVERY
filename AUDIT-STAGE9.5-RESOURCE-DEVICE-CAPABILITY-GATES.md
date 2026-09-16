# Stage 9.5 — Resource / Device Capability Gates

## Security objective
Move WovenHat away from one broad `DeviceIo` permission toward explicit,
auditable resource authority before driver isolation and graphics/input work.

## Capability decomposition
- `NetworkIo`: socket/DNS/DHCP/ping resource access; included in normal userspace.
- `StorageIo`: direct storage-device resource access; SystemService/Kernel only.
- `DisplayIo`: direct display resource access; SystemService/Kernel only.
- `InputIo`: direct input-device resource access; SystemService/Kernel only.
- `DeviceIo`: retained as the legacy/generic raw-device class during migration.

## Sandbox device policy
`SandboxProfile` now carries an allocation-free `DevicePolicy` bitmask. A
resource operation succeeds only when the caller has the class capability AND
the current sandbox allows the device class.

## Real enforcement
Network-facing syscalls call `task::authorize_current_device(Network)` before
resource use. `socket_close` intentionally remains teardown-safe so policy
tightening cannot trap a resource handle that must be released.

## Audit
Each runtime gate emits `SandboxDeviceAccess` with the device class as target,
the required capability as detail, and explicit allow/deny outcome.

## Compatibility
The User domain and `CapabilitySet::userspace()` include `NetworkIo`, preserving
existing validated userspace networking. Direct storage/display/input resource
caps are restricted to SystemService and Kernel domains.

## Acceptance
The Stage 9.5 acceptance recursively preserves the full Stage 9.4D chain, then
requires the Stage 9.5 production marker on 1/2/4 CPU boots. No stress count,
timeout or prior acceptance criterion is weakened.

## Build fix — 2026-09-13
The first Stage 9.5 candidate failed strict `-D warnings` because the new
`DeviceClass::Display` and `DeviceClass::Input` variants were defined and
mapped to capabilities but were not constructed by the production proof.
The proof now explicitly verifies that a SystemService profile can authorize
both DisplayIo and InputIo. This removes the dead-code failure by exercising
the intended policy paths; no lint suppression or acceptance weakening was
added.

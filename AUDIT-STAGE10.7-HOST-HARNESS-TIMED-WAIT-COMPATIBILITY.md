# Stage 10.7 Host Harness Timed-Wait Compatibility Repair

Date: 2026-09-18

## Scope

The Stage 10.7 SMP liveness repair changed the production async-network worker to use the kernel's timed scheduler wait (`timer::ticks()` plus `task::wait_for_event_until()`) as a bounded liveness backstop after each queue pass.

The production 4-CPU Stage 10.7 QEMU test passed, but the preservation acceptance chain failed earlier in the host `async_network` test because that test compiles the production worker against local scheduler/timer doubles. Those doubles still exposed only the older `wait_for_event()` surface and no `timer` module.

## Repair

`tests/async_network.rs` now supplies the two production interfaces required by the worker:

- `timer::ticks() -> u64`, returning a deterministic host-test tick value.
- `task::wait_for_event_until(u64)`, using the same `Parked` unwind boundary as the existing `wait_for_event()` double.

This is a host-harness compatibility repair only. It does not alter the production networking, scheduler, async-operation ABI, socket semantics, cancellation, teardown, or SMP liveness implementation.

## Validation requirement

Run the authoritative Windows Stage 10.7 gate. The repair is not accepted until the host tests and the preservation/current 1/2/4-CPU QEMU gates pass.

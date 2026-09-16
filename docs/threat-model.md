# WovenHat OS threat model

## Scope and security objective

WovenHat protects kernel integrity, process isolation, capability-scoped
resources, and filesystem/network data from a malicious or compromised Ring-3
application. The kernel is the reference monitor: every privileged syscall,
IPC transfer, file operation, mapping, and device operation must pass its
capability and ownership checks. WovenGuard provenance may further narrow a
delegated capability and revocation must take effect before a subsequent use.

## Assets

- kernel code, page tables, interrupt state, scheduler ownership, and kernel
  stacks;
- process address spaces, credentials, handles, IPC payloads, and shared-memory
  mappings;
- filesystem contents, metadata, journal intents, snapshots, and storage keys;
- network packets, socket state, and device/MMIO state;
- audit records used to investigate privileged actions.

## Adversaries

The primary attacker controls a Ring-3 program and may submit malformed syscall
arguments, ELF files, paths, IPC messages, capability transfers, network data,
or filesystem structures. A compromised isolated driver is treated as an
untrusted service with only its delegated IPC/device capabilities. A network
peer may send arbitrary packets. Physical attackers, firmware compromise,
malicious boot media, and a compromised hypervisor are outside the kernel-only
model and require secure/measured boot and hardware protections.

## Security boundaries and controls

- Ring transitions, pointer/range validation, guarded user stacks, ELF segment
  validation, and W^X prevent direct kernel or executable-writable mappings.
- Generation-safe handles, owner checks, process teardown, IPC capability
  escrow, lineage revocation, and sandbox file/device scopes prevent confused
  deputy and use-after-revoke failures.
- Scheduler ownership, TLB shootdowns, bounded queues, cancellation, and
  teardown paths prevent a task from retaining another task's resources.
- FAT32/WovenFS metadata records, checksums, durable intents, snapshots, and
  recovery markers detect or contain storage corruption within their documented
  crash model.
- Network and device services are capability-gated and auditable; QEMU
  virtio/PIO drivers remain platform-specific until broader drivers land.

## Assumptions and residual risk

The model assumes the boot chain and firmware initially load an authentic
kernel, the CPU enforces paging and privilege transitions, and cryptographic
keys are provisioned by a trusted key vault. It does not claim protection from
physical memory access, DMA by an unisolated device, speculative-execution
side channels, firmware/hypervisor compromise, or power loss during an
uncalibrated storage write. Production completion therefore still requires a
signed/measured boot chain, IOMMU/DMA isolation, production AEAD/key storage,
unclean-shutdown testing, broader hardware drivers, and an independent red-team
assessment.

This document is the security design baseline; each residual item remains an
explicit acceptance gate in the stage production-gap audit.

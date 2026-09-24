# TCP close/drain preservation defect - 2026-09-24

## Reproduction

During this-machine physical-inventory work, the full Stage 10.7 preservation
gate failed on its final 4-CPU TCP boot. Earlier 1/2/4-CPU runs had passed;
the failure was intermittent and was not treated as acceptance.

Original failed gate: `audit-artifacts/acceptance-20260924-164802-963/`.
Diagnosis/focused evidence: `audit-artifacts/ax201-host-preflight-20260924-164148/`.
The first instrumented retry passed, the next failed, and a further run
captured `closing TCP unpin tx_queue=16` immediately before socket removal.
The host was waiting for connection A's TX; the guest later failed assertion
19 in connection B's receive/copyout checks. Logs are retained under
`tcp-reproduction/`, `before-close-fix/`, and `tcp-queued-close.log`.

## Cause and correction

`socket_close`, `close_process_sockets` and last-reference `unpin_socket`
removed a smoltcp transport as soon as no async references remained. A send
completion means bytes entered the TCP buffer, so last-reference release can
precede interface transmission/acknowledgement. Removal discarded that buffer.

Descriptor close still revokes process access immediately. TCP transport
retirement now begins only after all existing async references drain. It
calls smoltcp's graceful close, retains the existing slot/transport during
the FIN/data exchange, and reaps it from `network::poll` when smoltcp reports
it no longer open. UDP and unopened TCP sockets still retire immediately.
Owner teardown uses the same path. A newly defined 30-second retirement
grace bounds retention for an unresponsive peer; on the next polling pass
after expiry the transport is aborted and the slot released. This is not
a delivery guarantee for a dead peer, nor a wall-clock guarantee if network
polling stops. smoltcp treats CLOSED and TIME-WAIT as not open; this change
does not add a separate long-lived TIME-WAIT tuple table.

The 16-slot capacity remains fixed and retiring sockets remain counted in
`NetStats.user_sockets`. Slots and generations cannot be reused prematurely.
The runtime IRQ mutex protects state transitions and socket-set mutation;
there is no new lock, allocation class, unsafe code, syscall or capability.
Existing generation-tagged async references keep their ownership until
consumption/cancellation/owner cleanup. No wait or task switch occurs under
the runtime lock. Interface polling and notification order are preserved.

## Acceptance strengthening

The TCP cleanup loop now waits for both async requests and all transport
slots to return to baseline. Its existing 200-tick bound is unchanged.
All original assertions remain, including exact payloads, invalid copyout,
close-before-completion, EOF, cancellation and owner cleanup. Host receive
timeout remains five seconds and QEMU timeout remains 180 seconds.

The assembly acceptance stub stores an assertion number in unused stack
scratch before each conditional failure branch. MOV preserves the comparison
flags; success still exits 0 and failure still exits nonzero. There are no
ABI changes. The kernel reports this number, and the host logs which of the
two payload exchanges completed. An initial diagnostic formatting build
error is also preserved in `tcp-diagnostic-4.log`.

## Results and limits

The first corrected 4-CPU run and ten further 4-CPU repeats passed. Corrected
repeats include actual 16-byte queued-close cases with successful host
verification. The complete `RUN-STAGE10.7.ps1` rerun passed, exit 0, under
`audit-artifacts/acceptance-20260924-171843-691/`: 75 QEMU boots on 1/2/4 CPUs,
35 Rust tests, nine Python harness tests, build and warning-denying Clippy.
Stages 10.8, 10.9 and 13.9 passed their warning-denying feature Clippy and
1/2/4-CPU QEMU checks. Stage 13.9 required all 54 Wi-Fi markers. Host-test
Clippy and the normal 4-CPU release shell/PS2/IOAPIC/SMP smoke also passed.
Focused results are in `ax201-host-preflight-20260924-164148/results.json`;
release serial/QEMU evidence is retained in its `release-shell-evidence/`.

The 30-second dead-peer expiry path is reviewed but not separately fault-
injected in this pass. This repair establishes no physical AX201 firmware,
DMA, interrupt or RF acceptance, and does not close the broader production
network-worker/physical-driver gaps.

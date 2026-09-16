# Stage 10.4 Preservation Isolation Fix

The first Stage 10.4 package executed its new real block-I/O Ring-3 probe in every `qemu-test` production boot. That meant the Stage 7.6 preservation stress suite was no longer testing the accepted Stage 10.3 workload: each legacy boot also performed new Stage 10.4 storage work. The first 1-CPU preservation boot timed out before a Stage 10.4 marker was produced.

This fix introduces the compile-time feature `stage10-4-test`. Ordinary `qemu-test` builds preserve the exact Stage 10.3 runtime path and do not spawn the Stage 10.4 storage service. The Stage 10.4 acceptance runner first completes the entire Stage 10.3 preservation chain, then performs separate 1/2/4 CPU boots with `stage10-4-test` enabled and requires the Stage 10.4 marker.

This is test isolation, not a timeout increase or weakened assertion. The Stage 10.4 implementation, WovenGuard StorageIo boundary, bounce buffers, cancellation, stale-handle checks and teardown remain intact.

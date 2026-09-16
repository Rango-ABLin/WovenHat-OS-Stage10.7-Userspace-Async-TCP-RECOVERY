# Notification recipient reclamation audit — 2026-09-16

Process teardown now removes queued notifications addressed to the exiting
process. The bounded queue is compacted in FIFO order, reclaimed slots are
immediately reusable, and the Stage 11.3 structural gate covers both removal
and preservation of another recipient's event.

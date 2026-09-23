#[path = "../kernel/src/irq_mailbox.rs"]
mod irq_mailbox;
use irq_mailbox::Mailbox;
use std::sync::{Arc, Barrier};

#[test]
fn coalesces_without_losing_a_publication_after_claim() {
    let mailbox = Mailbox::new();
    assert!(!mailbox.activate(0));
    assert!(!mailbox.activate(u64::MAX));
    assert!(mailbox.activate(1));
    assert!(!mailbox.activate(2));
    assert!(mailbox.publish(1));
    assert!(mailbox.publish(1));
    assert_eq!(mailbox.claim(), Some(1));
    assert_eq!(mailbox.claim(), None);
    assert!(mailbox.publish(1));
    assert_eq!(mailbox.claim(), Some(1));
}

#[test]
fn stale_producer_and_retire_cannot_affect_rebound_subscription() {
    let mailbox = Mailbox::new();
    assert!(mailbox.activate(7));
    let captured = mailbox.epoch();
    assert!(mailbox.publish(captured));
    mailbox.retire(captured);
    assert_eq!(mailbox.claim(), None);
    assert!(mailbox.activate(8));
    assert!(!mailbox.publish(captured));
    mailbox.retire(captured);
    assert_eq!(mailbox.epoch(), 8);
    assert!(mailbox.publish(8));
    assert_eq!(mailbox.claim(), Some(8));
}

#[test]
fn concurrent_publishers_survive_claim_and_retirement() {
    let mailbox = Arc::new(Mailbox::new());
    let barrier = Arc::new(Barrier::new(5));
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let mailbox = Arc::clone(&mailbox);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                for epoch in 1..=100 {
                    barrier.wait();
                    assert!(mailbox.publish(epoch));
                    barrier.wait();
                    // This publication races retirement/rebinding. Whether it wins
                    // before retirement or is rejected, it must not tag epoch+1.
                    let _ = mailbox.publish(epoch);
                    barrier.wait();
                }
            })
        })
        .collect();
    assert!(mailbox.activate(1));
    for epoch in 1..=100 {
        barrier.wait();
        barrier.wait();
        assert_eq!(mailbox.claim(), Some(epoch));
        mailbox.retire(epoch);
        let next = if epoch == 100 { 0 } else { epoch + 1 };
        if next != 0 {
            assert!(mailbox.activate(next));
        }
        barrier.wait();
        assert_eq!(mailbox.epoch(), next);
        assert_eq!(mailbox.claim(), None);
    }
    for worker in workers {
        worker.join().unwrap();
    }
}

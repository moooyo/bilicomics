use std::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, Wake, Waker},
};

use super::*;

fn event(epoch: u64, sequence: u64, key: &str) -> JobEvent {
    JobEvent { epoch, sequence, key: key.to_owned(), apply: Box::new(|_| {}) }
}

#[derive(Default)]
struct WakeCounter(AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) { self.0.fetch_add(1, Ordering::SeqCst); }
    fn wake_by_ref(self: &Arc<Self>) { self.0.fetch_add(1, Ordering::SeqCst); }
}

fn poll_once<F: Future>(future: Pin<&mut F>, counter: &Arc<WakeCounter>) -> Poll<F::Output> {
    let waker = Waker::from(counter.clone());
    future.poll(&mut Context::from_waker(&waker))
}

#[test]
fn regular_keys_are_bounded_and_replacement_does_not_consume_a_new_key() {
    let mailbox = JobMailbox::new();
    let tickets = (0..MAX_REGULAR_KEYS).map(|index|
        mailbox.begin(1, index as u64, &format!("regular-{index}"), false).unwrap()
    ).collect::<Vec<_>>();
    assert!(matches!(mailbox.begin(1, 200, "overflow", false), Err(CapacityError::RegularKeys)));
    let replacement = mailbox.begin(1, 201, "regular-0", false).unwrap();
    assert!(!tickets[0].is_current());
    assert!(replacement.is_current());
    assert_eq!(mailbox.inner.lock().latest.len(), MAX_REGULAR_KEYS);
    drop(tickets);
    assert!(replacement.is_current());
    assert!(mailbox.begin(1, 202, "available", false).is_ok());
}

#[test]
fn speculative_busy_metadata_lock_does_not_wait_or_register_a_key() {
    let mailbox = JobMailbox::new();
    let state = mailbox.inner.lock();
    assert!(mailbox.try_begin_with_delivery(1, 1, "speculative").unwrap().is_none());
    assert!(state.latest.is_empty());
    assert!(state.ready.is_empty());
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), SPECULATIVE_DELIVERY_CAPACITY);
    drop(state);
    let admitted = mailbox.try_begin_with_delivery(1, 2, "speculative").unwrap().unwrap();
    assert!(admitted.0.is_current());
    drop(admitted);
    assert!(mailbox.inner.lock().latest.is_empty());
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), SPECULATIVE_DELIVERY_CAPACITY);
}

#[tokio::test]
async fn speculative_delivery_is_independent_of_all_regular_slots() {
    let mailbox = JobMailbox::new();
    let mut held = Vec::new();
    for index in 0..DELIVERY_CAPACITY {
        let ticket = mailbox.begin(1, index as u64, &format!("held-{index}"), false).unwrap();
        let permit = ticket.acquire_delivery().await.unwrap();
        held.push((ticket, permit));
    }
    let speculative = mailbox.try_begin_with_delivery(1, 100, "speculative").unwrap().unwrap();
    for sequence in 100..132 {
        assert!(mailbox.try_begin_with_delivery(1, sequence, "speculative").unwrap().is_none());
        assert!(mailbox.try_begin_with_delivery(1, sequence, "other-speculative").unwrap().is_none());
    }
    assert!(held[0].0.is_current());
    assert!(speculative.0.is_current());
    assert_eq!(mailbox.inner.lock().latest.len(), DELIVERY_CAPACITY + 1);
    assert!(mailbox.inner.lock().ready.is_empty());
    assert_eq!(mailbox.inner.delivery.available_permits(), 0);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), 0);
    let regular = mailbox.begin(1, 200, "regular-waiter", false).unwrap();
    let counter = Arc::new(WakeCounter::default());
    let mut waiting = Box::pin(regular.acquire_delivery());
    assert!(poll_once(waiting.as_mut(), &counter).is_pending());
    drop(held.pop());
    let Poll::Ready(Some(permit)) = poll_once(waiting.as_mut(), &counter) else {
        panic!("Speculative work must not hold a released regular result slot");
    };
    drop(waiting);
    drop(permit);
    drop(regular);
    drop(speculative);
    assert_eq!(mailbox.inner.delivery.available_permits(), 1);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), SPECULATIVE_DELIVERY_CAPACITY);
    drop(held);
    assert!(mailbox.inner.lock().latest.is_empty());
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
}

#[test]
fn speculative_duplicate_preserves_the_ready_callback_and_its_wakeup() {
    let mailbox = JobMailbox::new();
    let notifications = Arc::new(AtomicUsize::new(0));
    let notified = notifications.clone();
    mailbox.set_wakeup(Arc::new(move || { notified.fetch_add(1, Ordering::SeqCst); }));
    let dropped = Arc::new(AtomicUsize::new(0));
    let outside = Arc::new(AtomicBool::new(true));
    let (ticket, permit) = mailbox.try_begin_with_delivery(1, 1, "reader").unwrap().unwrap();
    assert!(ticket.publish(probed_event(&mailbox, 1, &dropped, &outside), permit));
    assert!(mailbox.try_begin_with_delivery(1, 2, "reader").unwrap().is_none());
    assert!(mailbox.try_begin_with_delivery(1, 3, "other-speculative").unwrap().is_none());
    assert!(mailbox.is_current(1, 1, "reader"));
    assert_eq!(mailbox.inner.lock().latest.len(), 1);
    assert_eq!(mailbox.inner.lock().ready.len(), 1);
    assert!(mailbox.inner.lock().wakeup_scheduled);
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), 0);
    let taken = mailbox.take().unwrap();
    assert_eq!(taken.sequence, 1);
    drop(taken);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert!(outside.load(Ordering::SeqCst));
    assert!(mailbox.inner.lock().latest.is_empty());
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), SPECULATIVE_DELIVERY_CAPACITY);
}

#[test]
fn speculative_key_neither_consumes_nor_bypasses_regular_capacity() {
    let mailbox = JobMailbox::new();
    let held = (0..MAX_REGULAR_KEYS - 1).map(|index| {
        mailbox.begin(1, index as u64, &format!("held-{index}"), false).unwrap()
    }).collect::<Vec<_>>();
    let (ticket, permit) = mailbox.try_begin_with_delivery(1, 200, "speculative").unwrap().unwrap();
    let last_regular = mailbox.begin(1, 201, "last-regular", false).unwrap();
    assert!(matches!(mailbox.begin(1, 202, "overflow", false),
        Err(CapacityError::RegularKeys)));
    assert!(matches!(mailbox.begin(1, 203, "speculative", false),
        Err(CapacityError::RegularKeys)));
    assert!(mailbox.try_begin_with_delivery(1, 204, "held-0").unwrap().is_none());
    assert!(ticket.is_current());
    assert!(held[0].is_current());
    assert_eq!(mailbox.inner.lock().latest.len(), MAX_REGULAR_KEYS + 1);
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
    drop(ticket);
    assert_eq!(mailbox.inner.lock().latest.len(), MAX_REGULAR_KEYS);
    assert!(mailbox.try_begin_with_delivery(1, 205, "speculative").unwrap().is_none());
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), 0);
    drop(permit);
    let next = mailbox.try_begin_with_delivery(1, 206, "speculative-at-capacity").unwrap().unwrap();
    assert!(next.0.is_current());
    assert_eq!(mailbox.inner.lock().latest.len(), MAX_REGULAR_KEYS + 1);
    drop(next);
    drop(last_regular);
    drop(held);
    assert!(mailbox.inner.lock().latest.is_empty());
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), SPECULATIVE_DELIVERY_CAPACITY);
}

#[test]
fn speculative_delivery_stays_reserved_until_the_published_callback_is_consumed() {
    let mailbox = JobMailbox::new();
    let (ticket, permit) = mailbox.try_begin_with_delivery(1, 1, "speculative").unwrap().unwrap();
    assert!(ticket.is_current());
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), 0);
    assert!(ticket.publish(event(1, 1, "speculative"), permit));
    let taken = mailbox.take().unwrap();
    assert!(mailbox.inner.lock().latest.is_empty());
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), 0);
    assert!(mailbox.try_begin_with_delivery(1, 2, "next-speculative").unwrap().is_none());
    drop(taken);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), SPECULATIVE_DELIVERY_CAPACITY);
    let next = mailbox.try_begin_with_delivery(1, 3, "next-speculative").unwrap().unwrap();
    drop(next);
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), SPECULATIVE_DELIVERY_CAPACITY);
}

#[test]
fn speculative_close_rejects_admission_and_releases_only_finished_delivery() {
    let mailbox = JobMailbox::new();
    let (ticket, permit) = mailbox.try_begin_with_delivery(1, 1, "speculative").unwrap().unwrap();
    mailbox.close();
    assert!(!ticket.is_current());
    assert!(matches!(mailbox.try_begin_with_delivery(1, 2, "after-close"),
        Err(CapacityError::Closed)));
    assert!(mailbox.inner.lock().latest.is_empty());
    assert!(mailbox.inner.lock().ready.is_empty());
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), 0);
    assert!(!ticket.publish(event(1, 1, "speculative"), permit));
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), SPECULATIVE_DELIVERY_CAPACITY);
    assert!(mailbox.take().is_none());
}

#[tokio::test]
async fn speculative_delivery_preserves_the_last_regular_slot() {
    let mailbox = JobMailbox::new();
    let mut held = Vec::new();
    for index in 0..DELIVERY_CAPACITY - 1 {
        let ticket = mailbox.begin(1, index as u64, &format!("held-{index}"), false).unwrap();
        let permit = ticket.acquire_delivery().await.unwrap();
        held.push((ticket, permit));
    }
    let speculative = mailbox.try_begin_with_delivery(1, 100, "speculative").unwrap().unwrap();
    let regular = mailbox.begin(1, 101, "reader-light-write", false).unwrap();
    let counter = Arc::new(WakeCounter::default());
    let mut waiting = Box::pin(regular.acquire_delivery());
    let Poll::Ready(Some(permit)) = poll_once(waiting.as_mut(), &counter) else {
        panic!("Speculative work must leave the last regular result slot available");
    };
    drop(waiting);
    drop(permit);
    drop(regular);
    assert_eq!(mailbox.inner.delivery.available_permits(), 1);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), 0);
    drop(speculative);
    drop(held);
    assert!(mailbox.inner.lock().latest.is_empty());
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), SPECULATIVE_DELIVERY_CAPACITY);
}

#[test]
fn speculative_invalidation_and_clear_keep_actual_delivery_reserved() {
    for clear in [false, true] {
        let mailbox = JobMailbox::new();
        let (ticket, permit) = mailbox.try_begin_with_delivery(1, 1, "speculative").unwrap().unwrap();
        if clear { mailbox.clear(); } else { mailbox.invalidate("speculative"); }
        assert!(!ticket.is_current());
        assert!(mailbox.inner.lock().latest.is_empty());
        assert!(mailbox.try_begin_with_delivery(1, 2, "replacement").unwrap().is_none());
        assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
        assert_eq!(mailbox.inner.speculative_delivery.available_permits(), 0);
        assert!(!ticket.publish(event(1, 1, "speculative"), permit));
        let replacement = mailbox.try_begin_with_delivery(1, 3, "replacement").unwrap().unwrap();
        drop(replacement);
        assert!(mailbox.inner.lock().latest.is_empty());
        assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
        assert_eq!(mailbox.inner.speculative_delivery.available_permits(), SPECULATIVE_DELIVERY_CAPACITY);
    }
}

#[tokio::test]
async fn speculative_key_stays_exclusive_while_recovering_lost_delivery() {
    let mailbox = JobMailbox::new();
    let (ticket, permit) = mailbox.try_begin_with_delivery(1, 1, "speculative").unwrap().unwrap();
    drop(permit);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), SPECULATIVE_DELIVERY_CAPACITY);
    assert!(mailbox.try_begin_with_delivery(1, 2, "other-speculative").unwrap().is_none());
    assert!(ticket.is_current());
    let recovered = ticket.acquire_delivery().await.unwrap();
    assert!(Arc::ptr_eq(recovered.semaphore(), &mailbox.inner.speculative_delivery));
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
    assert!(ticket.publish(event(1, 1, "speculative"), recovered));
    drop(mailbox.take().unwrap());
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), SPECULATIVE_DELIVERY_CAPACITY);
}

#[tokio::test]
async fn publication_rejects_a_permit_from_the_other_delivery_lane() {
    let mailbox = JobMailbox::new();
    let (speculative, speculative_permit) = mailbox.try_begin_with_delivery(1, 1, "speculative")
        .unwrap().unwrap();
    let regular = mailbox.begin(1, 2, "regular", false).unwrap();
    let regular_permit = regular.acquire_delivery().await.unwrap();
    assert!(!speculative.publish(event(1, 1, "speculative"), regular_permit));
    assert!(regular.is_current());
    assert!(!regular.publish(event(1, 2, "regular"), speculative_permit));
    assert!(mailbox.inner.lock().latest.is_empty());
    assert!(mailbox.inner.lock().ready.is_empty());
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), SPECULATIVE_DELIVERY_CAPACITY);
}

#[tokio::test]
async fn independent_delivery_lanes_preserve_regular_result_order_at_capacity() {
    let mailbox = JobMailbox::new();
    let counter = Arc::new(WakeCounter::default());
    for sequence in 0..DELIVERY_CAPACITY as u64 {
        if sequence == 4 {
            let (ticket, permit) = mailbox.try_begin_with_delivery(1, 100, "speculative").unwrap().unwrap();
            assert!(ticket.publish(event(1, 100, "speculative"), permit));
        }
        let key = format!("regular-{sequence}");
        let ticket = mailbox.begin(1, sequence, &key, false).unwrap();
        let mut delivery = Box::pin(ticket.acquire_delivery());
        let Poll::Ready(Some(permit)) = poll_once(delivery.as_mut(), &counter) else {
            panic!("The independent result lanes must hold all regular results");
        };
        drop(delivery);
        assert!(ticket.publish(event(1, sequence, &key), permit));
    }
    assert_eq!(mailbox.inner.lock().ready.len(), DELIVERY_CAPACITY + SPECULATIVE_DELIVERY_CAPACITY);
    let taken = (0..DELIVERY_CAPACITY + SPECULATIVE_DELIVERY_CAPACITY)
        .map(|_| mailbox.take().unwrap()).collect::<Vec<_>>();
    assert_eq!(taken.iter().map(|event| event.sequence).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 100, 4, 5, 6, 7]);
    assert!(mailbox.inner.lock().latest.is_empty());
    assert_eq!(mailbox.inner.delivery.available_permits(), 0);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), 0);
    assert!(mailbox.try_begin_with_delivery(1, 101, "next-speculative").unwrap().is_none());
    drop(taken);
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
    assert_eq!(mailbox.inner.speculative_delivery.available_permits(), SPECULATIVE_DELIVERY_CAPACITY);
}

#[tokio::test]
async fn taking_a_result_reclaims_its_key_but_keeps_delivery_until_consumed() {
    let mailbox = JobMailbox::new();
    let mut tickets = (0..MAX_REGULAR_KEYS).map(|index|
        mailbox.begin(1, index as u64, &format!("regular-{index}"), false).unwrap()
    ).collect::<Vec<_>>();
    let ticket = tickets.pop().unwrap();
    let sequence = MAX_REGULAR_KEYS as u64 - 1;
    let key = format!("regular-{sequence}");
    let permit = ticket.acquire_delivery().await.unwrap();
    assert!(ticket.publish(event(1, sequence, &key), permit));
    assert!(matches!(mailbox.begin(1, 200, "overflow", false), Err(CapacityError::RegularKeys)));
    let taken = mailbox.take().unwrap();
    assert_eq!(taken.sequence, sequence);
    assert!(mailbox.begin(1, 201, "available", false).is_ok());
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY - 1);
    drop(taken);
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
}

#[tokio::test]
async fn stale_completion_and_ticket_drop_cannot_retire_a_new_generation() {
    let mailbox = JobMailbox::new();
    let previous = mailbox.begin(1, 1, "reader", false).unwrap();
    let permit = previous.acquire_delivery().await.unwrap();
    let current = mailbox.begin(1, 2, "reader", false).unwrap();
    assert!(!previous.publish(event(1, 1, "reader"), permit));
    assert!(current.is_current());
    let permit = current.acquire_delivery().await.unwrap();
    assert!(current.publish(event(1, 2, "reader"), permit));
    assert_eq!(mailbox.take().unwrap().sequence, 2);
    assert!(mailbox.take().is_none());
    assert!(mailbox.inner.lock().latest.is_empty());
}

#[tokio::test]
async fn repeated_queued_replacement_coalesces_without_generation_tombstones() {
    let mailbox = JobMailbox::new();
    for sequence in 0..64 {
        let ticket = mailbox.begin(1, sequence, "reader", false).unwrap();
        let permit = ticket.acquire_delivery().await.unwrap();
        assert!(ticket.publish(event(1, sequence, "reader"), permit));
        assert_eq!(mailbox.inner.lock().latest.len(), 1);
        assert_eq!(mailbox.inner.lock().ready.len(), 1);
        assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY - 1);
    }
    let latest = mailbox.take().unwrap();
    assert_eq!(latest.sequence, 63);
    drop(latest);
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
}

#[tokio::test]
async fn ready_bursts_schedule_one_wakeup_until_the_queue_is_consumed() {
    let mailbox = JobMailbox::new();
    let notifications = Arc::new(AtomicUsize::new(0));
    let notified = notifications.clone();
    mailbox.set_wakeup(Arc::new(move || { notified.fetch_add(1, Ordering::SeqCst); }));
    for sequence in 0..DELIVERY_CAPACITY as u64 {
        let key = format!("ready-{sequence}");
        let ticket = mailbox.begin(1, sequence, &key, false).unwrap();
        let permit = ticket.acquire_delivery().await.unwrap();
        assert!(ticket.publish(event(1, sequence, &key), permit));
    }
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
    for _ in 0..DELIVERY_CAPACITY { drop(mailbox.take().unwrap()); }
    assert!(mailbox.take().is_none());
    let ticket = mailbox.begin(1, 100, "next", false).unwrap();
    let permit = ticket.acquire_delivery().await.unwrap();
    assert!(ticket.publish(event(1, 100, "next"), permit));
    assert_eq!(notifications.load(Ordering::SeqCst), 2);
    drop(mailbox.take());
}

#[tokio::test]
async fn replacing_a_queued_result_reuses_its_pending_wakeup() {
    let mailbox = JobMailbox::new();
    let notifications = Arc::new(AtomicUsize::new(0));
    let notified = notifications.clone();
    mailbox.set_wakeup(Arc::new(move || { notified.fetch_add(1, Ordering::SeqCst); }));
    for sequence in 0..64 {
        let ticket = mailbox.begin(1, sequence, "reader", false).unwrap();
        let permit = ticket.acquire_delivery().await.unwrap();
        assert!(ticket.publish(event(1, sequence, "reader"), permit));
    }
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
    assert_eq!(mailbox.take().unwrap().sequence, 63);
    assert!(mailbox.take().is_none());
    let ticket = mailbox.begin(1, 64, "reader", false).unwrap();
    let permit = ticket.acquire_delivery().await.unwrap();
    assert!(ticket.publish(event(1, 64, "reader"), permit));
    assert_eq!(notifications.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn bounded_consumer_batches_rearm_results_without_waiting_for_a_timer() {
    let mailbox = JobMailbox::new();
    let notifications = Arc::new(AtomicUsize::new(0));
    let notified = notifications.clone();
    mailbox.set_wakeup(Arc::new(move || { notified.fetch_add(1, Ordering::SeqCst); }));
    for sequence in 0..2 {
        let key = format!("ready-{sequence}");
        let ticket = mailbox.begin(1, sequence, &key, false).unwrap();
        let permit = ticket.acquire_delivery().await.unwrap();
        assert!(ticket.publish(event(1, sequence, &key), permit));
    }
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
    drop(mailbox.take().unwrap());
    mailbox.wake_if_ready();
    assert_eq!(notifications.load(Ordering::SeqCst), 2);
    drop(mailbox.take().unwrap());
    mailbox.wake_if_ready();
    assert_eq!(notifications.load(Ordering::SeqCst), 2);
    let ticket = mailbox.begin(1, 2, "next", false).unwrap();
    let permit = ticket.acquire_delivery().await.unwrap();
    assert!(ticket.publish(event(1, 2, "next"), permit));
    assert_eq!(notifications.load(Ordering::SeqCst), 3);
    mailbox.close();
    mailbox.wake_if_ready();
    assert_eq!(notifications.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn installing_a_wakeup_delivers_an_already_queued_result_outside_the_lock() {
    let mailbox = JobMailbox::new();
    let ticket = mailbox.begin(1, 1, "reader", false).unwrap();
    let permit = ticket.acquire_delivery().await.unwrap();
    assert!(ticket.publish(event(1, 1, "reader"), permit));
    let notifications = Arc::new(AtomicUsize::new(0));
    let outside = Arc::new(AtomicBool::new(true));
    let notified = notifications.clone();
    let unlocked = outside.clone();
    let weak = Arc::downgrade(&mailbox.inner);
    mailbox.set_wakeup(Arc::new(move || {
        let inner = weak.upgrade().unwrap();
        let available = inner.state.try_lock().is_ok();
        unlocked.fetch_and(available, Ordering::SeqCst);
        if available {
            let mailbox = JobMailbox { inner };
            assert!(mailbox.is_current(1, 1, "reader"));
        }
        notified.fetch_add(1, Ordering::SeqCst);
    }));
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
    assert!(outside.load(Ordering::SeqCst));
    drop(mailbox.take());
}

#[tokio::test]
async fn publishing_notifies_outside_the_lock_and_stale_or_closed_work_stays_silent() {
    let mailbox = JobMailbox::new();
    let notifications = Arc::new(AtomicUsize::new(0));
    let outside = Arc::new(AtomicBool::new(true));
    let notified = notifications.clone();
    let unlocked = outside.clone();
    let weak = Arc::downgrade(&mailbox.inner);
    mailbox.set_wakeup(Arc::new(move || {
        let inner = weak.upgrade().unwrap();
        let available = inner.state.try_lock().is_ok();
        unlocked.fetch_and(available, Ordering::SeqCst);
        if available {
            let mailbox = JobMailbox { inner };
            assert!(mailbox.is_current(1, 2, "reader"));
        }
        notified.fetch_add(1, Ordering::SeqCst);
    }));
    let stale = mailbox.begin(1, 1, "reader", false).unwrap();
    let stale_permit = stale.acquire_delivery().await.unwrap();
    let current = mailbox.begin(1, 2, "reader", false).unwrap();
    assert!(!stale.publish(event(1, 1, "reader"), stale_permit));
    assert_eq!(notifications.load(Ordering::SeqCst), 0);
    let permit = current.acquire_delivery().await.unwrap();
    assert!(current.publish(event(1, 2, "reader"), permit));
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
    assert!(outside.load(Ordering::SeqCst));
    drop(mailbox.take());
    let closing = mailbox.begin(1, 3, "closing", false).unwrap();
    let permit = closing.acquire_delivery().await.unwrap();
    mailbox.close();
    assert!(!closing.publish(event(1, 3, "closing"), permit));
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
    assert!(mailbox.take().is_none());
}

#[tokio::test]
async fn cancellation_is_terminal_and_wakes_every_registered_waiter() {
    let mailbox = JobMailbox::new();
    let ticket = mailbox.begin(1, 1, "reader", false).unwrap();
    let counter = Arc::new(WakeCounter::default());
    let mut first = Box::pin(ticket.cancelled());
    let mut second = Box::pin(ticket.cancelled());
    assert!(poll_once(first.as_mut(), &counter).is_pending());
    assert!(poll_once(second.as_mut(), &counter).is_pending());
    mailbox.invalidate("reader");
    assert!(counter.0.load(Ordering::SeqCst) >= 2);
    assert!(poll_once(first.as_mut(), &counter).is_ready());
    assert!(poll_once(second.as_mut(), &counter).is_ready());
    ticket.cancelled().await;
    assert!(ticket.acquire_delivery().await.is_none());
}

#[tokio::test]
async fn cancellation_removes_an_ordinary_delivery_waiter_without_taking_a_slot() {
    let mailbox = JobMailbox::new();
    let blocker = mailbox.begin(1, 1, "blocker", false).unwrap();
    let mut permits = Vec::new();
    for _ in 0..DELIVERY_CAPACITY { permits.push(blocker.acquire_delivery().await.unwrap()); }
    let ticket = mailbox.begin(1, 2, "waiting", false).unwrap();
    let counter = Arc::new(WakeCounter::default());
    let mut waiting = Box::pin(ticket.acquire_delivery());
    assert!(poll_once(waiting.as_mut(), &counter).is_pending());
    mailbox.invalidate("waiting");
    assert!(matches!(poll_once(waiting.as_mut(), &counter), Poll::Ready(None)));
    assert_eq!(mailbox.inner.delivery.available_permits(), 0);
    drop(permits);
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
}

#[tokio::test]
async fn eight_delivery_slots_bound_work_and_queued_results_until_ui_consumption() {
    let mailbox = JobMailbox::new();
    for sequence in 0..DELIVERY_CAPACITY as u64 {
        let key = format!("ready-{sequence}");
        let ticket = mailbox.begin(1, sequence, &key, false).unwrap();
        let permit = ticket.acquire_delivery().await.unwrap();
        assert!(ticket.publish(event(1, sequence, &key), permit));
    }
    let ticket = mailbox.begin(1, 9, "waiting", false).unwrap();
    let counter = Arc::new(WakeCounter::default());
    let mut waiting = Box::pin(ticket.acquire_delivery());
    assert!(poll_once(waiting.as_mut(), &counter).is_pending());
    let consumed = mailbox.take().unwrap();
    assert!(poll_once(waiting.as_mut(), &counter).is_pending());
    drop(consumed);
    let permit = waiting.await.unwrap();
    assert_eq!(mailbox.inner.delivery.available_permits(), 0);
    drop(permit);
    mailbox.clear();
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
}

#[tokio::test]
async fn clear_does_not_release_a_durable_worker_or_its_execution_slot() {
    let mailbox = JobMailbox::new();
    let mut tickets = (0..MAX_DURABLE_IN_FLIGHT).map(|index|
        mailbox.begin(1, index as u64, &format!("purchase-{index}"), true).unwrap()
    ).collect::<Vec<_>>();
    let ticket = tickets.remove(0);
    let permit = ticket.acquire_delivery().await.unwrap();
    let (finish, finished) = tokio::sync::oneshot::channel::<()>();
    let worker = tokio::spawn(async move {
        let _ = finished.await;
        ticket.publish(event(1, 0, "purchase-0"), permit)
    });
    mailbox.clear();
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY - 1);
    assert!(matches!(mailbox.begin(2, 100, "purchase-new", true), Err(CapacityError::DurableInFlight)));
    finish.send(()).unwrap();
    assert!(!worker.await.unwrap());
    assert_eq!(mailbox.inner.delivery.available_permits(), DELIVERY_CAPACITY);
    assert!(mailbox.begin(2, 101, "purchase-new", true).is_ok());
    drop(tickets);
}

#[tokio::test]
async fn durable_delivery_waits_through_ui_clear_before_executing() {
    let mailbox = JobMailbox::new();
    let blocker = mailbox.begin(1, 1, "blocker", false).unwrap();
    let mut permits = Vec::new();
    for _ in 0..DELIVERY_CAPACITY { permits.push(blocker.acquire_delivery().await.unwrap()); }
    let durable = mailbox.begin(1, 2, "purchase", true).unwrap();
    let counter = Arc::new(WakeCounter::default());
    let mut waiting = Box::pin(durable.acquire_delivery());
    assert!(poll_once(waiting.as_mut(), &counter).is_pending());
    mailbox.clear();
    assert!(!durable.is_current());
    assert!(poll_once(waiting.as_mut(), &counter).is_pending());
    drop(permits.pop());
    let permit = waiting.await.unwrap();
    assert!(!durable.publish(event(1, 2, "purchase"), permit));
    drop(permits);
}

struct DropProbe {
    mailbox: JobMailbox,
    dropped: Arc<AtomicUsize>,
    outside_lock: Arc<AtomicBool>,
}

impl Drop for DropProbe {
    fn drop(&mut self) {
        let unlocked = self.mailbox.inner.state.try_lock().is_ok();
        self.outside_lock.fetch_and(unlocked, Ordering::SeqCst);
        if unlocked { self.mailbox.invalidate("reentrant-probe"); }
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

fn probed_event(mailbox: &JobMailbox, sequence: u64, dropped: &Arc<AtomicUsize>,
    outside_lock: &Arc<AtomicBool>) -> JobEvent
{
    let probe = DropProbe { mailbox: mailbox.clone(), dropped: dropped.clone(),
        outside_lock: outside_lock.clone() };
    JobEvent { epoch: 1, sequence, key: "reader".into(), apply: Box::new(move |_| drop(probe)) }
}

fn probed_wakeup(mailbox: &JobMailbox, dropped: &Arc<AtomicUsize>,
    outside_lock: &Arc<AtomicBool>) -> Arc<dyn Fn() + Send + Sync>
{
    let probe = DropProbe { mailbox: mailbox.clone(), dropped: dropped.clone(),
        outside_lock: outside_lock.clone() };
    Arc::new(move || { let _ = &probe; })
}

#[test]
fn replacing_closing_and_rejecting_wakeup_callbacks_release_them_outside_the_mutex() {
    let mailbox = JobMailbox::new();
    let dropped = Arc::new(AtomicUsize::new(0));
    let outside = Arc::new(AtomicBool::new(true));
    mailbox.set_wakeup(probed_wakeup(&mailbox, &dropped, &outside));
    mailbox.set_wakeup(probed_wakeup(&mailbox, &dropped, &outside));
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    mailbox.close();
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
    mailbox.set_wakeup(probed_wakeup(&mailbox, &dropped, &outside));
    assert_eq!(dropped.load(Ordering::SeqCst), 3);
    assert!(outside.load(Ordering::SeqCst));
    assert!(mailbox.inner.lock().wakeup.is_none());
}

#[tokio::test]
async fn replacement_clear_and_stale_publish_drop_payloads_outside_the_mutex() {
    let mailbox = JobMailbox::new();
    let dropped = Arc::new(AtomicUsize::new(0));
    let outside = Arc::new(AtomicBool::new(true));
    let previous = mailbox.begin(1, 1, "reader", false).unwrap();
    let permit = previous.acquire_delivery().await.unwrap();
    assert!(previous.publish(probed_event(&mailbox, 1, &dropped, &outside), permit));
    let current = mailbox.begin(1, 2, "reader", false).unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    let permit = current.acquire_delivery().await.unwrap();
    assert!(current.publish(probed_event(&mailbox, 2, &dropped, &outside), permit));
    mailbox.clear();
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
    let stale = mailbox.begin(1, 3, "reader", false).unwrap();
    let permit = stale.acquire_delivery().await.unwrap();
    mailbox.invalidate("reader");
    assert!(!stale.publish(probed_event(&mailbox, 3, &dropped, &outside), permit));
    assert_eq!(dropped.load(Ordering::SeqCst), 3);
    assert!(outside.load(Ordering::SeqCst));
}

#[tokio::test]
async fn shutdown_wakes_waiters_and_keeps_durable_leases_until_worker_drop() {
    let mailbox = JobMailbox::new();
    let durable = mailbox.begin(1, 1, "purchase", true).unwrap();
    let blocker = mailbox.begin(1, 2, "blocker", false).unwrap();
    let mut permits = Vec::new();
    for _ in 0..DELIVERY_CAPACITY { permits.push(blocker.acquire_delivery().await.unwrap()); }
    let counter = Arc::new(WakeCounter::default());
    let mut waiting = Box::pin(durable.acquire_delivery());
    assert!(poll_once(waiting.as_mut(), &counter).is_pending());
    mailbox.close();
    assert!(matches!(poll_once(waiting.as_mut(), &counter), Poll::Ready(None)));
    drop(waiting);
    assert_eq!(mailbox.inner.durable.available_permits(), MAX_DURABLE_IN_FLIGHT - 1);
    assert!(matches!(mailbox.begin(2, 3, "closed", false), Err(CapacityError::Closed)));
    drop(durable);
    assert_eq!(mailbox.inner.durable.available_permits(), MAX_DURABLE_IN_FLIGHT);
    drop(permits);
}

#[test]
fn poisoned_metadata_does_not_panic_or_reset_capacity_on_cleanup() {
    let mailbox = JobMailbox::new();
    let durable = mailbox.begin(1, 1, "purchase", true).unwrap();
    let inner = mailbox.inner.clone();
    let poisoned = std::thread::spawn(move || {
        let _state = inner.state.lock().unwrap();
        panic!("Poison the metadata lock without changing its state");
    });
    assert!(poisoned.join().is_err());
    mailbox.clear();
    assert!(!durable.is_current());
    assert_eq!(mailbox.inner.durable.available_permits(), MAX_DURABLE_IN_FLIGHT - 1);
    assert!(mailbox.begin(2, 2, "regular", false).is_ok());
    drop(durable);
    assert_eq!(mailbox.inner.durable.available_permits(), MAX_DURABLE_IN_FLIGHT);
}

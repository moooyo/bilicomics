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

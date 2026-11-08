//! Bounded, latest-per-key application work and result delivery.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, MutexGuard, Weak, atomic::{AtomicBool, Ordering}},
};

use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, TryAcquireError};

use super::App;

const MAX_REGULAR_KEYS: usize = 128;
const MAX_DURABLE_IN_FLIGHT: usize = 16;
const DELIVERY_CAPACITY: usize = 8;

pub(super) struct JobEvent {
    pub(super) epoch: u64,
    pub(super) sequence: u64,
    pub(super) key: String,
    pub(super) apply: Box<dyn FnOnce(&mut App) + Send>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(super) enum CapacityError {
    #[error("The application job registry has reached its regular-key capacity")]
    RegularKeys,
    #[error("The application has reached its durable in-flight job capacity")]
    DurableInFlight,
    #[error("The application job mailbox is closed")]
    Closed,
}

#[derive(Clone)]
pub(super) struct JobMailbox {
    inner: Arc<MailboxInner>,
}

struct MailboxInner {
    state: Mutex<MailboxState>,
    delivery: Arc<Semaphore>,
    durable: Arc<Semaphore>,
}

#[derive(Default)]
struct MailboxState {
    closed: bool,
    latest: HashMap<String, Arc<JobToken>>,
    ready: VecDeque<QueuedEvent>,
}

struct JobToken {
    epoch: u64,
    sequence: u64,
    key: String,
    durable: bool,
    invalidated: AtomicBool,
    cancelled: Notify,
}

struct QueuedEvent {
    token: Arc<JobToken>,
    event: JobEvent,
}

/// A worker owns this ticket until its actual future or blocking work finishes.
///
/// The ticket is deliberately not Clone. A durable lease belongs to the worker,
/// rather than the latest-key registry, so invalidation cannot free that lease.
pub(super) struct JobTicket {
    mailbox: Weak<MailboxInner>,
    token: Arc<JobToken>,
    _durable_lease: Option<OwnedSemaphorePermit>,
    published: bool,
}

impl Default for JobMailbox {
    fn default() -> Self { Self::new() }
}

impl JobMailbox {
    pub(super) fn new() -> Self {
        Self { inner: Arc::new(MailboxInner {
            state: Mutex::new(MailboxState {
                ready: VecDeque::with_capacity(DELIVERY_CAPACITY),
                ..Default::default()
            }),
            delivery: Arc::new(Semaphore::new(DELIVERY_CAPACITY)),
            durable: Arc::new(Semaphore::new(MAX_DURABLE_IN_FLIGHT)),
        }) }
    }

    /// Admit metadata without starting work or allocating a result payload.
    /// A failed admission leaves the previous generation current.
    pub(super) fn begin(&self, epoch: u64, sequence: u64, key: &str, durable: bool)
        -> Result<JobTicket, CapacityError>
    {
        let durable_lease = if durable {
            match self.inner.durable.clone().try_acquire_owned() {
                Ok(permit) => Some(permit),
                Err(TryAcquireError::NoPermits) => return Err(CapacityError::DurableInFlight),
                Err(TryAcquireError::Closed) => return Err(CapacityError::Closed),
            }
        } else { None };
        let token = Arc::new(JobToken {
            epoch, sequence, key: key.to_owned(), durable,
            invalidated: AtomicBool::new(false), cancelled: Notify::new(),
        });
        let (previous, discarded) = {
            let mut state = self.inner.lock();
            if state.closed { return Err(CapacityError::Closed); }
            let replaces_regular = state.latest.get(key).is_some_and(|token| !token.durable);
            if !durable && !replaces_regular
                && state.latest.values().filter(|token| !token.durable).count() >= MAX_REGULAR_KEYS
            {
                return Err(CapacityError::RegularKeys);
            }
            let previous = state.latest.insert(key.to_owned(), token.clone());
            let discarded = state.remove_ready(key);
            (previous, discarded)
        };
        if let Some(previous) = previous.as_ref() { previous.invalidate(); }
        drop(discarded);
        drop(previous);
        Ok(JobTicket { mailbox: Arc::downgrade(&self.inner), token,
            _durable_lease: durable_lease, published: false })
    }

    /// Retire the latest key before handing its result to the UI.
    /// The delivery slot stays inside the returned callback until apply or drop.
    pub(super) fn take(&self) -> Option<JobEvent> {
        loop {
            let (queued, current, retired) = {
                let mut state = self.inner.lock();
                let queued = state.ready.pop_front()?;
                let current = !state.closed && state.matches(&queued.token);
                let retired = if current { state.latest.remove(&queued.token.key) } else { None };
                (queued, current, retired)
            };
            let QueuedEvent { token, event } = queued;
            token.invalidate();
            drop(retired);
            if current { return Some(event); }
            drop(event);
        }
    }

    pub(super) fn invalidate(&self, key: &str) {
        let (previous, discarded) = {
            let mut state = self.inner.lock();
            (state.latest.remove(key), state.remove_ready(key))
        };
        if let Some(previous) = previous.as_ref() { previous.invalidate(); }
        drop(discarded);
        drop(previous);
    }

    pub(super) fn is_current(&self, epoch: u64, sequence: u64, key: &str) -> bool {
        let state = self.inner.lock();
        !state.closed && state.latest.get(key).is_some_and(|token|
            token.epoch == epoch && token.sequence == sequence
                && !token.invalidated.load(Ordering::Acquire))
    }

    /// Clear UI work without closing capacity or cancelling durable execution.
    pub(super) fn clear(&self) { self.inner.clear(false); }

    /// Stop new admission and wake delivery waiters during application shutdown.
    /// Already executing durable futures remain the responsibility of their owner.
    pub(super) fn close(&self) { self.inner.clear(true); }
}

impl MailboxInner {
    fn lock(&self) -> MutexGuard<'_, MailboxState> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn clear(&self, close: bool) {
        let (previous, discarded) = {
            let mut state = self.lock();
            state.closed |= close;
            (std::mem::take(&mut state.latest), std::mem::take(&mut state.ready))
        };
        if close {
            self.delivery.close();
            self.durable.close();
        }
        for token in previous.values() { token.invalidate(); }
        drop(discarded);
        drop(previous);
    }

    fn retire(&self, token: &Arc<JobToken>) {
        let (previous, discarded) = {
            let mut state = self.lock();
            if state.matches(token) {
                (state.latest.remove(&token.key), state.remove_ready(&token.key))
            } else { (None, None) }
        };
        token.invalidate();
        drop(discarded);
        drop(previous);
    }
}

impl Drop for MailboxInner {
    fn drop(&mut self) {
        self.delivery.close();
        self.durable.close();
        let state = self.state.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::mem::take(&mut state.latest);
        let discarded = std::mem::take(&mut state.ready);
        for token in previous.values() { token.invalidate(); }
        drop(discarded);
        drop(previous);
    }
}

impl MailboxState {
    fn matches(&self, token: &Arc<JobToken>) -> bool {
        !token.invalidated.load(Ordering::Acquire)
            && self.latest.get(&token.key).is_some_and(|current| Arc::ptr_eq(current, token))
    }

    fn remove_ready(&mut self, key: &str) -> Option<QueuedEvent> {
        let index = self.ready.iter().position(|queued| queued.token.key == key)?;
        self.ready.remove(index)
    }
}

impl JobToken {
    fn invalidate(&self) {
        if !self.invalidated.swap(true, Ordering::AcqRel) { self.cancelled.notify_waiters(); }
    }
}

impl JobTicket {
    pub(super) fn is_current(&self) -> bool {
        if self.token.invalidated.load(Ordering::Acquire) { return false; }
        let Some(mailbox) = self.mailbox.upgrade() else { return false; };
        let state = mailbox.lock();
        !state.closed && state.matches(&self.token)
    }

    pub(super) async fn cancelled(&self) {
        loop {
            let notified = self.token.cancelled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.is_current() { return; }
            notified.await;
        }
    }

    /// Regular work obtains a slot before execution. Protected financial work
    /// persists its outcome before waiting for UI delivery. Keep a permit with
    /// actual blocking work if its async waiter is cancelled.
    pub(super) async fn acquire_delivery(&self) -> Option<OwnedSemaphorePermit> {
        let delivery = self.mailbox.upgrade()?.delivery.clone();
        if self.token.durable {
            // UI invalidation must not cancel an admitted financial operation.
            return delivery.acquire_owned().await.ok();
        }
        if !self.is_current() { return None; }
        tokio::select! {
            biased;
            _ = self.cancelled() => None,
            permit = delivery.acquire_owned() => {
                let permit = permit.ok()?;
                self.is_current().then_some(permit)
            }
        }
    }

    /// Publish once, discarding stale or mismatched UI results outside the lock.
    /// Financial work must persist its observed result before calling this method.
    pub(super) fn publish(mut self, event: JobEvent, permit: OwnedSemaphorePermit) -> bool {
        let Some(mailbox) = self.mailbox.upgrade() else { return false; };
        if event.epoch != self.token.epoch || event.sequence != self.token.sequence
            || event.key != self.token.key || permit.num_permits() != 1
            || !Arc::ptr_eq(permit.semaphore(), &mailbox.delivery)
        {
            return false;
        }
        let JobEvent { epoch, sequence, key, apply } = event;
        let event = JobEvent { epoch, sequence, key, apply: Box::new(move |app| {
            let _delivery = permit;
            apply(app);
        }) };
        let mut queued = Some(QueuedEvent { token: self.token.clone(), event });
        let (published, discarded) = {
            let mut state = mailbox.lock();
            if !state.closed && state.matches(&self.token) {
                let discarded = state.remove_ready(&self.token.key);
                if let Some(ready) = queued.take() { state.ready.push_back(ready); }
                (true, discarded)
            } else { (false, None) }
        };
        self.published = published;
        drop(discarded);
        drop(queued);
        published
    }
}

impl Drop for JobTicket {
    fn drop(&mut self) {
        if self.published { return; }
        if let Some(mailbox) = self.mailbox.upgrade() { mailbox.retire(&self.token); }
        else { self.token.invalidate(); }
    }
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;

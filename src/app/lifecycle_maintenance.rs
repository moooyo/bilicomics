//! Pending-only sleep maintenance delivered through the event-loop control queue.
//!
//! The UI registry contains only weak references and is never accessed by a
//! runtime worker. The worker sends a scope identifier; the actual control
//! closure resolves that identifier after it reaches the UI thread.

use std::{
    cell::RefCell,
    collections::BTreeMap,
    future::{Future, pending},
    marker::PhantomData,
    pin::Pin,
    rc::{Rc, Weak},
    sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}},
    time::Instant,
};

use anyhow::{Context, Result};
use tokio::sync::{Notify, Semaphore, oneshot, watch};

use super::App;

type ControlCallback = Box<dyn FnOnce() + Send + 'static>;
type Dispatcher = Arc<dyn Fn(ControlCallback) -> bool + Send + Sync>;
type DispatchFuture<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

thread_local! {
    static APPLICATIONS: RefCell<BTreeMap<u64, Weak<RefCell<App>>>> = const { RefCell::new(BTreeMap::new()) };
}

#[derive(Clone, Debug)]
pub(super) struct WakeToken {
    pub(super) epoch: u64,
    pub(super) cycle: u64,
    pub(super) revision: u64,
    pub(super) cancelled: Arc<AtomicBool>,
}

impl WakeToken {
    pub(super) fn is_cancelled(&self) -> bool { self.cancelled.load(Ordering::Acquire) }
    fn identity(&self) -> (u64, u64, u64) { (self.epoch, self.cycle, self.revision) }
}

#[derive(Clone, Debug)]
pub(super) struct WakePlan {
    pub(super) token: WakeToken,
    pub(super) deadline: Instant,
}

/// Sendable access to a UI-owned application through one bounded control slot.
#[derive(Clone)]
pub(super) struct RemoteApp {
    scope: u64,
    alive: Arc<AtomicBool>,
    callbacks: Arc<Semaphore>,
    dispatch: Dispatcher,
}

impl RemoteApp {
    pub(super) fn is_closed(&self) -> bool { !self.alive.load(Ordering::Acquire) }

    /// Resolve and validate the application only after reaching its UI thread.
    /// The delivery slot stays in that real closure even if its waiter is dropped.
    pub(super) async fn call<T, F>(&self, token: &WakeToken, apply: F) -> Option<T>
    where T: Send + 'static, F: FnOnce(&mut App) -> T + Send + 'static,
    {
        if token.is_cancelled() { return None; }
        let scope = self.scope;
        let token = token.clone();
        self.post(move || {
            if token.is_cancelled() { return None; }
            let application = resolve_application(scope)?;
            let mut application = application.borrow_mut();
            application.sleep_maintenance_matches(scope, &token).then(|| apply(&mut application))
        }).await
    }

    async fn receipts(&self) -> bool {
        let scope = self.scope;
        self.post(move || {
            let application = resolve_application(scope)?;
            let _ = application.borrow_mut().sleep_receipt_ready(scope);
            Some(())
        }).await.is_some()
    }

    async fn post<T, F>(&self, apply: F) -> Option<T>
    where T: Send + 'static, F: FnOnce() -> Option<T> + Send + 'static,
    {
        if self.is_closed() { return None; }
        let delivery = self.callbacks.clone().acquire_owned().await.ok()?;
        if self.is_closed() { return None; }
        let (sender, receiver) = oneshot::channel();
        let alive = self.alive.clone();
        let callback = Box::new(move || {
            let _delivery = delivery;
            let result = if alive.load(Ordering::Acquire) { apply() } else { None };
            let _ = sender.send(result);
        });
        if !(self.dispatch)(callback) {
            self.alive.store(false, Ordering::Release);
            self.callbacks.close();
            return None;
        }
        receiver.await.ok().flatten()
    }
}

fn resolve_application(scope: u64) -> Option<Rc<RefCell<App>>> {
    // Release the registry borrow before any application code or destructor runs.
    APPLICATIONS.with(|applications| applications.borrow().get(&scope).and_then(Weak::upgrade))
}

/// The driver is installed and destroyed on the UI thread that owns App.
pub(super) struct MaintenanceDriver {
    remote: RemoteApp,
    plans: watch::Sender<Option<WakePlan>>,
    signal: Arc<Notify>,
    guards: RefCell<Vec<std::sync::Weak<AtomicBool>>>,
    task: tokio::task::JoinHandle<()>,
    _ui_thread: PhantomData<Rc<()>>,
}

impl MaintenanceDriver {
    pub(super) fn install(application: &Rc<RefCell<App>>) -> Result<Self> {
        static NEXT_SCOPE: AtomicU64 = AtomicU64::new(1);
        let scope = allocate_scope(&NEXT_SCOPE)?;
        let handle = application.borrow().handle.clone();
        APPLICATIONS.with(|applications| { applications.borrow_mut().insert(scope, Rc::downgrade(application)); });
        let remote = RemoteApp {
            scope, alive: Arc::new(AtomicBool::new(true)), callbacks: Arc::new(Semaphore::new(1)),
            dispatch: Arc::new(|callback| slint::invoke_from_event_loop(callback).is_ok()),
        };
        let (plans, receiver) = watch::channel(None);
        let signal = Arc::new(Notify::new());
        let task = handle.spawn(run_scheduler(receiver, signal.clone(), remote.clone()));
        Ok(Self { remote, plans, signal, guards: RefCell::new(Vec::new()), task, _ui_thread: PhantomData })
    }

    pub(super) fn scope(&self) -> u64 { self.remote.scope }
    pub(super) fn signal(&self) -> Arc<Notify> { self.signal.clone() }

    /// A new revision is required to run the same deadline again. Setting None
    /// only disarms its timer; it does not cancel a worker using the same token.
    pub(super) fn set_plan(&self, plan: Option<WakePlan>) {
        if let Some(plan) = &plan {
            let mut guards = self.guards.borrow_mut();
            guards.retain(|guard| guard.strong_count() > 0);
            let guard = Arc::downgrade(&plan.token.cancelled);
            if !guards.iter().any(|previous| previous.ptr_eq(&guard)) { guards.push(guard); }
        }
        let _ = self.plans.send_replace(plan);
    }
}

impl Drop for MaintenanceDriver {
    fn drop(&mut self) {
        self.remote.alive.store(false, Ordering::Release);
        for guard in self.guards.get_mut().drain(..).filter_map(|guard| guard.upgrade()) {
            guard.store(true, Ordering::Release);
        }
        self.remote.callbacks.close();
        self.task.abort();
        let _ = APPLICATIONS.try_with(|applications| { applications.borrow_mut().remove(&self.remote.scope); });
    }
}

fn allocate_scope(next: &AtomicU64) -> Result<u64> {
    let mut scope = next.load(Ordering::Acquire);
    loop {
        let following = scope.checked_add(1).context("Local maintenance scope exhausted")?;
        match next.compare_exchange_weak(scope, following, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Ok(scope),
            Err(current) => scope = current,
        }
    }
}

trait SchedulerDispatch: Clone + Send + Sync + 'static {
    fn is_closed(&self) -> bool;
    fn receipts(&self) -> DispatchFuture<'_>;
    fn maintenance(&self, plan: WakePlan) -> DispatchFuture<'_>;
}

impl SchedulerDispatch for RemoteApp {
    fn is_closed(&self) -> bool { RemoteApp::is_closed(self) }
    fn receipts(&self) -> DispatchFuture<'_> { Box::pin(self.receipts()) }
    fn maintenance(&self, plan: WakePlan) -> DispatchFuture<'_> {
        Box::pin(async move {
            super::lifecycle::run_sleep_maintenance(self.clone(), plan).await;
            !self.is_closed()
        })
    }
}

async fn run_scheduler<D: SchedulerDispatch>(mut plans: watch::Receiver<Option<WakePlan>>,
    receipts: Arc<Notify>, dispatch: D) {
    let mut fired = None;
    loop {
        if dispatch.is_closed() { return; }
        let plan = { plans.borrow_and_update().clone() };
        let due = plan.filter(|plan| !plan.token.is_cancelled() && Some(plan.token.identity()) != fired);
        let deadline = due.as_ref().map(|plan| plan.deadline);
        tokio::select! {
            biased;
            changed = plans.changed() => { if changed.is_err() { return; } }
            _ = receipts.notified() => {
                // A notification may wake the registered waiter and leave one
                // additional stored permit. Consume that existing burst before
                // the UI captures the outbox; later arrivals remain observable.
                {
                    let extra = receipts.notified();
                    tokio::pin!(extra);
                    std::future::poll_fn(|context| {
                        let _ = extra.as_mut().poll(context);
                        std::task::Poll::Ready(())
                    }).await;
                }
                if !dispatch.receipts().await { return; }
            }
            _ = wait_deadline(deadline) => {
                let Some(plan) = due else { continue; };
                fired = Some(plan.token.identity());
                if plan.token.is_cancelled() { continue; }
                if !dispatch.maintenance(plan).await { return; }
            }
        }
    }
}

async fn wait_deadline(deadline: Option<Instant>) {
    if let Some(deadline) = deadline { tokio::time::sleep_until(deadline.into()).await; }
    else { pending::<()>().await; }
}

#[cfg(test)]
#[path = "lifecycle_maintenance_tests.rs"]
mod tests;

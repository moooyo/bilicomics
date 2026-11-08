use super::*;

use std::{collections::VecDeque, sync::{Mutex, atomic::AtomicUsize}, time::Duration};
use tokio::sync::mpsc;

fn token(revision: u64) -> WakeToken {
    WakeToken { epoch: 3, cycle: 5, revision, cancelled: Arc::new(AtomicBool::new(false)) }
}

fn plan(revision: u64) -> WakePlan { WakePlan { token: token(revision), deadline: Instant::now() } }

#[derive(Clone, Debug, Eq, PartialEq)]
enum Observation { Receipts, Maintenance(u64) }

#[derive(Clone)]
struct DispatchFixture {
    observations: mpsc::UnboundedSender<Observation>,
    block_first: Arc<Notify>,
    blocked: bool,
    active: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
}

impl SchedulerDispatch for DispatchFixture {
    fn is_closed(&self) -> bool { false }
    fn receipts(&self) -> DispatchFuture<'_> {
        Box::pin(async move { self.observations.send(Observation::Receipts).is_ok() })
    }
    fn maintenance(&self, plan: WakePlan) -> DispatchFuture<'_> {
        Box::pin(async move {
            let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
            self.maximum.fetch_max(active, Ordering::AcqRel);
            let sent = self.observations.send(Observation::Maintenance(plan.token.revision)).is_ok();
            if self.blocked && plan.token.revision == 1 { self.block_first.notified().await; }
            self.active.fetch_sub(1, Ordering::AcqRel);
            sent
        })
    }
}

fn fixture(blocked: bool) -> (DispatchFixture, mpsc::UnboundedReceiver<Observation>) {
    let (observations, receiver) = mpsc::unbounded_channel();
    (DispatchFixture { observations, blocked, block_first: Arc::new(Notify::new()),
        active: Arc::new(AtomicUsize::new(0)), maximum: Arc::new(AtomicUsize::new(0)) }, receiver)
}

async fn observe(receiver: &mut mpsc::UnboundedReceiver<Observation>) -> Observation {
    tokio::time::timeout(Duration::from_secs(2), receiver.recv()).await.unwrap().unwrap()
}

#[test]
fn driver_scope_allocation_fails_instead_of_wrapping_or_reusing() {
    let next = AtomicU64::new(u64::MAX - 1);
    assert_eq!(allocate_scope(&next).unwrap(), u64::MAX - 1);
    assert!(allocate_scope(&next).is_err());
    assert_eq!(next.load(Ordering::Acquire), u64::MAX);
}

#[tokio::test]
async fn an_idle_scheduler_has_no_timer_and_receipt_signals_coalesce() {
    let (dispatch, mut observations) = fixture(false);
    let (sender, receiver) = watch::channel(None);
    let receipts = Arc::new(Notify::new());
    let task = tokio::spawn(run_scheduler(receiver, receipts.clone(), dispatch));
    assert!(tokio::time::timeout(Duration::from_millis(30), observations.recv()).await.is_err());
    for _ in 0..64 { receipts.notify_one(); }
    assert_eq!(observe(&mut observations).await, Observation::Receipts);
    assert!(tokio::time::timeout(Duration::from_millis(30), observations.recv()).await.is_err());
    drop(sender);
    tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
}

#[tokio::test]
async fn a_due_revision_runs_once_and_a_new_revision_can_reschedule() {
    let (dispatch, mut observations) = fixture(false);
    let first = plan(1);
    let (sender, receiver) = watch::channel(Some(first.clone()));
    let task = tokio::spawn(run_scheduler(receiver, Arc::new(Notify::new()), dispatch));
    assert_eq!(observe(&mut observations).await, Observation::Maintenance(1));
    let _ = sender.send_replace(Some(first));
    assert!(tokio::time::timeout(Duration::from_millis(30), observations.recv()).await.is_err());
    let _ = sender.send_replace(Some(plan(2)));
    assert_eq!(observe(&mut observations).await, Observation::Maintenance(2));
    drop(sender);
    tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
}

#[tokio::test]
async fn cancelled_or_disarmed_plans_do_not_run_checkpoint_work() {
    let (dispatch, mut observations) = fixture(false);
    let cancelled = plan(1);
    cancelled.token.cancelled.store(true, Ordering::Release);
    let (sender, receiver) = watch::channel(Some(cancelled));
    let task = tokio::spawn(run_scheduler(receiver, Arc::new(Notify::new()), dispatch));
    assert!(tokio::time::timeout(Duration::from_millis(30), observations.recv()).await.is_err());
    let future = WakePlan { token: token(2), deadline: Instant::now() + Duration::from_secs(60) };
    let _ = sender.send_replace(Some(future));
    tokio::task::yield_now().await;
    let _ = sender.send_replace(None);
    assert!(tokio::time::timeout(Duration::from_millis(30), observations.recv()).await.is_err());
    let _ = sender.send_replace(Some(plan(3)));
    assert_eq!(observe(&mut observations).await, Observation::Maintenance(3));
    drop(sender);
    tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
}

#[tokio::test]
async fn a_changed_plan_waits_for_the_previous_checkpoint_to_finish() {
    let (dispatch, mut observations) = fixture(true);
    let release = dispatch.block_first.clone();
    let maximum = dispatch.maximum.clone();
    let (sender, receiver) = watch::channel(Some(plan(1)));
    let task = tokio::spawn(run_scheduler(receiver, Arc::new(Notify::new()), dispatch));
    assert_eq!(observe(&mut observations).await, Observation::Maintenance(1));
    let _ = sender.send_replace(Some(plan(2)));
    assert!(tokio::time::timeout(Duration::from_millis(30), observations.recv()).await.is_err());
    assert_eq!(maximum.load(Ordering::Acquire), 1);
    release.notify_one();
    assert_eq!(observe(&mut observations).await, Observation::Maintenance(2));
    assert_eq!(maximum.load(Ordering::Acquire), 1);
    drop(sender);
    tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
}

#[derive(Default)]
struct ControlFixture {
    queue: Mutex<VecDeque<ControlCallback>>,
    queued: Notify,
}

impl ControlFixture {
    fn remote(self: &Arc<Self>) -> RemoteApp {
        let fixture = self.clone();
        RemoteApp { scope: 42, alive: Arc::new(AtomicBool::new(true)), callbacks: Arc::new(Semaphore::new(1)),
            dispatch: Arc::new(move |callback| {
                fixture.queue.lock().unwrap().push_back(callback);
                fixture.queued.notify_one();
                true
            }) }
    }

    async fn wait_for_callback(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let notified = self.queued.notified();
                if !self.queue.lock().unwrap().is_empty() { return; }
                notified.await;
            }
        }).await.unwrap();
    }

    fn pop(&self) -> ControlCallback { self.queue.lock().unwrap().pop_front().unwrap() }
}

#[tokio::test]
async fn cancellation_is_checked_before_posting_an_application_callback() {
    let fixture = Arc::new(ControlFixture::default());
    let remote = fixture.remote();
    let cancelled = token(1);
    cancelled.cancelled.store(true, Ordering::Release);
    let result: Option<()> = remote.call(&cancelled, |_| panic!("An invalidated UI action must not run")).await;
    assert!(result.is_none());
    assert!(fixture.queue.lock().unwrap().is_empty());
}

#[tokio::test]
async fn dropping_a_callback_waiter_keeps_the_single_slot_with_the_real_callback() {
    let fixture = Arc::new(ControlFixture::default());
    let remote = fixture.remote();
    let first = tokio::spawn({ let remote = remote.clone(); async move { remote.post(|| Some(1)).await } });
    fixture.wait_for_callback().await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert_eq!(remote.callbacks.available_permits(), 0);
    let second = tokio::spawn({ let remote = remote.clone(); async move { remote.post(|| Some(2)).await } });
    tokio::task::yield_now().await;
    assert_eq!(fixture.queue.lock().unwrap().len(), 1);
    drop(fixture.pop());
    fixture.wait_for_callback().await;
    let callback = fixture.pop();
    callback();
    assert_eq!(second.await.unwrap(), Some(2));
    assert_eq!(remote.callbacks.available_permits(), 1);
}

#[tokio::test]
async fn shutdown_rejects_waiters_and_drops_queued_actions_without_applying_them() {
    let fixture = Arc::new(ControlFixture::default());
    let remote = fixture.remote();
    let applied = Arc::new(AtomicUsize::new(0));
    let first = tokio::spawn({ let remote = remote.clone(); let applied = applied.clone(); async move {
        remote.post(move || { applied.fetch_add(1, Ordering::AcqRel); Some(1) }).await
    }});
    fixture.wait_for_callback().await;
    let second = tokio::spawn({ let remote = remote.clone(); async move { remote.post(|| Some(2)).await } });
    tokio::task::yield_now().await;
    remote.alive.store(false, Ordering::Release);
    remote.callbacks.close();
    assert_eq!(second.await.unwrap(), None);
    let callback = fixture.pop();
    callback();
    assert_eq!(first.await.unwrap(), None);
    assert_eq!(applied.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn rejected_control_dispatch_closes_the_bridge() {
    let remote = RemoteApp { scope: 1, alive: Arc::new(AtomicBool::new(true)),
        callbacks: Arc::new(Semaphore::new(1)), dispatch: Arc::new(|callback| { drop(callback); false }) };
    assert_eq!(remote.post(|| Some(1)).await, None);
    assert!(remote.is_closed());
    assert!(remote.callbacks.is_closed());
    assert_eq!(remote.post(|| Some(2)).await, None);
}

fn driver_fixture(remote: RemoteApp) -> MaintenanceDriver {
    let (plans, _) = watch::channel(None);
    MaintenanceDriver { remote, plans, signal: Arc::new(Notify::new()), guards: RefCell::new(Vec::new()),
        task: tokio::spawn(pending::<()>()), _ui_thread: PhantomData }
}

#[tokio::test]
async fn dropping_a_driver_cancels_an_active_guard_even_after_disarming_its_timer() {
    let fixture = Arc::new(ControlFixture::default());
    let remote = fixture.remote();
    let driver = driver_fixture(remote.clone());
    let active = plan(1);
    driver.set_plan(Some(active.clone()));
    driver.set_plan(None);
    assert!(!active.token.is_cancelled());
    drop(driver);
    assert!(active.token.is_cancelled());
    assert!(remote.is_closed());
    assert!(remote.callbacks.is_closed());
}

#[tokio::test]
async fn completed_sleep_cycles_do_not_accumulate_guard_history() {
    let fixture = Arc::new(ControlFixture::default());
    let driver = driver_fixture(fixture.remote());
    let still_running = plan(1);
    driver.set_plan(Some(still_running.clone()));
    driver.set_plan(None);
    for revision in 2..10_002 {
        driver.set_plan(Some(plan(revision)));
        driver.set_plan(None);
        assert!(driver.guards.borrow().len() <= 2);
    }
    drop(driver);
    assert!(still_running.token.is_cancelled());
}

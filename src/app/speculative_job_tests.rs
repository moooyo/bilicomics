//! Headless prediction scheduling tests using explicit worker and mailbox gates.

use std::{
    future::{Future, poll_fn},
    sync::{Condvar, Mutex, atomic::AtomicUsize},
    task::Poll,
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};

use super::*;

const PREPARE_KEY: &str = "reader-prepare-next";
const TEST_DEADLINE: Duration = Duration::from_secs(8);

struct WorkerGate {
    entered: Semaphore,
    released: Mutex<bool>,
    changed: Condvar,
}

impl WorkerGate {
    fn new() -> Arc<Self> {
        Arc::new(Self { entered: Semaphore::new(0), released: Mutex::new(false), changed: Condvar::new() })
    }

    fn wait(&self) {
        self.entered.add_permits(1);
        let mut released = self.released.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*released {
            released = self.changed.wait(released).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    fn release(&self) {
        *self.released.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.changed.notify_all();
    }
}

struct ReleaseOnDrop(Arc<WorkerGate>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) { self.0.release(); }
}

struct CallbackDropped(Option<oneshot::Sender<()>>);

impl Drop for CallbackDropped {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() { let _ = sender.send(()); }
    }
}

#[derive(Debug)]
struct Observation {
    value: Option<u8>,
    failed: bool,
    cancelled: bool,
    pending_during_callback: bool,
}

fn observe(app: &App, result: Result<u8>, cancelled: &AtomicBool,
    observations: &Mutex<Vec<Observation>>) {
    observations.lock().unwrap().push(Observation {
        value: result.as_ref().ok().copied(), failed: result.is_err(),
        cancelled: cancelled.load(Ordering::Acquire),
        pending_during_callback: app.pending.contains_key(PREPARE_KEY),
    });
}

fn fixture(directory: &Path) -> Result<App> {
    test_support::app(directory, DeviceProfile::Paperwhite5,
        BiliClient::offline(directory)?, tokio::runtime::Handle::current())
}

fn publication_gate(app: &App) -> Arc<Semaphore> {
    let published = Arc::new(Semaphore::new(0));
    let notification = published.clone();
    app.jobs.set_wakeup(Arc::new(move || notification.add_permits(1)));
    published
}

async fn apply_until(app: &mut App, published: &Semaphore,
    mut finished: impl FnMut(&App) -> bool) -> Result<()>
{
    tokio::time::timeout(TEST_DEADLINE, async {
        while !finished(app) {
            published.acquire().await.context("The publication gate was closed")?.forget();
            app.tick()?;
        }
        Ok::<(), anyhow::Error>(())
    }).await.context("The application did not apply the expected callback")?
}

async fn settle_initial_maintenance(app: &mut App, published: &Semaphore) -> Result<()> {
    // Keep the first automatic cover-maintenance job outside the explicit
    // delivery-capacity fixtures used by the blocked-worker tests.
    app.tick()?;
    apply_until(app, published, |app| !app.pending.contains_key("cover-maintenance")).await
}

async fn hold_delivery(app: &App, count: usize)
    -> Result<(Vec<jobs::JobTicket>, Vec<OwnedSemaphorePermit>)>
{
    let mut tickets = Vec::new();
    let mut delivery = Vec::new();
    for index in 0..count {
        let ticket = app.jobs.begin(app.epoch, 100 + index as u64,
            &format!("speculative-held-{index}"), false)?;
        delivery.push(ticket.acquire_delivery().await.context("The fixture could not reserve delivery")?);
        tickets.push(ticket);
    }
    Ok((tickets, delivery))
}

struct RunningPrediction {
    cancelled: Arc<AtomicBool>,
    callback_dropped: oneshot::Receiver<()>,
}

async fn start_blocked(app: &mut App, published: &Semaphore, gate: &Arc<WorkerGate>,
    observations: &Arc<Mutex<Vec<Observation>>>) -> Result<RunningPrediction>
{
    tokio::time::timeout(TEST_DEADLINE, async {
        loop {
            let cancelled = Arc::new(AtomicBool::new(false));
            let callback_cancelled = cancelled.clone();
            let callback_observations = observations.clone();
            let worker_gate = gate.clone();
            let (dropped, callback_dropped) = oneshot::channel();
            let callback_drop = CallbackDropped(Some(dropped));
            let before = observations.lock().unwrap().len();
            if !app.background_speculative(PREPARE_KEY, None, cancelled.clone(), move || {
                worker_gate.wait();
                Ok(7_u8)
            }, move |app, result| {
                observe(app, result, &callback_cancelled, &callback_observations);
                drop(callback_drop);
            }) {
                app.tick()?;
                tokio::task::yield_now().await;
                continue;
            }
            loop {
                let entered = tokio::select! {
                    permit = gate.entered.acquire() => {
                        permit.context("The worker gate was closed")?.forget();
                        true
                    },
                    permit = published.acquire() => {
                        permit.context("The publication gate was closed")?.forget();
                        false
                    },
                };
                if entered { return Ok(RunningPrediction { cancelled, callback_dropped }); }
                // Tick can also consume the fixture's cover-maintenance result.
                app.tick()?;
                let observations = observations.lock().unwrap();
                if observations.len() == before { continue; }
                // Unrelated process-wide foreground tests may cancel a prediction
                // before its closure starts. Consume that attempt before retrying.
                let observed = observations.last().unwrap();
                anyhow::ensure!(observations.len() == before + 1 && observed.cancelled && observed.failed,
                    "A prediction finished without entering its worker gate or being cancelled");
                anyhow::ensure!(!app.pending.contains_key(PREPARE_KEY),
                    "An early cancelled prediction retained its pending generation");
                break;
            }
        }
    }).await.context("The blocked prediction could not obtain an idle process budget")?
}

#[derive(Clone, Copy)]
enum Outcome { Success, Error, Panic }

async fn complete_case(app: &mut App, published: &Semaphore, outcome: Outcome,
    observations: &Arc<Mutex<Vec<Observation>>>) -> Result<()>
{
    tokio::time::timeout(TEST_DEADLINE, async {
        loop {
            let cancelled = Arc::new(AtomicBool::new(false));
            let callback_cancelled = cancelled.clone();
            let callback_observations = observations.clone();
            let before = observations.lock().unwrap().len();
            if !app.background_speculative(PREPARE_KEY, None, cancelled, move || match outcome {
                Outcome::Success => Ok(7_u8),
                Outcome::Error => anyhow::bail!("Controlled speculative application failure"),
                Outcome::Panic => panic!("Controlled speculative application panic"),
            }, move |app, result| {
                observe(app, result, &callback_cancelled, &callback_observations);
            }) {
                app.tick()?;
                tokio::task::yield_now().await;
                continue;
            }
            apply_until(app, published, |_| observations.lock().unwrap().len() > before).await?;
            let observed = observations.lock().unwrap();
            anyhow::ensure!(observed.len() == before + 1, "A prediction did not deliver exactly one callback");
            let result = observed.last().unwrap();
            anyhow::ensure!(!result.pending_during_callback && !app.pending.contains_key(PREPARE_KEY),
                "A current prediction callback retained its pending generation");
            if result.cancelled { continue; }
            match outcome {
                Outcome::Success => anyhow::ensure!(result.value == Some(7) && !result.failed,
                    "A successful prediction lost its result"),
                Outcome::Error | Outcome::Panic => anyhow::ensure!(result.failed && result.value.is_none(),
                    "A failed prediction did not deliver an error"),
            }
            return Ok(());
        }
    }).await.context("The prediction could not complete without unrelated foreground cancellation")?
}

#[tokio::test]
async fn full_ordinary_capacity_keeps_one_independent_prediction_lane() -> Result<()> {
    let _serial = source::work::process_test_lock().lock().await;
    let directory = tempfile::tempdir()?;
    let mut app = fixture(directory.path())?;
    let (mut held_tickets, held_delivery) = hold_delivery(&app, 8).await?;
    for index in 8..128 {
        held_tickets.push(app.jobs.begin(app.epoch, 100 + index as u64,
            &format!("speculative-held-{index}"), false)?);
    }
    assert!(matches!(app.jobs.begin(app.epoch, 300, "speculative-normal-overflow", false),
        Err(jobs::CapacityError::RegularKeys)));
    let (held_prediction, held_prediction_delivery) = app.jobs
        .try_begin_with_delivery(app.epoch, 400, PREPARE_KEY)?
        .context("Ordinary capacity blocked the independent prediction lane")?;
    app.jobs.invalidate(PREPARE_KEY);
    let sequence = app.sequence;
    let workers = Arc::new(AtomicUsize::new(0));
    let callbacks = Arc::new(AtomicUsize::new(0));
    for _ in 0..4 {
        let workers = workers.clone();
        let callbacks = callbacks.clone();
        assert!(!app.background_speculative(PREPARE_KEY, None, Arc::new(AtomicBool::new(false)),
            move || { workers.fetch_add(1, Ordering::SeqCst); Ok(()) },
            move |_, _| { callbacks.fetch_add(1, Ordering::SeqCst); }));
        assert_eq!(app.sequence, sequence);
        assert!(app.pending.is_empty());
        assert!(!app.jobs.is_current(app.epoch, sequence + 1, PREPARE_KEY));
        assert!(held_tickets.iter().all(jobs::JobTicket::is_current));
    }
    assert_eq!(workers.load(Ordering::SeqCst), 0);
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);
    assert!(app.jobs.take().is_none());
    drop(held_prediction_delivery);
    drop(held_prediction);
    let published = publication_gate(&app);
    let observations = Arc::new(Mutex::new(Vec::new()));
    complete_case(&mut app, &published, Outcome::Success, &observations).await?;
    assert!(held_tickets.iter().all(jobs::JobTicket::is_current));
    assert!(matches!(app.jobs.begin(app.epoch, 301, "speculative-normal-overflow", false),
        Err(jobs::CapacityError::RegularKeys)));
    drop(held_delivery);
    drop(held_tickets);
    Ok(())
}

#[tokio::test]
async fn an_existing_prediction_generation_is_never_replaced_on_busy_admission() -> Result<()> {
    let _serial = source::work::process_test_lock().lock().await;
    let directory = tempfile::tempdir()?;
    let mut app = fixture(directory.path())?;
    let (epoch, sequence, ticket) = app.begin_job(PREPARE_KEY, false)?;
    for _ in 0..4 {
        assert!(!app.background_speculative(PREPARE_KEY, None, Arc::new(AtomicBool::new(false)),
            || Ok(()), |_, _| panic!("A rejected prediction invoked its callback")));
        assert_eq!(app.sequence, sequence);
        assert_eq!(app.pending.get(PREPARE_KEY), Some(&sequence));
        assert!(app.jobs.is_current(epoch, sequence, PREPARE_KEY));
        assert!(ticket.is_current());
    }
    app.invalidate(PREPARE_KEY);
    drop(ticket);
    Ok(())
}

#[tokio::test]
async fn network_waiting_does_not_prevent_an_idle_prediction_from_completing() -> Result<()> {
    let _serial = source::work::process_test_lock().lock().await;
    let directory = tempfile::tempdir()?;
    let mut app = fixture(directory.path())?;
    let published = publication_gate(&app);
    let (started, did_start) = oneshot::channel();
    let (_release_network, network_released) = oneshot::channel::<()>();
    app.source_job("speculative-network-wait", async move {
        let _ = started.send(());
        let _ = network_released.await;
        Ok(())
    }, |_, _| panic!("The synthetic network waiter completed before invalidation"));
    tokio::time::timeout(TEST_DEADLINE, did_start).await
        .context("The synthetic network waiter did not start")??;
    let network_sequence = app.pending["speculative-network-wait"];
    let observations = Arc::new(Mutex::new(Vec::new()));
    complete_case(&mut app, &published, Outcome::Success, &observations).await?;
    assert_eq!(app.pending.get("speculative-network-wait"), Some(&network_sequence));
    assert!(app.jobs.is_current(app.epoch, network_sequence, "speculative-network-wait"));
    app.invalidate("speculative-network-wait");
    Ok(())
}

#[tokio::test]
async fn successful_failed_and_panicking_predictions_each_apply_and_retire_their_pending_key() -> Result<()> {
    let _serial = source::work::process_test_lock().lock().await;
    for outcome in [Outcome::Success, Outcome::Error, Outcome::Panic] {
        let directory = tempfile::tempdir()?;
        let mut app = fixture(directory.path())?;
        let published = publication_gate(&app);
        let observations = Arc::new(Mutex::new(Vec::new()));
        complete_case(&mut app, &published, outcome, &observations).await?;
        let calls = observations.lock().unwrap().len();
        app.tick()?;
        assert_eq!(observations.lock().unwrap().len(), calls);
        assert!(!app.pending.contains_key(PREPARE_KEY));
    }
    Ok(())
}

#[tokio::test]
async fn a_foreground_worker_keeps_the_eighth_delivery_slot_while_cancelling_prediction() -> Result<()> {
    let _serial = source::work::process_test_lock().lock().await;
    let directory = tempfile::tempdir()?;
    let mut app = fixture(directory.path())?;
    let published = publication_gate(&app);
    settle_initial_maintenance(&mut app, &published).await?;
    let (held_tickets, held_delivery) = hold_delivery(&app, 7).await?;
    let gate = WorkerGate::new();
    let _release = ReleaseOnDrop(gate.clone());
    let observations = Arc::new(Mutex::new(Vec::new()));
    let running = start_blocked(&mut app, &published, &gate, &observations).await?;
    let before = observations.lock().unwrap().len();
    let (started, did_start) = oneshot::channel();
    let foreground = Arc::new(Mutex::new(None));
    let observed_foreground = foreground.clone();
    app.background("reader-render-speculative-foreground", move || {
        let _ = started.send(());
        Ok(42_u8)
    }, move |_, result| *observed_foreground.lock().unwrap() = Some(result.unwrap()));
    tokio::time::timeout(TEST_DEADLINE, did_start).await
        .context("Foreground work waited for the blocked speculative worker")??;
    assert!(running.cancelled.load(Ordering::Acquire));
    assert!(!*gate.released.lock().unwrap());
    apply_until(&mut app, &published, |_| foreground.lock().unwrap().is_some()).await?;
    assert_eq!(*foreground.lock().unwrap(), Some(42));
    assert_eq!(observations.lock().unwrap().len(), before);
    gate.release();
    apply_until(&mut app, &published, |_| observations.lock().unwrap().len() > before).await?;
    let observations = observations.lock().unwrap();
    assert_eq!(observations.len(), before + 1);
    let result = observations.last().unwrap();
    assert!(result.failed && result.cancelled);
    assert!(!result.pending_during_callback);
    assert!(!app.pending.contains_key(PREPARE_KEY));
    drop(held_delivery);
    drop(held_tickets);
    Ok(())
}

#[derive(Clone, Copy)]
enum Invalidation { Key, Epoch, Close }

#[tokio::test]
async fn invalidation_retires_the_ui_marker_but_keeps_the_worker_and_prediction_lane() -> Result<()> {
    let _serial = source::work::process_test_lock().lock().await;
    for invalidation in [Invalidation::Key, Invalidation::Epoch, Invalidation::Close] {
        let directory = tempfile::tempdir()?;
        let mut app = fixture(directory.path())?;
        let published = publication_gate(&app);
        settle_initial_maintenance(&mut app, &published).await?;
        let (held_tickets, held_delivery) = hold_delivery(&app, 7).await?;
        let gate = WorkerGate::new();
        let _release = ReleaseOnDrop(gate.clone());
        let observations = Arc::new(Mutex::new(Vec::new()));
        let running = start_blocked(&mut app, &published, &gate, &observations).await?;
        let calls_before = observations.lock().unwrap().len();
        match invalidation {
            Invalidation::Key => app.invalidate(PREPARE_KEY),
            Invalidation::Epoch => { app.jobs.clear(); app.pending.clear(); app.epoch += 1; },
            Invalidation::Close => { app.jobs.close(); app.pending.clear(); },
        }
        tokio::time::timeout(TEST_DEADLINE, running.callback_dropped).await
            .context("The invalid prediction ticket kept its awaiting callback alive")??;
        assert!(running.cancelled.load(Ordering::Acquire));
        assert!(!app.pending.contains_key(PREPARE_KEY));
        assert_eq!(observations.lock().unwrap().len(), calls_before);
        assert!(app.jobs.take().is_none());

        let next_directory = tempfile::tempdir()?;
        let mut next = fixture(next_directory.path())?;
        for _ in 0..4 {
            assert!(!next.background_speculative(PREPARE_KEY, None, Arc::new(AtomicBool::new(false)),
                || Ok(()), |_, _| panic!("A prediction passed a still-running speculative worker")));
            assert_eq!(next.sequence, 0);
            assert!(next.pending.is_empty());
            assert!(!next.jobs.is_current(next.epoch, 1, PREPARE_KEY));
        }
        if !matches!(invalidation, Invalidation::Close) {
            assert!(app.jobs.try_begin_with_delivery(app.epoch, 200, PREPARE_KEY)?.is_none(),
                "UI invalidation freed the blocked worker's prediction delivery lane");
            let ticket = app.jobs.begin(app.epoch, 200, "speculative-delivery-after-invalidation", false)?;
            let mut delivery = Box::pin(ticket.acquire_delivery());
            let polled = poll_fn(|context| Poll::Ready(Future::poll(delivery.as_mut(), context))).await;
            let Poll::Ready(Some(permit)) = polled else {
                anyhow::bail!("A blocked prediction consumed the eighth ordinary delivery slot");
            };
            drop(delivery);
            drop(permit);
            drop(ticket);
            gate.release();
            let replacement = tokio::time::timeout(TEST_DEADLINE, async {
                loop {
                    if let Some(admission) = app.jobs.try_begin_with_delivery(app.epoch, 201, PREPARE_KEY)? {
                        return Ok::<_, anyhow::Error>(admission);
                    }
                    tokio::task::yield_now().await;
                }
            }).await.context("The exited prediction retained its dedicated delivery lane")??;
            drop(replacement);
        } else {
            assert!(matches!(app.jobs.try_begin_with_delivery(app.epoch, 200, PREPARE_KEY),
                Err(jobs::CapacityError::Closed)));
            gate.release();
        }
        let next_published = publication_gate(&next);
        let next_observations = Arc::new(Mutex::new(Vec::new()));
        complete_case(&mut next, &next_published, Outcome::Success, &next_observations).await?;
        assert_eq!(observations.lock().unwrap().len(), calls_before);
        assert!(app.jobs.take().is_none());
        drop(held_delivery);
        drop(held_tickets);
    }
    Ok(())
}

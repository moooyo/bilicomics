//! Explicit worker and delivery gates for optional cover ownership contracts.

use super::*;
use std::sync::{Condvar, Mutex};
use tokio::sync::Semaphore;

const DEADLINE: Duration = Duration::from_secs(8);

async fn next_event(app: &App) -> Result<JobEvent> {
    let wakeup = Arc::new(Notify::new());
    let notify = wakeup.clone();
    app.jobs.set_wakeup(Arc::new(move || notify.notify_one()));
    tokio::time::timeout(DEADLINE, async {
        loop {
            if let Some(event) = app.jobs.take() { return event; }
            wakeup.notified().await;
        }
    }).await.context("The optional-cover result was not delivered")
}

struct WorkGate {
    entered: Semaphore,
    released: Mutex<bool>,
    changed: Condvar,
}

impl WorkGate {
    fn new() -> Arc<Self> {
        Arc::new(Self { entered: Semaphore::new(0), released: Mutex::new(false), changed: Condvar::new() })
    }

    fn wait(&self) {
        self.entered.add_permits(1);
        let mut released = self.released.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*released { released = self.changed.wait(released).unwrap_or_else(std::sync::PoisonError::into_inner); }
    }

    fn release(&self) {
        *self.released.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.changed.notify_all();
    }
}

struct ReleaseGate(Arc<WorkGate>);

impl Drop for ReleaseGate {
    fn drop(&mut self) { self.0.release(); }
}

struct BlockedCover {
    token: PreparationToken,
    gate: Arc<WorkGate>,
    _release: ReleaseGate,
}

async fn start_blocked(app: &mut App) -> Result<BlockedCover> {
    tokio::time::timeout(DEADLINE, async {
        loop {
            let gate = WorkGate::new();
            let release = ReleaseGate(gate.clone());
            let worker_gate = gate.clone();
            let token = PreparationToken::new(app.sleep_screen.generation);
            let worker_token = token.clone();
            let callback_token = token.clone();
            let identity = app.sleep_screen.identity.clone().context("The fixture needs a cover identity")?;
            if !app.background_optional_cover(LOCAL_JOB, None, token.cancelled.clone(), token.drained.clone(), move || {
                worker_gate.wait();
                worker_token.check()?;
                Ok(())
            }, move |app, _| { app.complete_sleep_cover_preparation(&identity, &callback_token); }) {
                tokio::task::yield_now().await;
                continue;
            }
            app.track_sleep_cover_preparation(LOCAL_JOB, token.clone());
            enum Arrival { Entered, Result(JobEvent) }
            let arrival = tokio::select! {
                permit = gate.entered.acquire() => {
                    permit.context("The worker gate was closed")?.forget();
                    Arrival::Entered
                },
                event = next_event(app) => Arrival::Result(event?),
            };
            match arrival {
                Arrival::Entered => return Ok(BlockedCover { token, gate, _release: release }),
                Arrival::Result(event) => {
                    app.pending.remove(&event.key);
                    (event.apply)(app);
                    app.reap_sleep_cover_preparation();
                    anyhow::ensure!(token.is_cancelled() && token.drained.load(Ordering::Acquire),
                        "A worker finished without entering the gate or being cancelled");
                    // Other foreground fixtures share the process budget. An
                    // attempt cancelled before its closure starts is retried.
                }
            }
        }
    }).await.context("The cover worker did not obtain an idle process budget")?
}

#[tokio::test]
async fn cancellation_and_ui_invalidation_keep_the_real_worker_and_delivery_reserved() -> Result<()> {
    let _serial = source::work::process_test_lock().lock().await;
    let directory = tempfile::tempdir()?;
    let mut app = reader_app(directory.path(), tokio::runtime::Handle::current())?;
    app.sleep_screen.identity = Some(CoverIdentity::new(&app.account, app.epoch, &comic()).unwrap());
    app.install_sleep_cover(prepared_cover(16, 24, 68));
    let retry_at = Instant::now() + Duration::from_secs(60);
    app.sleep_screen.retry_at = Some(retry_at);
    let running = start_blocked(&mut app).await?;
    app.defer_sleep_cover_for_reader_activity();
    app.invalidate(LOCAL_JOB);
    app.reap_sleep_cover_preparation();
    assert!(app.sleep_screen.pending);
    assert!(!running.token.drained.load(Ordering::Acquire));
    assert_eq!(app.sleep_cover_resource_snapshot()["draining"], true);
    assert!(app.jobs.try_begin_with_delivery(app.epoch, app.sequence + 1, "cover-drain-probe")?.is_none());
    let prediction_cancelled = Arc::new(AtomicBool::new(false));
    assert!(!app.background_speculative("reader-prepare-next", None, prediction_cancelled.clone(),
        || Ok(()), |_, _| panic!("A prediction passed the actual blocked cover worker")));
    assert!(!prediction_cancelled.load(Ordering::Acquire));
    assert_eq!(app.sleep_screen.retry_at, Some(retry_at));
    assert_eq!(dimensions(app.sleep_screen.cover.as_ref().unwrap()), (16, 24));
    running.gate.release();
    tokio::time::timeout(DEADLINE, async {
        while !running.token.drained.load(Ordering::Acquire)
            || source::work::speculative_in_flight_for(&running.token.cancelled) { tokio::task::yield_now().await; }
    }).await.context("The completed worker did not return its drain guard")?;
    app.reap_sleep_cover_preparation();
    assert!(!app.sleep_screen.pending);
    assert!(app.sleep_screen.preparation.is_none());
    assert!(app.sleep_screen.full_resolution);
    assert_eq!(app.sleep_screen.retry_at, Some(retry_at));
    assert!(!app.optional_cover_work_allowed(), "Draining cancellation must not restart optional work immediately");
    Ok(())
}

#[tokio::test]
async fn an_early_guard_drop_cannot_retire_a_still_owned_speculative_slot() -> Result<()> {
    let _serial = source::work::process_test_lock().lock().await;
    let directory = tempfile::tempdir()?;
    let mut app = reader_app(directory.path(), tokio::runtime::Handle::current())?;
    let token = PreparationToken::new(app.sleep_screen.generation);
    let lease = tokio::time::timeout(DEADLINE, async {
        loop {
            if let Some(lease) = source::work::try_speculative(source::work::COVER_WORK_BYTES, token.cancelled.clone()) {
                break lease;
            }
            tokio::task::yield_now().await;
        }
    }).await.context("The early-drop fixture did not obtain an idle slot")?;
    token.cancel();
    token.drained.store(true, Ordering::Release);
    app.track_sleep_cover_preparation(LOCAL_JOB, token.clone());
    assert!(source::work::speculative_in_flight_for(&token.cancelled));
    assert!(!source::work::speculative_in_flight_for(&Arc::new(AtomicBool::new(true))),
        "Equal cancellation values must not alias an unrelated owner");
    app.reap_sleep_cover_preparation();
    assert!(app.sleep_screen.pending && app.sleep_screen.preparation.is_some());
    assert_eq!(app.sleep_cover_resource_snapshot()["draining"], true);
    drop(lease);
    assert!(!source::work::speculative_in_flight_for(&token.cancelled));
    app.reap_sleep_cover_preparation();
    assert!(!app.sleep_screen.pending && app.sleep_screen.preparation.is_none());
    Ok(())
}

#[tokio::test]
async fn a_completed_worker_remains_pending_until_its_result_envelope_is_drained() -> Result<()> {
    let _serial = source::work::process_test_lock().lock().await;
    let directory = tempfile::tempdir()?;
    let mut app = reader_app(directory.path(), tokio::runtime::Handle::current())?;
    app.sleep_screen.identity = Some(CoverIdentity::new(&app.account, app.epoch, &comic()).unwrap());
    let running = start_blocked(&mut app).await?;
    running.gate.release();
    let event = next_event(&app).await?;
    assert!(!running.token.drained.load(Ordering::Acquire));
    assert!(app.sleep_screen.pending);
    assert!(app.jobs.try_begin_with_delivery(app.epoch, app.sequence + 1, "cover-envelope-probe")?.is_none());
    app.pending.remove(&event.key);
    (event.apply)(&mut app);
    assert!(running.token.drained.load(Ordering::Acquire));
    app.reap_sleep_cover_preparation();
    assert!(!app.sleep_screen.pending);
    let admission = app.jobs.try_begin_with_delivery(app.epoch, app.sequence + 1, "cover-envelope-probe")?;
    assert!(admission.is_some());
    drop(admission);
    Ok(())
}

#[tokio::test]
async fn a_network_candidate_cannot_preempt_prediction_or_disappear_on_busy_prepare() -> Result<()> {
    let _serial = source::work::process_test_lock().lock().await;
    let directory = tempfile::tempdir()?;
    let mut app = reader_app(directory.path(), tokio::runtime::Handle::current())?;
    let transport = connect_fixture(&mut app)?;
    let identity = CoverIdentity::new(&app.account, app.epoch, &comic()).unwrap();
    app.sleep_screen.identity = Some(identity.clone());
    app.sleep_screen.idle_not_before = None;
    tokio::time::timeout(DEADLINE, async {
        loop {
            let cancelled = Arc::new(AtomicBool::new(false));
            let Some(lease) = source::work::try_speculative(source::work::HEAVY_WORK_BYTES, cancelled.clone()) else {
                tokio::task::yield_now().await;
                continue;
            };
            let candidate = app.client.download_cover_candidate(COVER_URL).await?;
            let bytes = candidate.encoded_len();
            if cancelled.load(Ordering::Acquire) { drop(lease); continue; }
            app.sleep_screen.candidate = Some(candidate);
            app.sleep_screen.retry_at = None;
            app.prepare_sleep_cover_candidate(identity.clone());
            if cancelled.load(Ordering::Acquire) { drop(lease); continue; }
            assert!(!app.sleep_screen.pending);
            assert_eq!(app.sleep_screen.candidate.as_ref().unwrap().encoded_len(), bytes);
            assert!(app.sleep_screen.retry_at.is_some());
            assert!(app.pending.is_empty());
            assert!(!directory.path().join("covers").join(&identity.file_name).exists());
            drop(lease);
            break;
        }
        Ok::<(), anyhow::Error>(())
    }).await.context("No uncontested candidate response could be observed")??;
    assert!(transport.requests.load(Ordering::SeqCst) >= 1);
    Ok(())
}

#[tokio::test]
async fn reader_entry_discards_an_already_taken_ordinary_cover_response_and_keeps_preview() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = reader_app(directory.path(), tokio::runtime::Handle::current())?;
    let transport = connect_fixture(&mut app)?;
    seed_thumbnail(&mut app);
    app.route = "detail".into();
    app.request_cover(comic(), covers::cached_cover_path(&app.data_dir, COVER_URL));
    let event = next_event(&app).await?;
    assert_eq!(event.key, format!("cover:{}", comic().id));
    app.route = "reader".into();
    app.defer_sleep_cover_for_reader_activity();
    assert!(app.cover_pending.is_empty());
    assert!(!app.pending.contains_key(&event.key));
    // The consumer had already removed this envelope, so its independent
    // cover token must reject it even after normal job invalidation.
    (event.apply)(&mut app);
    assert!(!app.pending.contains_key(&format!("cover-prepare:{}", comic().id)));
    assert_eq!(dimensions(&app.cached_cover_preview(&comic()).unwrap()), (8, 12));
    assert_eq!(transport.requests.load(Ordering::SeqCst), 1);
    assert!(!covers::cached_cover_path(&app.data_dir, COVER_URL).exists());
    Ok(())
}

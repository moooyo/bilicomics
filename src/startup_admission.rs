//! Cancelable startup admission without retaining a UI object on a worker.

use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::sync::watch;

const SIGNAL_WAIT_INTERVAL: Duration = Duration::from_millis(25);

/// The early signal and the original UI-turn fallback share one actual start.
#[derive(Clone)]
pub(crate) struct StartupAdmission {
    started: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
}

impl StartupAdmission {
    pub(crate) fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self { started: Arc::new(AtomicBool::new(false)), cancelled }
    }

    pub(crate) fn start_once(&self, start: impl FnOnce()) -> bool {
        if self.cancelled.load(Ordering::Acquire)
            || self.started.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err()
        {
            return false;
        }
        start();
        true
    }

    pub(crate) fn started(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UiTurnState {
    Waiting,
    Yielded,
    Closed,
}

/// Covers keep their original first-bootstrap-turn admission boundary.
#[derive(Clone)]
pub(crate) struct CoverPreparationGate {
    state: watch::Sender<UiTurnState>,
}

impl Default for CoverPreparationGate {
    fn default() -> Self {
        let (state, _) = watch::channel(UiTurnState::Waiting);
        Self { state }
    }
}

impl CoverPreparationGate {
    fn release(&self) {
        self.state.send_if_modified(|state| {
            if *state != UiTurnState::Waiting { return false; }
            *state = UiTurnState::Yielded;
            true
        });
    }

    fn close(&self) {
        self.state.send_replace(UiTurnState::Closed);
    }

    async fn wait(&self, cancelled: &AtomicBool) -> bool {
        let mut state = self.state.subscribe();
        loop {
            if cancelled.load(Ordering::Acquire) { return false; }
            match *state.borrow_and_update() {
                UiTurnState::Yielded => return true,
                UiTurnState::Closed => return false,
                UiTurnState::Waiting => {}
            }
            tokio::select! {
                changed = state.changed() => {
                    if changed.is_err() { return false; }
                }
                // Cancellation also ends the wait if a caller has not closed it yet.
                _ = tokio::time::sleep(SIGNAL_WAIT_INTERVAL) => {}
            }
        }
    }
}

/// This UI-owned flag deliberately does not follow background admission.
pub(crate) struct StartupUiTurn {
    yielded: bool,
    covers: CoverPreparationGate,
}

impl StartupUiTurn {
    pub(crate) fn new(covers: CoverPreparationGate) -> Self {
        Self { yielded: false, covers }
    }

    pub(crate) fn has_yielded(&self) -> bool { self.yielded }

    pub(crate) fn yield_now(&mut self) {
        self.yielded = true;
        self.covers.release();
    }
}

/// Wait asynchronously between metadata preparation and cover pixel work.
/// A canceled or failed preparation cannot publish a completed application.
pub(crate) async fn complete_after_ui_turn<M, R>(
    handle: &tokio::runtime::Handle,
    metadata: tokio::task::JoinHandle<Result<Option<M>>>,
    covers: CoverPreparationGate,
    cancelled: Arc<AtomicBool>,
    complete: impl FnOnce(M) -> Result<R> + Send + 'static,
) -> Result<Option<R>>
where
    M: Send + 'static,
    R: Send + 'static,
{
    let metadata = metadata.await;
    if cancelled.load(Ordering::Acquire) { return Ok(None); }
    let Some(metadata) = metadata.context("Startup metadata worker stopped")?? else {
        return Ok(None);
    };
    if !covers.wait(&cancelled).await { return Ok(None); }
    let completion_cancelled = cancelled.clone();
    let completed = handle.spawn_blocking(move || {
        if completion_cancelled.load(Ordering::Acquire) { return Ok(None); }
        complete(metadata).map(Some)
    }).await;
    if cancelled.load(Ordering::Acquire) { return Ok(None); }
    completed.context("Startup cover worker stopped")?
}

/// Declared after the runtime so every early return retires its waiter first.
/// The retirement closure stays on the UI thread and never invokes UI callbacks.
pub(crate) struct StartupLifetime {
    cancelled: Arc<AtomicBool>,
    covers: CoverPreparationGate,
    retire: Option<Box<dyn FnOnce()>>,
    waiter: Option<JoinHandle<()>>,
}

impl StartupLifetime {
    pub(crate) fn new(cancelled: Arc<AtomicBool>, covers: CoverPreparationGate) -> Self {
        Self { cancelled, covers, retire: None, waiter: None }
    }

    pub(crate) fn set_retirement(&mut self, retire: impl FnOnce() + 'static) {
        self.retire = Some(Box::new(retire));
    }

    /// The callback only queues Tokio work; this thread never owns native I/O.
    pub(crate) fn wait_for_signal(
        &mut self,
        receiver: Receiver<()>,
        submitted: impl FnOnce() + Send + 'static,
    ) -> io::Result<()> {
        let cancelled = self.cancelled.clone();
        self.waiter = Some(thread::Builder::new().name("bilicomics-first-frame".into()).spawn(move || {
            loop {
                if cancelled.load(Ordering::Acquire) { return; }
                match receiver.recv_timeout(SIGNAL_WAIT_INTERVAL) {
                    Ok(()) => {
                        if !cancelled.load(Ordering::Acquire) { submitted(); }
                        return;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        })?);
        Ok(())
    }

    pub(crate) fn stop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(retire) = self.retire.take() { retire(); }
        self.covers.close();
        if let Some(waiter) = self.waiter.take() {
            // A live sender can delay cancellation by at most one bounded wait.
            let _ = waiter.join();
        }
    }
}

impl Drop for StartupLifetime {
    fn drop(&mut self) { self.stop(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::Future,
        sync::{Barrier, atomic::AtomicUsize, mpsc},
        task::{Context as TaskContext, Waker},
        time::Instant,
    };

    #[test]
    fn early_and_fallback_admission_start_exactly_once() {
        let admission = StartupAdmission::new(Arc::new(AtomicBool::new(false)));
        let race = Arc::new(Barrier::new(3));
        let starts = Arc::new(AtomicUsize::new(0));
        thread::scope(|scope| {
            for _ in 0..2 {
                let admission = admission.clone();
                let race = race.clone();
                let starts = starts.clone();
                scope.spawn(move || {
                    race.wait();
                    admission.start_once(|| { starts.fetch_add(1, Ordering::AcqRel); });
                });
            }
            race.wait();
        });
        assert!(admission.started());
        assert_eq!(starts.load(Ordering::Acquire), 1);
        assert!(!admission.start_once(|| panic!("duplicate start")));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn early_admission_does_not_remove_the_original_ui_yield() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let admission = StartupAdmission::new(cancelled.clone());
        let covers = CoverPreparationGate::default();
        let mut ui_turn = StartupUiTurn::new(covers.clone());
        assert!(admission.start_once(|| {}));
        assert!(!ui_turn.has_yielded());
        let handle = tokio::runtime::Handle::current();
        let metadata = handle.spawn_blocking(|| Ok(Some(7)));
        while !metadata.is_finished() { tokio::task::yield_now().await; }
        let completions = Arc::new(AtomicUsize::new(0));
        let observed = completions.clone();
        let mut waiting = std::pin::pin!(complete_after_ui_turn(&handle, metadata, covers,
            cancelled, move |value| {
                observed.fetch_add(1, Ordering::AcqRel);
                Ok(value)
            }));
        let mut context = TaskContext::from_waker(Waker::noop());
        assert!(waiting.as_mut().poll(&mut context).is_pending());
        assert_eq!(completions.load(Ordering::Acquire), 0);
        ui_turn.yield_now();
        assert!(ui_turn.has_yielded());
        assert_eq!(waiting.await.unwrap(), Some(7));
        assert_eq!(completions.load(Ordering::Acquire), 1);
    }

    #[test]
    fn exit_before_the_first_frame_cancels_and_joins_a_live_sender() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let covers = CoverPreparationGate::default();
        let (sender, receiver) = mpsc::channel();
        let submitted = Arc::new(AtomicBool::new(false));
        let observed = submitted.clone();
        let mut lifetime = StartupLifetime::new(cancelled.clone(), covers);
        lifetime.wait_for_signal(receiver, move || { observed.store(true, Ordering::Release); }).unwrap();
        let stopping = Instant::now();
        drop(lifetime);
        assert!(stopping.elapsed() < Duration::from_secs(2));
        assert!(cancelled.load(Ordering::Acquire));
        assert!(!submitted.load(Ordering::Acquire));
        assert!(sender.send(()).is_err());
    }

    #[test]
    fn a_disconnected_signal_does_not_admit_preparation() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        let mut lifetime = StartupLifetime::new(cancelled, CoverPreparationGate::default());
        lifetime.wait_for_signal(receiver, || panic!("disconnection is not submission")).unwrap();
        lifetime.waiter.take().unwrap().join().unwrap();
    }

    #[test]
    fn retirement_is_idempotent_and_cancellation_rejects_late_admission() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let admission = StartupAdmission::new(cancelled.clone());
        let retirements = Arc::new(AtomicUsize::new(0));
        let observed = retirements.clone();
        let mut lifetime = StartupLifetime::new(cancelled, CoverPreparationGate::default());
        lifetime.set_retirement(move || { observed.fetch_add(1, Ordering::AcqRel); });
        lifetime.stop();
        lifetime.stop();
        drop(lifetime);
        assert_eq!(retirements.load(Ordering::Acquire), 1);
        assert!(!admission.start_once(|| panic!("canceled start")));
        assert!(!admission.started());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn metadata_failure_never_waits_for_covers_or_publishes_ready() {
        let handle = tokio::runtime::Handle::current();
        let metadata = handle.spawn_blocking(|| -> Result<Option<()>> { anyhow::bail!("fixture failure") });
        let result = complete_after_ui_turn(&handle, metadata, CoverPreparationGate::default(),
            Arc::new(AtomicBool::new(false)), |()| -> Result<()> { panic!("failed metadata reached covers") }).await;
        assert!(result.unwrap_err().to_string().contains("fixture failure"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_closed_gate_drops_prepared_metadata_without_cover_work() {
        let handle = tokio::runtime::Handle::current();
        let metadata = handle.spawn_blocking(|| Ok(Some(7)));
        let covers = CoverPreparationGate::default();
        covers.close();
        let result = complete_after_ui_turn(&handle, metadata, covers,
            Arc::new(AtomicBool::new(false)), |_| -> Result<()> { panic!("retired covers started") }).await;
        assert_eq!(result.unwrap(), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_while_waiting_does_not_publish_ready() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let handle = tokio::runtime::Handle::current();
        let metadata = handle.spawn_blocking(|| Ok(Some(7)));
        while !metadata.is_finished() { tokio::task::yield_now().await; }
        let gate = CoverPreparationGate::default();
        let mut waiting = std::pin::pin!(complete_after_ui_turn(&handle, metadata, gate, cancelled.clone(),
            |_| -> Result<()> { panic!("canceled covers started") }));
        let mut context = TaskContext::from_waker(Waker::noop());
        assert!(waiting.as_mut().poll(&mut context).is_pending());
        cancelled.store(true, Ordering::Release);
        assert_eq!(waiting.await.unwrap(), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_after_cover_work_still_discards_the_ready_value() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let handle = tokio::runtime::Handle::current();
        let metadata = handle.spawn_blocking(|| Ok(Some(7)));
        let covers = CoverPreparationGate::default();
        covers.release();
        let completed_cancelled = cancelled.clone();
        let result = complete_after_ui_turn(&handle, metadata, covers, cancelled, move |value| {
            completed_cancelled.store(true, Ordering::Release);
            Ok(value)
        }).await;
        assert_eq!(result.unwrap(), None);
    }
}

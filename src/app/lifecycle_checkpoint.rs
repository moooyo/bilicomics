//! Background-owned, revocable local checkpoints while native drawing is parked.

use std::{path::PathBuf, sync::{Arc, atomic::{AtomicBool, Ordering}}};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use super::{App, LocalAttempt, LocalWrite, LocalWrites};
use crate::{domain::ReadingAnchor, source::work, store::Store};
use super::super::{account, lifecycle_maintenance::{RemoteApp, WakePlan, WakeToken}, now};

struct CheckpointInput {
    database: PathBuf,
    account: String,
    reader: Option<(String, String, ReadingAnchor, bool)>,
    preferences: Value,
    deferred: Option<Value>,
    writes: LocalWrites,
    observed_at: i64,
}

async fn worker<T, F>(revoked: Arc<AtomicBool>, task: F) -> Result<T>
where T: Send + 'static, F: FnOnce() -> Result<T> + Send + 'static {
    work::run_blocking(work::METADATA_WORK_BYTES, move |cancel| {
        ensure!(!revoked.load(Ordering::Acquire) && !cancel.is_cancelled(), "Sleep checkpoint was superseded");
        task()
    }).await.map_err(anyhow::Error::new)?
}

async fn persist_state(input: &CheckpointInput, token: &WakeToken,
    anchor: Option<(String, ReadingAnchor)>, preferences: Vec<(String, Value)>) -> Result<()> {
    let database = input.database.clone();
    let account = input.account.clone();
    let revoked = token.cancelled.clone();
    let fence = revoked.clone();
    let observed_at = input.observed_at;
    worker(revoked, move || {
        let store = Store::open(database)?;
        store.write_sleep_checkpoint(&account, anchor.as_ref().map(|(comic, anchor)| (comic.as_str(), anchor)),
            &preferences, &[], observed_at, &fence)
    }).await
}

async fn persist_downloads(remote: &RemoteApp, input: &CheckpointInput, token: &WakeToken) -> Result<()> {
    if let Some(deferred) = &input.deferred {
        persist_state(input, token, None, vec![("queued_download_intents".into(), deferred.clone())]).await?;
    }
    let mut after = None::<String>;
    loop {
        let database = input.database.clone();
        let account = input.account.clone();
        let cursor = after.clone();
        let records = worker(token.cancelled.clone(), move ||
            Store::open(database)?.active_downloads_page(&account, cursor.as_deref(), 64)).await?;
        if records.is_empty() { return Ok(()); }
        after = records.last().map(|record| record.id.clone());
        let records = Arc::new(records);
        let stage = records.clone();
        let cycle = token.cycle;
        remote.call(token, move |app| app.stage_sleep_task_checkpoints(cycle, &stage))
            .await.context("The sleep checkpoint scope is inactive")??;
        let database = input.database.clone();
        let account = input.account.clone();
        let revoked = token.cancelled.clone();
        let fence = revoked.clone();
        let observed_at = input.observed_at;
        worker(revoked, move || Store::open(database)?.write_sleep_checkpoint(
            &account, None, &[], &records, observed_at, &fence)).await?;
    }
}

/// Called only by the single pending-only driver. All UI callbacks merely
/// capture or install RAM state; SQLite and receipt retries stay on workers.
pub(super) async fn run_sleep_maintenance(remote: RemoteApp, plan: WakePlan) {
    let Some(input) = remote.call(&plan.token, |app| app.begin_sleep_checkpoint()).await.flatten() else { return; };
    let mut attempted = input.writes;
    attempted.insert(LocalWrite::Metadata);
    let mut attempt = LocalAttempt::default();
    for write in LocalWrite::ORDER {
        if plan.token.cancelled.load(Ordering::Acquire) { return; }
        if !attempted.contains(write) { continue; }
        let result = match write {
            LocalWrite::Reader => match &input.reader {
                Some((comic, _, anchor, false)) => persist_state(&input, &plan.token,
                    Some((comic.clone(), anchor.clone())), Vec::new()).await,
                Some((_, episode_id, anchor, true)) => match serde_json::to_value(anchor) {
                    Ok(value) => persist_state(&input, &plan.token, None,
                        vec![(format!("reader_strip_anchor_{episode_id}"), value)]).await,
                    Err(error) => Err(error.into()),
                },
                None => Ok(()),
            },
            LocalWrite::Preferences => persist_state(&input, &plan.token, None,
                vec![("preferences".into(), input.preferences.clone())]).await,
            LocalWrite::Downloads => persist_downloads(&remote, &input, &plan.token).await,
            LocalWrite::Receipts => worker(plan.token.cancelled.clone(), account::retry_observed_receipts).await,
            LocalWrite::Metadata => {
                let database = input.database.clone();
                worker(plan.token.cancelled.clone(), move || Store::open(database)?.checkpoint()).await
            }
        };
        if result.is_err() {
            attempt.failed.insert(write);
            attempt.errors.push(write.error());
        }
    }
    if plan.token.cancelled.load(Ordering::Acquire) { return; }
    remote.call(&plan.token, move |app| app.finish_sleep_checkpoint(attempted, attempt.failed)).await;
}

impl App {
    fn begin_sleep_checkpoint(&mut self) -> Option<CheckpointInput> {
        let writes = self.lifecycle.local_retry.due(std::time::Instant::now())?;
        if self.lifecycle.maintenance_running { return None; }
        let prepared = (|| -> Result<CheckpointInput> {
            Ok(CheckpointInput {
                database: self.store.path().to_owned(), account: self.account.clone(),
                reader: self.reader_sleep_checkpoint(),
                preferences: serde_json::to_value(&self.preferences)?,
                deferred: if writes.contains(LocalWrite::Downloads) {
                    Some(self.prepare_sleep_deferred_checkpoint(self.lifecycle.cycle)?)
                } else { None },
                writes, observed_at: now(),
            })
        })();
        match prepared {
            Ok(input) => {
                self.lifecycle.maintenance_running = true;
                self.lifecycle.maintenance_deadline = None;
                if let Some(driver) = &self.lifecycle.maintenance { driver.set_plan(None); }
                Some(input)
            }
            Err(_) => {
                self.lifecycle.checkpoint_failed = true;
                self.lifecycle.local_retry.completed(writes, std::time::Instant::now());
                self.reschedule_sleep_maintenance();
                None
            }
        }
    }

    fn finish_sleep_checkpoint(&mut self, attempted: LocalWrites, mut failed: LocalWrites) {
        // A receipt can arrive after this worker cloned the outbox or a new
        // failure can be reported while an earlier batch is still in flight.
        let carried = LocalWrites(self.lifecycle.local_retry.pending.0 & !attempted.0);
        failed.0 |= carried.0;
        if self.account_receipts_pending() { failed.insert(LocalWrite::Receipts); }
        self.lifecycle.checkpoint_failed |= !failed.is_empty();
        self.lifecycle.local_retry.completed(failed, std::time::Instant::now());
        self.lifecycle.maintenance_running = false;
        if let Some(token) = self.lifecycle.maintenance_token.take() { token.cancelled.store(true, Ordering::Release); }
        self.reschedule_sleep_maintenance();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::LocalRetry;
    use crate::{app::test_support, platform::DeviceProfile, source::BiliClient};
    use std::{cell::RefCell, rc::Rc, time::{Duration, Instant}};

    fn app(directory: &std::path::Path, handle: tokio::runtime::Handle) -> Result<App> {
        test_support::app(directory, DeviceProfile::Paperwhite5, BiliClient::offline(directory)?, handle)
    }

    #[test]
    fn a_disarmed_timer_keeps_the_current_attempt_valid_until_resume_revokes_it() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_time().build()?;
        let application = Rc::new(RefCell::new(app(directory.path(), runtime.handle().clone())?));
        App::install_sleep_maintenance(&application)?;
        let mut application = application.borrow_mut();
        application.lifecycle.suspended = true;
        application.lifecycle.cycle = 7;
        application.lifecycle.maintenance_revision = 3;
        let token = WakeToken { epoch: application.epoch, cycle: 7, revision: 3,
            cancelled: Arc::new(AtomicBool::new(false)) };
        application.lifecycle.maintenance_token = Some(token.clone());
        application.lifecycle.maintenance_running = true;
        let driver = application.lifecycle.maintenance.as_ref().unwrap();
        let scope = driver.scope();
        driver.set_plan(None);
        assert!(application.sleep_maintenance_matches(scope, &token));
        assert!(!application.sleep_maintenance_matches(scope, &WakeToken {
            epoch: token.epoch, cycle: token.cycle, revision: token.revision,
            cancelled: Arc::new(AtomicBool::new(false)),
        }));
        assert!(!application.sleep_maintenance_matches(scope, &WakeToken { cycle: 8, ..token.clone() }));
        application.cancel_sleep_maintenance();
        assert!(token.cancelled.load(Ordering::Acquire));
        assert!(!application.sleep_maintenance_matches(scope, &token));
        assert!(!application.lifecycle.maintenance_running);
        Ok(())
    }

    #[test]
    fn completion_preserves_new_receipt_work_that_arrived_during_an_anchor_retry() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_time().build()?;
        let mut application = app(directory.path(), runtime.handle().clone())?;
        application.lifecycle.suspended = true;
        application.lifecycle.maintenance_running = true;
        application.lifecycle.local_retry.request(LocalWrites::one(LocalWrite::Reader), Instant::now());
        let mut attempted = LocalWrites::one(LocalWrite::Reader);
        attempted.insert(LocalWrite::Metadata);
        application.lifecycle.local_retry.request(LocalWrites::one(LocalWrite::Receipts), Instant::now());
        application.finish_sleep_checkpoint(attempted, LocalWrites::default());
        assert_eq!(application.lifecycle.local_retry.pending, LocalWrites::one(LocalWrite::Receipts));
        assert!(application.lifecycle.local_retry.retry_at.is_some());
        assert!(!application.lifecycle.maintenance_running);
        Ok(())
    }

    #[test]
    fn an_idle_installed_controller_does_not_create_periodic_checkpoint_work() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_time().build()?;
        let application = Rc::new(RefCell::new(app(directory.path(), runtime.handle().clone())?));
        App::install_sleep_maintenance(&application)?;
        let mut application = application.borrow_mut();
        application.lifecycle.suspended = true;
        application.lifecycle.local_retry = LocalRetry::default();
        for _ in 0..1_000 { application.tick_suspended()?; }
        assert!(application.lifecycle.local_retry.pending.is_empty());
        assert!(application.lifecycle.maintenance_token.is_none());
        assert!(application.lifecycle.maintenance_deadline.is_none());
        assert!(!application.lifecycle.maintenance_running);
        assert!(!application.lifecycle.checkpoint_failed);
        Ok(())
    }

    #[test]
    fn unchanged_pending_work_keeps_one_deadline_revision() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_time().build()?;
        let application = Rc::new(RefCell::new(app(directory.path(), runtime.handle().clone())?));
        App::install_sleep_maintenance(&application)?;
        let mut application = application.borrow_mut();
        application.lifecycle.suspended = true;
        application.lifecycle.local_retry.completed(LocalWrites::one(LocalWrite::Reader), Instant::now() + Duration::from_secs(3_600));
        application.reschedule_sleep_maintenance();
        let revision = application.lifecycle.maintenance_revision;
        let token = application.lifecycle.maintenance_token.clone().unwrap();
        let deadline = application.lifecycle.maintenance_deadline.unwrap();
        for _ in 0..1_000 { application.reschedule_sleep_maintenance(); }
        assert_eq!(application.lifecycle.maintenance_revision, revision);
        assert_eq!(application.lifecycle.maintenance_deadline, Some(deadline));
        assert!(Arc::ptr_eq(&application.lifecycle.maintenance_token.as_ref().unwrap().cancelled, &token.cancelled));
        assert_eq!(application.lifecycle.local_retry.retry_at, Some(deadline));
        application.cancel_sleep_maintenance();
        Ok(())
    }
}

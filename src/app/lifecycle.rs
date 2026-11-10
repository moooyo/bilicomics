use std::{cell::RefCell, rc::Rc, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};

use anyhow::{Result, anyhow};
use slint::ComponentHandle;

use super::App;
use super::lifecycle_maintenance::{MaintenanceDriver, RemoteApp, WakePlan, WakeToken};
use crate::{
    downloads::DownloadPauseHandle,
    platform::{RefreshKind, RefreshPlan},
    ui::ShellModel,
};

const LOCAL_RETRY_INITIAL: Duration = Duration::from_secs(1);
const LOCAL_RETRY_MAXIMUM: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocalWrite {
    Reader,
    Preferences,
    Downloads,
    Receipts,
    Metadata,
}

impl LocalWrite {
    const ORDER: [Self; 5] = [Self::Reader, Self::Preferences, Self::Downloads, Self::Receipts, Self::Metadata];

    const fn bit(self) -> u8 { 1_u8 << (self as u8) }

    const fn error(self) -> &'static str {
        match self {
            Self::Reader => "reader_position_checkpoint_failed",
            Self::Preferences => "preferences_checkpoint_failed",
            Self::Downloads => "download_pause_checkpoint_failed",
            Self::Receipts => "payment_receipt_checkpoint_failed",
            Self::Metadata => "metadata_checkpoint_failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct LocalWrites(u8);

impl LocalWrites {
    const fn all() -> Self { Self((1_u8 << LocalWrite::ORDER.len()) - 1) }
    const fn one(write: LocalWrite) -> Self { Self(write.bit()) }
    const fn contains(self, write: LocalWrite) -> bool { self.0 & write.bit() != 0 }
    const fn is_empty(self) -> bool { self.0 == 0 }
    fn insert(&mut self, write: LocalWrite) { self.0 |= write.bit(); }
    fn remove(&mut self, write: LocalWrite) { self.0 &= !write.bit(); }
}

#[derive(Default)]
struct LocalRetry {
    pending: LocalWrites,
    retry_at: Option<Instant>,
    backoff_step: u8,
}

impl LocalRetry {
    fn request(&mut self, writes: LocalWrites, now: Instant) {
        if (self.pending.0 | writes.0) == self.pending.0 { return; }
        self.pending.0 |= writes.0;
        // A newly observed receipt must not inherit an older metadata failure's
        // long delay. Repeated observations of the same pending work retain it.
        self.retry_at = Some(now);
        self.backoff_step = 0;
    }

    fn due(&self, now: Instant) -> Option<LocalWrites> {
        (!self.pending.is_empty() && self.retry_at.is_none_or(|at| now >= at)).then_some(self.pending)
    }

    fn completed(&mut self, failed: LocalWrites, now: Instant) {
        self.pending = failed;
        if failed.is_empty() {
            self.retry_at = None;
            self.backoff_step = 0;
        } else {
            let delay = LOCAL_RETRY_INITIAL.saturating_mul(1_u32 << self.backoff_step.min(5))
                .min(LOCAL_RETRY_MAXIMUM);
            self.retry_at = Some(now + delay);
            self.backoff_step = self.backoff_step.saturating_add(1).min(5);
        }
    }
}

#[derive(Default)]
struct LocalAttempt {
    failed: LocalWrites,
    errors: Vec<&'static str>,
}

impl LocalAttempt {
    fn result(self) -> Result<()> {
        if self.errors.is_empty() { Ok(()) } else { Err(anyhow!(self.errors.join("; "))) }
    }
}

fn attempt_local_writes<F>(mut writes: LocalWrites, mut persist: F) -> LocalAttempt
where F: FnMut(LocalWrite) -> Result<()> {
    if writes.is_empty() { return LocalAttempt::default(); }
    // A retried local write can append to WAL even if the earlier checkpoint
    // succeeded. Checkpoint once after actionable work, never on an idle tick.
    writes.insert(LocalWrite::Metadata);
    let mut attempt = LocalAttempt::default();
    for write in LocalWrite::ORDER {
        if writes.contains(write) && persist(write).is_err() {
            attempt.failed.insert(write);
            attempt.errors.push(write.error());
        }
    }
    attempt
}

#[derive(Default)]
pub(super) struct LifecycleState {
    pub(super) reader_checkpoints: super::reader_checkpoint::ReaderCheckpointWriter,
    suspended: bool,
    cycle: u64,
    checkpoint_failed: bool,
    local_retry: LocalRetry,
    download_pause: Option<(String, u64, DownloadPauseHandle)>,
    maintenance: Option<MaintenanceDriver>,
    maintenance_token: Option<WakeToken>,
    maintenance_revision: u64,
    maintenance_deadline: Option<Instant>,
    maintenance_running: bool,
}

impl App {
    pub fn is_suspended(&self) -> bool { self.lifecycle.suspended }

    /// Close application input and transfer admission without any SQLite work.
    /// Native callers can release display/input leases immediately after this.
    pub fn begin_suspend_intent_gate(&mut self) {
        self.lifecycle.reader_checkpoints.fence();
        if !self.lifecycle.suspended {
            self.retain_reader_frame_for_sleep();
            self.cancel_sleep_maintenance();
            self.lifecycle.suspended = true;
            self.lifecycle.cycle = self.lifecycle.cycle.wrapping_add(1);
            self.lifecycle.checkpoint_failed = false;
            self.lifecycle.local_retry = LocalRetry::default();
        }
        if let Some(ui) = self.ui.upgrade() { ui.global::<ShellModel>().set_suspended(true); }
        self.cache_download_pause_handle();
        self.set_download_transport_suspended(true);
        self.release_download_wake_guard();
    }

    /// Keep the same application, reader, route, and prepared frames in memory.
    /// A failed checkpoint is reported after all release-related work is tried.
    pub fn suspend(&mut self) -> Result<()> {
        self.begin_suspend_intent_gate();
        self.cancel_sleep_maintenance();
        let attempt = self.persist_local_checkpoint(self.full_local_checkpoint());
        self.reschedule_sleep_maintenance();
        attempt.result()
    }

    /// Resume the retained frame before admitting new read or download work.
    /// Payment and recharge submissions are never dispatched here.
    pub fn resume(&mut self) -> Result<()> {
        if !self.lifecycle.suspended { return Ok(()); }
        self.cancel_sleep_maintenance();
        self.lifecycle.suspended = false;
        if let Some(ui) = self.ui.upgrade() { ui.global::<ShellModel>().set_suspended(false); }
        let mut errors = Vec::new();
        if self.platform.capabilities().explicit_refresh {
            if self.platform.submit_refresh(RefreshPlan { kind: RefreshKind::Full, rect: None }).is_err() {
                errors.push("resume_refresh_unavailable");
            }
        } else if let Some(ui) = self.ui.upgrade() {
            ui.window().request_redraw();
        }
        if self.resume_reader_from_memory().is_err() { errors.push("reader_frame_restore_failed"); }
        // A pre-sleep storage failure must not replace the live reader with an
        // older stored anchor. Retry the current RAM position instead.
        // Wake acknowledgement never waits for a retry deadline. Flush the live
        // RAM position immediately, including any earlier failed local work.
        errors.extend(self.persist_local_checkpoint(self.full_local_checkpoint()).errors);

        self.cache_download_pause_handle();
        self.set_download_transport_suspended(false);
        if self.resume_downloads_from_sleep().is_err() { errors.push("download_resume_failed"); }
        self.dirty = true;
        if self.lifecycle.checkpoint_failed || !errors.is_empty() { self.toast("common_storage_error"); }
        self.lifecycle.checkpoint_failed |= !errors.is_empty();
        if errors.is_empty() { Ok(()) } else { Err(anyhow!(errors.join("; "))) }
    }

    /// Retry only failed local checkpoints or observed payment receipts during sleep.
    /// Returning an error from a periodic tick would discard the retained RAM
    /// reader in callers that treat ordinary tick failures as fatal.
    pub(super) fn tick_suspended(&mut self) -> Result<()> {
        if !self.lifecycle.suspended { return Ok(()); }
        let now = Instant::now();
        // This is an in-memory outbox query. Durable unknown/acknowledged
        // transactions are not dirty work and never cause a new submission.
        if self.account_receipts_pending() {
            self.lifecycle.local_retry.request(LocalWrites::one(LocalWrite::Receipts), now);
        }
        if self.lifecycle.maintenance.is_some() {
            self.reschedule_sleep_maintenance();
            return Ok(());
        }
        if let Some(writes) = self.lifecycle.local_retry.due(now) {
            self.persist_local_checkpoint(writes);
        }
        Ok(())
    }

    fn full_local_checkpoint(&self) -> LocalWrites {
        let mut writes = LocalWrites::all();
        if self.has_sleep_download_snapshot(self.lifecycle.cycle)
            && !self.lifecycle.local_retry.pending.contains(LocalWrite::Downloads) {
            writes.remove(LocalWrite::Downloads);
        }
        writes
    }

    fn persist_local_checkpoint(&mut self, writes: LocalWrites) -> LocalAttempt {
        let attempt = attempt_local_writes(writes, |write| match write {
            LocalWrite::Reader => self.save_reading_position(),
            LocalWrite::Preferences => self.save_preferences(),
            LocalWrite::Downloads => self.pause_downloads_for_sleep(self.lifecycle.cycle),
            LocalWrite::Receipts => self.flush_account_receipts(),
            LocalWrite::Metadata => self.store.checkpoint(),
        });
        self.lifecycle.checkpoint_failed |= !attempt.failed.is_empty();
        self.lifecycle.local_retry.completed(attempt.failed, Instant::now());
        attempt
    }

    pub(super) fn cache_download_pause_handle(&mut self) {
        if let Some(manager) = self.downloads.as_ref() {
            self.lifecycle.download_pause = Some((self.account.clone(), self.epoch, manager.pause_handle()));
        }
    }

    pub(super) fn set_download_transport_suspended(&self, suspended: bool) {
        if let Some((account, epoch, handle)) = self.lifecycle.download_pause.as_ref() {
            if account == &self.account && *epoch == self.epoch {
                if suspended { handle.suspend(); } else { handle.resume(); }
            }
        }
    }

    pub(super) fn mark_lifecycle_checkpoint_error(&mut self) {
        self.lifecycle.checkpoint_failed = true;
        self.lifecycle.local_retry.request(LocalWrites::all(), Instant::now());
        self.reschedule_sleep_maintenance();
    }
    pub(super) fn lifecycle_cycle(&self) -> u64 { self.lifecycle.cycle }

    pub(super) fn install_sleep_maintenance(app: &Rc<RefCell<Self>>) -> Result<()> {
        let driver = MaintenanceDriver::install(app)?;
        app.borrow_mut().lifecycle.maintenance = Some(driver);
        Ok(())
    }

    pub(super) fn sleep_receipt_signal(&self) -> Option<Arc<tokio::sync::Notify>> {
        self.lifecycle.maintenance.as_ref().map(MaintenanceDriver::signal)
    }

    pub(super) fn sleep_receipt_ready(&mut self, scope: u64) {
        if !self.is_suspended() || self.quitting
            || self.lifecycle.maintenance.as_ref().is_none_or(|driver| driver.scope() != scope)
            || !self.account_receipts_pending() { return; }
        self.lifecycle.local_retry.request(LocalWrites::one(LocalWrite::Receipts), Instant::now());
        self.reschedule_sleep_maintenance();
    }

    pub(super) fn sleep_maintenance_matches(&self, scope: u64, token: &WakeToken) -> bool {
        self.is_suspended() && !self.quitting && token.epoch == self.epoch
            && token.cycle == self.lifecycle.cycle && token.revision == self.lifecycle.maintenance_revision
            && !token.cancelled.load(Ordering::Acquire)
            && self.lifecycle.maintenance.as_ref().is_some_and(|driver| driver.scope() == scope)
            && self.lifecycle.maintenance_token.as_ref().is_some_and(|current|
                Arc::ptr_eq(&current.cancelled, &token.cancelled))
    }

    pub(super) fn cancel_sleep_maintenance(&mut self) {
        if let Some(token) = self.lifecycle.maintenance_token.take() { token.cancelled.store(true, Ordering::Release); }
        self.lifecycle.maintenance_running = false;
        self.lifecycle.maintenance_deadline = None;
        if let Some(driver) = &self.lifecycle.maintenance { driver.set_plan(None); }
    }

    fn reschedule_sleep_maintenance(&mut self) {
        if self.lifecycle.maintenance.is_none() { return; }
        if !self.is_suspended() || self.quitting { self.cancel_sleep_maintenance(); return; }
        if self.lifecycle.maintenance_running { return; }
        if self.lifecycle.local_retry.pending.is_empty() {
            if self.lifecycle.maintenance_token.is_some() { self.cancel_sleep_maintenance(); }
            return;
        }
        let deadline = self.lifecycle.local_retry.retry_at.unwrap_or_else(Instant::now);
        if self.lifecycle.maintenance_deadline == Some(deadline)
            && self.lifecycle.maintenance_token.as_ref().is_some_and(|token| !token.cancelled.load(Ordering::Acquire)) { return; }
        if let Some(token) = self.lifecycle.maintenance_token.take() { token.cancelled.store(true, Ordering::Release); }
        let Some(revision) = self.lifecycle.maintenance_revision.checked_add(1) else {
            self.lifecycle.checkpoint_failed = true;
            self.lifecycle.maintenance = None;
            return;
        };
        self.lifecycle.maintenance_revision = revision;
        let token = WakeToken { epoch: self.epoch, cycle: self.lifecycle.cycle, revision,
            cancelled: Arc::new(AtomicBool::new(false)) };
        self.lifecycle.maintenance_token = Some(token.clone());
        self.lifecycle.maintenance_deadline = Some(deadline);
        if let Some(driver) = &self.lifecycle.maintenance { driver.set_plan(Some(WakePlan { token, deadline })); }
    }
}

#[path = "lifecycle_checkpoint.rs"]
mod checkpoint;

pub(super) async fn run_sleep_maintenance(remote: RemoteApp, plan: WakePlan) {
    checkpoint::run_sleep_maintenance(remote, plan).await;
}

#[cfg(test)]
#[path = "lifecycle_retry_tests.rs"]
mod retry_tests;

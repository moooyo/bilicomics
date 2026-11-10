//! A bounded, revocable writer for the latest presented reading position.
//!
//! Ordinary page turns replace one pending snapshot. Lifecycle boundaries revoke
//! the actual worker before making a synchronous checkpoint with the same SQLite
//! transaction fence used by sleep maintenance. No worker owns an App or UI image.

use std::{
    path::Path,
    sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver, TryRecvError}},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, ensure};
use serde::Serialize;
use tokio::{runtime::Handle, task::JoinHandle};

use crate::{domain::ReadingAnchor, store::Store};

const RETRY_INITIAL: Duration = Duration::from_secs(1);
const RETRY_MAXIMUM: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ReaderCheckpoint {
    pub(super) account: String,
    pub(super) epoch: u64,
    pub(super) comic_id: String,
    pub(super) episode_id: String,
    pub(super) anchor: ReadingAnchor,
    pub(super) demo_strip: bool,
}

impl ReaderCheckpoint {
    fn same_scope(&self, other: &Self) -> bool {
        self.account == other.account && self.epoch == other.epoch
            && self.comic_id == other.comic_id && self.episode_id == other.episode_id
            && self.demo_strip == other.demo_strip
    }

    fn persist(&self, store: &Store, revoked: &AtomicBool) -> Result<()> {
        if self.demo_strip {
            // The synthetic strip index has a different episode ID. Its saved
            // preference is owned by the original reader episode instead.
            let preferences = [(format!("reader_strip_anchor_{}", self.episode_id),
                serde_json::to_value(&self.anchor)?)];
            store.write_sleep_checkpoint(&self.account, None, &preferences, &[],
                self.anchor.updated_at, revoked)
        } else {
            store.write_sleep_checkpoint(&self.account, Some((&self.comic_id, &self.anchor)),
                &[], &[], self.anchor.updated_at, revoked)
        }
    }
}

#[derive(Clone)]
struct PendingCheckpoint {
    revision: u64,
    snapshot: ReaderCheckpoint,
    revoked: Arc<AtomicBool>,
}

struct InFlight {
    revision: u64,
    revoked: Arc<AtomicBool>,
    completion: Receiver<Result<()>>,
    task: JoinHandle<()>,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub(super) struct ReaderCheckpointDiagnostics {
    pub(super) pending: bool,
    pub(super) in_flight: bool,
    pub(super) failed_attempts: u64,
    pub(super) completed_writes: u64,
    pub(super) retries: u64,
}

#[derive(Default)]
pub(super) struct ReaderCheckpointWriter {
    pending: Option<PendingCheckpoint>,
    in_flight: Option<InFlight>,
    revision: u64,
    retry_at: Option<Instant>,
    failure_streak: u32,
    failed_attempts: u64,
    completed_writes: u64,
    retries: u64,
}

impl ReaderCheckpointWriter {
    /// Merge only positions owned by the same reader scope. A caller must flush
    /// an older scope before opening another episode or publishing an account.
    pub(super) fn submit_latest(&mut self, snapshot: ReaderCheckpoint) -> Result<()> {
        if let Some(pending) = self.pending.as_ref() {
            ensure!(pending.snapshot.same_scope(&snapshot),
                "A previous reader checkpoint must be flushed before changing its scope");
            if pending.snapshot == snapshot && !pending.revoked.load(Ordering::Acquire) {
                return Ok(());
            }
        }
        self.replace_pending(snapshot)
    }

    pub(super) fn pending_checkpoint(&self) -> Option<&ReaderCheckpoint> {
        self.pending.as_ref().map(|pending| &pending.snapshot)
    }

    /// Revoke an actual worker without SQLite work or waiting for it. The latest
    /// snapshot remains available to a boundary checkpoint and sleep retries.
    pub(super) fn fence(&mut self) {
        if let Some(worker) = self.in_flight.as_ref() {
            worker.revoked.store(true, Ordering::Release);
        }
    }

    /// Drain a completion and admit at most one actual blocking worker. Errors
    /// are reported once per current failed attempt, never on every idle tick.
    pub(super) fn drive(&mut self, handle: &Handle, database: &Path, now: Instant) -> Result<()> {
        self.collect_finished(now)?;
        if self.in_flight.is_some() || self.pending.is_none()
            || self.retry_at.is_some_and(|deadline| now < deadline) {
            return Ok(());
        }
        if self.pending.as_ref().is_some_and(|pending| pending.revoked.load(Ordering::Acquire)) {
            let snapshot = self.pending.as_ref().expect("A pending checkpoint exists").snapshot.clone();
            self.replace_pending(snapshot)?;
        }
        if self.failure_streak > 0 { self.retries = self.retries.saturating_add(1); }
        let database = database.to_owned();
        self.start_worker(handle, move |snapshot, revoked| {
            ensure!(!revoked.load(Ordering::Acquire), "Reader checkpoint was superseded");
            let store = Store::open(database)?;
            snapshot.persist(&store, revoked)
        });
        Ok(())
    }

    /// Flush the latest presented position at a lifecycle boundary. Revocation
    /// precedes the synchronous transaction: an older worker either commits
    /// first or observes its revoked token after obtaining SQLite's write lock.
    /// A failed flush retains the lightweight snapshot for a later retry.
    pub(super) fn flush(&mut self, store: &Store, latest: Option<ReaderCheckpoint>) -> Result<()> {
        if let Some(snapshot) = latest { self.submit_latest(snapshot)?; }
        self.fence();
        let Some(snapshot) = self.pending_checkpoint().cloned() else {
            self.clear_retry();
            return Ok(());
        };
        // A fresh revision prevents the old worker's completion from clearing a
        // failed synchronous snapshot, even when both positions are identical.
        self.replace_pending(snapshot)?;
        if self.failure_streak > 0 { self.retries = self.retries.saturating_add(1); }
        let pending = self.pending.as_ref().expect("The boundary snapshot was retained");
        match pending.snapshot.persist(store, &pending.revoked) {
            Ok(()) => {
                self.completed_writes = self.completed_writes.saturating_add(1);
                self.pending = None;
                self.clear_retry();
                Ok(())
            }
            Err(error) => {
                self.failed(Instant::now());
                Err(error.context("Checkpoint the presented reading position"))
            }
        }
    }

    /// Discard a retired scope without pretending that a running closure ended.
    /// Its slot remains occupied until the actual task completes.
    pub(super) fn retire(&mut self) {
        self.fence();
        if let Some(pending) = self.pending.take() {
            pending.revoked.store(true, Ordering::Release);
        }
        self.clear_retry();
    }

    pub(super) fn diagnostics(&self) -> ReaderCheckpointDiagnostics {
        ReaderCheckpointDiagnostics {
            pending: self.pending.is_some(), in_flight: self.in_flight.is_some(),
            failed_attempts: self.failed_attempts, completed_writes: self.completed_writes,
            retries: self.retries,
        }
    }

    fn replace_pending(&mut self, snapshot: ReaderCheckpoint) -> Result<()> {
        let revision = self.revision.checked_add(1).context("Reader checkpoint revision is exhausted")?;
        if let Some(pending) = self.pending.as_ref() {
            pending.revoked.store(true, Ordering::Release);
        }
        self.revision = revision;
        self.pending = Some(PendingCheckpoint {
            revision, snapshot, revoked: Arc::new(AtomicBool::new(false)),
        });
        Ok(())
    }

    fn start_worker<F>(&mut self, handle: &Handle, persist: F)
    where F: FnOnce(&ReaderCheckpoint, &AtomicBool) -> Result<()> + Send + 'static {
        assert!(self.in_flight.is_none(), "A reader checkpoint worker already owns the slot");
        let pending = self.pending.as_ref().expect("The worker needs a pending checkpoint").clone();
        let revision = pending.revision;
        let revoked = pending.revoked.clone();
        let (sender, completion) = mpsc::sync_channel(1);
        // SQLite metadata has one dedicated, bounded slot. The completion sender
        // and Store stay in the actual closure, independently of App job epochs.
        let task = handle.spawn_blocking(move || {
            let result = if pending.revoked.load(Ordering::Acquire) {
                Err(anyhow!("Reader checkpoint was superseded"))
            } else {
                persist(&pending.snapshot, &pending.revoked)
            };
            let _ = sender.try_send(result);
        });
        self.in_flight = Some(InFlight { revision, revoked, completion, task });
    }

    fn collect_finished(&mut self, now: Instant) -> Result<()> {
        if self.in_flight.as_ref().is_none_or(|worker| !worker.task.is_finished()) { return Ok(()); }
        let worker = self.in_flight.take().expect("The finished worker owns the slot");
        let result = match worker.completion.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Disconnected) => Err(anyhow!("The reader checkpoint worker stopped without a result")),
            Err(TryRecvError::Empty) => {
                self.in_flight = Some(worker);
                return Ok(());
            }
        };
        if result.is_ok() { self.completed_writes = self.completed_writes.saturating_add(1); }
        let current = !worker.revoked.load(Ordering::Acquire)
            && self.pending.as_ref().is_some_and(|pending| pending.revision == worker.revision);
        if !current { return Ok(()); }
        match result {
            Ok(()) => {
                self.pending = None;
                self.clear_retry();
                Ok(())
            }
            Err(error) => {
                self.failed(now);
                Err(error.context("Persist the presented reading position"))
            }
        }
    }

    fn failed(&mut self, now: Instant) {
        let delay = RETRY_INITIAL.saturating_mul(1_u32 << self.failure_streak.min(5)).min(RETRY_MAXIMUM);
        self.failure_streak = self.failure_streak.saturating_add(1);
        self.failed_attempts = self.failed_attempts.saturating_add(1);
        self.retry_at = Some(now + delay);
    }

    fn clear_retry(&mut self) {
        self.failure_streak = 0;
        self.retry_at = None;
    }
}

impl Drop for ReaderCheckpointWriter {
    fn drop(&mut self) { self.retire(); }
}

#[cfg(test)]
#[path = "reader_checkpoint_tests.rs"]
mod tests;

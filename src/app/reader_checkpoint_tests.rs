use super::*;

use std::{sync::{Mutex, mpsc::SyncSender}, thread};

use rusqlite::Connection;
use tokio::runtime::{Builder, Runtime};

fn runtime() -> Result<Runtime> {
    Ok(Builder::new_multi_thread().worker_threads(1).max_blocking_threads(2).enable_time().build()?)
}

fn checkpoint(page: usize) -> ReaderCheckpoint {
    ReaderCheckpoint {
        account: "checkpoint-account".into(), epoch: 3, comic_id: "comic".into(),
        episode_id: "episode".into(), demo_strip: false,
        anchor: ReadingAnchor {
            episode_id: "episode".into(), revision: "revision".into(),
            page_id: format!("page-{page}"), page_index: page, source_x: 0.5,
            source_y: 0.5, zoom_ratio: 1.0, updated_at: 42, ..Default::default()
        },
    }
}

fn wait_worker(writer: &ReaderCheckpointWriter) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while writer.in_flight.as_ref().is_some_and(|worker| !worker.task.is_finished()) {
        ensure!(Instant::now() < deadline, "The controlled checkpoint worker did not finish");
        thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

fn drain(writer: &mut ReaderCheckpointWriter, handle: &Handle, database: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        wait_worker(writer)?;
        writer.drive(handle, database, Instant::now())?;
        if writer.pending.is_none() && writer.in_flight.is_none() { return Ok(()); }
        ensure!(Instant::now() < deadline, "Reader checkpoint work did not drain");
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn rapid_positions_coalesce_behind_one_actual_worker() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("state.sqlite3"))?;
    let worker_store = store.reopen()?;
    let runtime = runtime()?;
    let mut writer = ReaderCheckpointWriter::default();
    writer.submit_latest(checkpoint(1))?;
    let (entered, observed) = mpsc::sync_channel(1);
    let (release, blocked) = mpsc::sync_channel(1);
    writer.start_worker(runtime.handle(), move |snapshot, revoked| {
        entered.send(()).context("Report the held checkpoint worker")?;
        blocked.recv_timeout(Duration::from_secs(5)).context("Release the held checkpoint worker")?;
        snapshot.persist(&worker_store, revoked)
    });
    observed.recv_timeout(Duration::from_secs(2))?;
    let worker_id = writer.in_flight.as_ref().unwrap().task.id();
    for page in 2..=256 {
        writer.submit_latest(checkpoint(page))?;
        writer.drive(runtime.handle(), store.path(), Instant::now())?;
        assert_eq!(writer.in_flight.as_ref().unwrap().task.id(), worker_id);
    }
    assert_eq!(writer.pending_checkpoint().unwrap().anchor.page_index, 256);
    assert!(writer.diagnostics().pending && writer.diagnostics().in_flight);
    release.send(())?;
    drain(&mut writer, runtime.handle(), store.path())?;
    assert_eq!(store.load_anchor("checkpoint-account", "comic")?, Some(checkpoint(256).anchor));
    assert_eq!(writer.diagnostics().completed_writes, 1);
    assert_eq!(writer.diagnostics().failed_attempts, 0);
    Ok(())
}

#[test]
fn retirement_keeps_the_actual_worker_slot_until_its_closure_returns() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("state.sqlite3"))?;
    let worker_store = store.reopen()?;
    let runtime = runtime()?;
    let mut writer = ReaderCheckpointWriter::default();
    writer.submit_latest(checkpoint(1))?;
    let (entered, observed) = mpsc::sync_channel(1);
    let (release, blocked) = mpsc::sync_channel(1);
    writer.start_worker(runtime.handle(), move |snapshot, revoked| {
        entered.send(())?;
        blocked.recv_timeout(Duration::from_secs(5))?;
        snapshot.persist(&worker_store, revoked)
    });
    observed.recv_timeout(Duration::from_secs(2))?;
    let worker_id = writer.in_flight.as_ref().unwrap().task.id();
    writer.retire();
    assert!(writer.pending_checkpoint().is_none());
    assert!(writer.diagnostics().in_flight);
    let mut replacement = checkpoint(8);
    replacement.account = "replacement-account".into();
    replacement.epoch += 1;
    writer.submit_latest(replacement.clone())?;
    writer.drive(runtime.handle(), store.path(), Instant::now())?;
    assert_eq!(writer.in_flight.as_ref().unwrap().task.id(), worker_id);
    release.send(())?;
    drain(&mut writer, runtime.handle(), store.path())?;
    assert!(store.load_anchor("checkpoint-account", "comic")?.is_none());
    assert_eq!(store.load_anchor(&replacement.account, &replacement.comic_id)?, Some(replacement.anchor));
    Ok(())
}

static BUSY_SIGNAL: Mutex<Option<SyncSender<()>>> = Mutex::new(None);

fn announce_checkpoint_busy(attempt: i32) -> bool {
    if let Some(signal) = BUSY_SIGNAL.lock().unwrap().take() { let _ = signal.try_send(()); }
    thread::sleep(Duration::from_millis(1));
    attempt < 5_000
}

#[test]
fn a_worker_actually_waiting_on_sqlite_cannot_replace_the_new_boundary_anchor() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("state.sqlite3"))?;
    let worker_store = store.reopen()?;
    worker_store.set_busy_handler_for_test(Some(announce_checkpoint_busy))?;
    let runtime = runtime()?;
    let mut writer = ReaderCheckpointWriter::default();
    writer.submit_latest(checkpoint(21))?;
    let revoked = writer.pending.as_ref().unwrap().revoked.clone();
    let database = store.path().to_owned();
    let (locked, lock_observed) = mpsc::sync_channel(1);
    let lock_holder = thread::spawn(move || -> Result<()> {
        let connection = Connection::open(database)?;
        connection.execute_batch("BEGIN IMMEDIATE;")?;
        locked.send(())?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !revoked.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        connection.execute_batch("ROLLBACK;")?;
        ensure!(revoked.load(Ordering::Acquire), "The boundary did not revoke the held worker");
        Ok(())
    });
    lock_observed.recv_timeout(Duration::from_secs(2))?;
    let (busy, busy_observed) = mpsc::sync_channel(1);
    *BUSY_SIGNAL.lock().unwrap() = Some(busy);
    writer.start_worker(runtime.handle(), move |snapshot, revoked| snapshot.persist(&worker_store, revoked));
    let waiting = busy_observed.recv_timeout(Duration::from_secs(2));
    // Both anchors deliberately have the same timestamp; the newer user action
    // goes backwards. Ordering must come from the fence, not page/time maxima.
    let latest = checkpoint(19);
    let checkpointed = writer.flush(&store, Some(latest.clone()));
    let unlocked = lock_holder.join().map_err(|_| anyhow!("The controlled SQLite lock holder panicked"))?;
    waiting.context("Observe the actual SQLite busy callback")?;
    unlocked?;
    checkpointed?;
    drain(&mut writer, runtime.handle(), store.path())?;
    assert_eq!(store.load_anchor(&latest.account, &latest.comic_id)?, Some(latest.anchor));
    assert_eq!(writer.diagnostics().completed_writes, 1);
    assert_eq!(writer.diagnostics().failed_attempts, 0);
    Ok(())
}

#[test]
fn a_real_write_failure_keeps_the_latest_snapshot_and_retries_with_backoff() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("state.sqlite3"))?;
    let control = Connection::open(store.path())?;
    control.execute_batch("CREATE TRIGGER reject_reader_checkpoint BEFORE INSERT ON reading_anchors
        BEGIN SELECT RAISE(FAIL,'controlled_reader_checkpoint_failure'); END;")?;
    let runtime = runtime()?;
    let mut writer = ReaderCheckpointWriter::default();
    let now = Instant::now();
    writer.submit_latest(checkpoint(2))?;
    writer.drive(runtime.handle(), store.path(), now)?;
    wait_worker(&writer)?;
    assert!(writer.drive(runtime.handle(), store.path(), now).is_err());
    assert!(writer.pending_checkpoint().is_some());
    assert_eq!(writer.diagnostics().failed_attempts, 1);
    assert!(store.load_anchor("checkpoint-account", "comic")?.is_none());
    writer.submit_latest(checkpoint(4))?;
    for _ in 0..10 {
        writer.drive(runtime.handle(), store.path(), now + Duration::from_millis(999))?;
        assert!(!writer.diagnostics().in_flight);
    }
    control.execute_batch("DROP TRIGGER reject_reader_checkpoint;")?;
    writer.drive(runtime.handle(), store.path(), now + RETRY_INITIAL)?;
    wait_worker(&writer)?;
    writer.drive(runtime.handle(), store.path(), now + RETRY_INITIAL)?;
    assert!(!writer.diagnostics().pending && !writer.diagnostics().in_flight);
    assert_eq!(writer.diagnostics().failed_attempts, 1);
    assert_eq!(writer.diagnostics().retries, 1);
    assert_eq!(writer.diagnostics().completed_writes, 1);
    assert_eq!(writer.failure_streak, 0);
    assert_eq!(store.load_anchor("checkpoint-account", "comic")?, Some(checkpoint(4).anchor));
    Ok(())
}

#[test]
fn failed_boundary_state_remains_available_after_the_reader_is_gone() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("state.sqlite3"))?;
    let control = Connection::open(store.path())?;
    control.execute_batch("CREATE TRIGGER reject_reader_boundary BEFORE INSERT ON reading_anchors
        BEGIN SELECT RAISE(FAIL,'controlled_reader_boundary_failure'); END;")?;
    let mut writer = ReaderCheckpointWriter::default();
    assert!(writer.flush(&store, Some(checkpoint(7))).is_err());
    assert_eq!(writer.pending_checkpoint(), Some(&checkpoint(7)));
    assert_eq!(writer.diagnostics().failed_attempts, 1);
    let mut other_scope = checkpoint(1);
    other_scope.episode_id = "other-episode".into();
    assert!(writer.flush(&store, Some(other_scope)).is_err());
    assert_eq!(writer.pending_checkpoint(), Some(&checkpoint(7)));
    control.execute_batch("DROP TRIGGER reject_reader_boundary;")?;
    // A closed reader supplies no current snapshot. Its lightweight checkpoint
    // must still be flushed at sleep, quit, or the next reader boundary.
    writer.flush(&store, None)?;
    assert!(writer.pending_checkpoint().is_none());
    assert_eq!(store.load_anchor("checkpoint-account", "comic")?, Some(checkpoint(7).anchor));
    assert_eq!(writer.diagnostics().failed_attempts, 1);
    assert_eq!(writer.diagnostics().retries, 1);
    Ok(())
}

#[test]
fn an_old_success_completion_does_not_clear_a_new_failed_boundary_snapshot() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("state.sqlite3"))?;
    let control = Connection::open(store.path())?;
    let runtime = runtime()?;
    let mut writer = ReaderCheckpointWriter::default();
    writer.submit_latest(checkpoint(3))?;
    writer.drive(runtime.handle(), store.path(), Instant::now())?;
    wait_worker(&writer)?;
    // Keep the successful completion undrained while a newer boundary fails.
    control.execute_batch("CREATE TRIGGER reject_new_boundary BEFORE INSERT ON reading_anchors
        BEGIN SELECT RAISE(FAIL,'controlled_new_boundary_failure'); END;")?;
    assert!(writer.flush(&store, Some(checkpoint(4))).is_err());
    writer.drive(runtime.handle(), store.path(), Instant::now())?;
    assert_eq!(writer.pending_checkpoint(), Some(&checkpoint(4)));
    assert_eq!(writer.diagnostics().failed_attempts, 1);
    assert!(!writer.diagnostics().in_flight);
    assert_eq!(store.load_anchor("checkpoint-account", "comic")?, Some(checkpoint(3).anchor));
    control.execute_batch("DROP TRIGGER reject_new_boundary;")?;
    writer.flush(&store, None)?;
    assert_eq!(store.load_anchor("checkpoint-account", "comic")?, Some(checkpoint(4).anchor));
    Ok(())
}

#[test]
fn a_panicked_worker_releases_its_finished_slot_and_keeps_a_retryable_snapshot() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("state.sqlite3"))?;
    let runtime = runtime()?;
    let mut writer = ReaderCheckpointWriter::default();
    writer.submit_latest(checkpoint(6))?;
    writer.start_worker(runtime.handle(), |_, _| panic!("controlled_checkpoint_worker_failure"));
    wait_worker(&writer)?;
    let now = Instant::now();
    assert!(writer.drive(runtime.handle(), store.path(), now).is_err());
    assert!(!writer.diagnostics().in_flight);
    assert_eq!(writer.pending_checkpoint(), Some(&checkpoint(6)));
    writer.drive(runtime.handle(), store.path(), now + RETRY_INITIAL)?;
    wait_worker(&writer)?;
    writer.drive(runtime.handle(), store.path(), now + RETRY_INITIAL)?;
    assert!(!writer.diagnostics().pending && !writer.diagnostics().in_flight);
    assert_eq!(writer.diagnostics().failed_attempts, 1);
    assert_eq!(writer.diagnostics().retries, 1);
    assert_eq!(store.load_anchor("checkpoint-account", "comic")?, Some(checkpoint(6).anchor));
    Ok(())
}

#[test]
fn unflushed_scope_changes_preserve_the_old_account_episode_and_position() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("state.sqlite3"))?;
    let mut writer = ReaderCheckpointWriter::default();
    let original = checkpoint(3);
    writer.submit_latest(original.clone())?;
    let mut variants = Vec::new();
    let mut changed = original.clone(); changed.account = "other-account".into(); variants.push(changed);
    let mut changed = original.clone(); changed.epoch += 1; variants.push(changed);
    let mut changed = original.clone(); changed.comic_id = "other-comic".into(); variants.push(changed);
    let mut changed = original.clone(); changed.episode_id = "other-episode".into(); variants.push(changed);
    let mut changed = original.clone(); changed.demo_strip = true; variants.push(changed);
    for changed in &variants {
        assert!(writer.submit_latest(changed.clone()).is_err());
        assert_eq!(writer.pending_checkpoint(), Some(&original));
    }
    writer.flush(&store, None)?;
    let replacement = variants.remove(0);
    writer.flush(&store, Some(replacement.clone()))?;
    assert_eq!(store.load_anchor(&original.account, &original.comic_id)?, Some(original.anchor));
    assert_eq!(store.load_anchor(&replacement.account, &replacement.comic_id)?, Some(replacement.anchor));
    Ok(())
}

#[test]
fn strip_checkpoints_use_the_original_episode_preference_key() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("state.sqlite3"))?;
    let mut writer = ReaderCheckpointWriter::default();
    let mut snapshot = checkpoint(0);
    snapshot.demo_strip = true;
    snapshot.anchor.episode_id = "episode:design-strip".into();
    snapshot.anchor.source_y = 0.7;
    writer.flush(&store, Some(snapshot.clone()))?;
    assert_eq!(store.preference::<ReadingAnchor>(&snapshot.account, "reader_strip_anchor_episode")?, Some(snapshot.anchor));
    assert!(store.preference::<ReadingAnchor>(&snapshot.account, "reader_strip_anchor_episode:design-strip")?.is_none());
    assert!(store.load_anchor(&snapshot.account, &snapshot.comic_id)?.is_none());
    Ok(())
}

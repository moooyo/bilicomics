use super::*;

use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}, mpsc};

const ACCOUNT: &str = "sleep-checkpoint-fixture";

fn anchor(page: &str, time: i64) -> ReadingAnchor {
    ReadingAnchor { episode_id: "episode".into(), revision: "revision".into(), page_id: page.into(),
        zoom_ratio: 1.0, updated_at: time, ..Default::default() }
}

fn task(id: &str, generation: u64) -> DownloadRecord {
    DownloadRecord { id: id.into(), comic_id: "comic".into(), episode_id: "episode".into(),
        revision: "revision".into(), status: DownloadStatus::Running, generation, retained: true, ..Default::default() }
}

#[test]
fn revoked_sleep_snapshots_never_replace_a_new_live_anchor_or_preferences() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("state.sqlite3"))?;
    let live = anchor("live-page", 20);
    store.save_anchor(ACCOUNT, "comic", &live)?;
    store.set_preference(ACCOUNT, "preferences", &serde_json::json!({"mode":"strip"}))?;
    let revoked = AtomicBool::new(true);
    assert!(store.write_sleep_checkpoint(ACCOUNT, Some(("comic", &anchor("old-page", 10))),
        &[("preferences".into(), serde_json::json!({"mode":"page"}))], &[], 10, &revoked).is_err());
    assert_eq!(store.load_anchor(ACCOUNT, "comic")?, Some(live));
    assert_eq!(store.preference::<serde_json::Value>(ACCOUNT, "preferences")?, Some(serde_json::json!({"mode":"strip"})));
    Ok(())
}

static BUSY_SIGNAL: Mutex<Option<mpsc::Sender<()>>> = Mutex::new(None);

fn announce_checkpoint_lock_wait(attempt: i32) -> bool {
    if let Some(sender) = BUSY_SIGNAL.lock().unwrap().take() { let _ = sender.send(()); }
    std::thread::sleep(Duration::from_millis(1));
    attempt < 2_000
}

#[test]
fn resume_revocation_while_begin_waits_for_sqlite_lock_prevents_the_late_commit() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("state.sqlite3");
    let store = Store::open(&database)?;
    store.save_anchor(ACCOUNT, "comic", &anchor("live-page", 20))?;
    let background = Store::open(&database)?;
    background.connection.busy_handler(Some(announce_checkpoint_lock_wait))?;
    let lock = Transaction::new_unchecked(&store.connection, TransactionBehavior::Immediate)?;
    let (sender, receiver) = mpsc::channel();
    *BUSY_SIGNAL.lock().unwrap() = Some(sender);
    let revoked = Arc::new(AtomicBool::new(false));
    let background_revocation = revoked.clone();
    let thread = std::thread::spawn(move || background.write_sleep_checkpoint(ACCOUNT,
        Some(("comic", &anchor("old-page", 10))), &[], &[], 10, &background_revocation));
    let waiting = receiver.recv_timeout(Duration::from_secs(2));
    revoked.store(true, Ordering::Release);
    lock.rollback()?;
    let outcome = thread.join().map_err(|_| anyhow::anyhow!("The controlled checkpoint worker panicked"))?;
    waiting.context("Observe the actual SQLite busy callback")?;
    assert!(outcome.is_err());
    assert_eq!(store.load_anchor(ACCOUNT, "comic")?.unwrap().page_id, "live-page");
    Ok(())
}

#[test]
fn a_failed_batch_rolls_back_its_anchor_before_any_pause_is_committed() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("state.sqlite3"))?;
    let original = anchor("live-page", 20);
    store.save_anchor(ACCOUNT, "comic", &original)?;
    let running = task("running", 2);
    store.save_download(ACCOUNT, &running)?;
    store.connection.execute_batch("CREATE TRIGGER reject_sleep_preference BEFORE INSERT ON preferences
        WHEN NEW.key='preferences' BEGIN SELECT RAISE(FAIL,'controlled_sleep_checkpoint_failure'); END;")?;
    assert!(store.write_sleep_checkpoint(ACCOUNT, Some(("comic", &anchor("old-page", 10))),
        &[("preferences".into(), serde_json::json!({"mode":"page"}))], &[running], 10, &AtomicBool::new(false)).is_err());
    assert_eq!(store.load_anchor(ACCOUNT, "comic")?, Some(original));
    assert_eq!(store.load_download(ACCOUNT, "running")?.unwrap().status, DownloadStatus::Running);
    assert!(store.preference::<serde_json::Value>(ACCOUNT, "preferences")?.is_none());
    Ok(())
}

#[test]
fn pause_checkpoints_use_exact_task_generation_and_preserve_newer_user_control() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("state.sqlite3"))?;
    let old = task("same-task", 2);
    let current = task("same-task", 3);
    store.save_download(ACCOUNT, &current)?;
    store.write_sleep_checkpoint(ACCOUNT, None, &[], &[old], 10, &AtomicBool::new(false))?;
    assert_eq!(store.load_download(ACCOUNT, "same-task")?.unwrap(), current);
    store.write_sleep_checkpoint(ACCOUNT, None, &[], &[current], 20, &AtomicBool::new(false))?;
    let paused = store.load_download(ACCOUNT, "same-task")?.unwrap();
    assert_eq!(paused.status, DownloadStatus::Paused);
    assert_eq!(paused.generation, 4);
    assert_eq!(paused.terminal_event, Some(serde_json::json!({"kind":"paused"})));
    Ok(())
}

#[test]
fn active_pause_scan_is_account_scoped_and_remains_bounded_as_rows_leave_the_scan() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("state.sqlite3"))?;
    for index in 0..130 { store.save_download(ACCOUNT, &task(&format!("task-{index:04}"), 1))?; }
    store.save_download("foreign-account", &task("foreign", 1))?;
    let mut after = None::<String>;
    let mut sizes = Vec::new();
    loop {
        let rows = store.active_downloads_page(ACCOUNT, after.as_deref(), 10_000)?;
        if rows.is_empty() { break; }
        assert!(rows.len() <= 64);
        after = rows.last().map(|record| record.id.clone());
        sizes.push(rows.len());
        store.write_sleep_checkpoint(ACCOUNT, None, &[], &rows, 10, &AtomicBool::new(false))?;
    }
    assert_eq!(sizes, [64, 64, 2]);
    assert!(!store.download_activity(ACCOUNT)?.0);
    assert!(store.download_activity("foreign-account")?.0);
    Ok(())
}

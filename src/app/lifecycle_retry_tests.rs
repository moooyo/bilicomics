use super::*;

use rusqlite::{Connection, params};

struct DurableFixture {
    database: Connection,
    calls: Vec<LocalWrite>,
}

impl DurableFixture {
    fn new() -> Result<Self> {
        let database = Connection::open_in_memory()?;
        database.execute_batch(
            "CREATE TABLE local_state (key TEXT PRIMARY KEY, payload TEXT NOT NULL);
             CREATE TABLE failed_writes (key TEXT PRIMARY KEY);
             CREATE TRIGGER reject_controlled_write BEFORE INSERT ON local_state
             WHEN EXISTS(SELECT 1 FROM failed_writes WHERE key=NEW.key)
             BEGIN SELECT RAISE(FAIL,'controlled_local_checkpoint_failure'); END;",
        )?;
        Ok(Self { database, calls: Vec::new() })
    }

    fn fail(&self, write: LocalWrite, failed: bool) -> Result<()> {
        if failed {
            self.database.execute("INSERT OR IGNORE INTO failed_writes VALUES(?1)", [write.error()])?;
        } else {
            self.database.execute("DELETE FROM failed_writes WHERE key=?1", [write.error()])?;
        }
        Ok(())
    }

    fn persist(&mut self, write: LocalWrite) -> Result<()> {
        self.calls.push(write);
        let payload = match write {
            LocalWrite::Reader => "retained-page-17-anchor",
            LocalWrite::Preferences => "retained-reader-preferences",
            LocalWrite::Downloads => "paused-download-generation-2",
            LocalWrite::Receipts => "{\"state\":\"unknown\",\"attempts\":1}",
            LocalWrite::Metadata => "checkpoint-complete",
        };
        self.database.execute(
            "INSERT INTO local_state VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET payload=excluded.payload",
            params![write.error(), payload],
        )?;
        if write == LocalWrite::Metadata { self.database.execute_batch("PRAGMA wal_checkpoint(PASSIVE)")?; }
        Ok(())
    }

    fn value(&self, write: LocalWrite) -> Result<String> {
        Ok(self.database.query_row("SELECT payload FROM local_state WHERE key=?1", [write.error()], |row| row.get(0))?)
    }
}

fn drive_due(retry: &mut LocalRetry, database: &mut DurableFixture, now: Instant) -> Option<LocalAttempt> {
    let writes = retry.due(now)?;
    let attempt = attempt_local_writes(writes, |write| database.persist(write));
    retry.completed(attempt.failed, now);
    Some(attempt)
}

#[test]
fn a_clean_sleep_and_durable_unknown_transaction_do_no_periodic_writes() -> Result<()> {
    let now = Instant::now();
    let mut database = DurableFixture::new()?;
    let mut retry = LocalRetry::default();
    let first = attempt_local_writes(LocalWrites::all(), |write| database.persist(write));
    assert!(first.failed.is_empty());
    retry.completed(first.failed, now);
    let saved_calls = database.calls.clone();
    for tick in 1..=600 {
        assert!(drive_due(&mut retry, &mut database, now + Duration::from_secs(tick)).is_none());
    }
    assert_eq!(database.calls, saved_calls);
    assert_eq!(database.value(LocalWrite::Receipts)?, "{\"state\":\"unknown\",\"attempts\":1}");
    assert_eq!(database.value(LocalWrite::Reader)?, "retained-page-17-anchor");
    Ok(())
}

#[test]
fn a_late_observed_receipt_reopens_only_local_receipt_persistence() -> Result<()> {
    let now = Instant::now();
    let mut database = DurableFixture::new()?;
    let mut retry = LocalRetry::default();
    assert!(retry.due(now).is_none());
    retry.request(LocalWrites::one(LocalWrite::Receipts), now);
    assert!(drive_due(&mut retry, &mut database, now).unwrap().failed.is_empty());
    assert_eq!(database.calls, [LocalWrite::Receipts, LocalWrite::Metadata]);
    assert_eq!(database.value(LocalWrite::Receipts)?, "{\"state\":\"unknown\",\"attempts\":1}");
    assert!(drive_due(&mut retry, &mut database, now + Duration::from_secs(300)).is_none());
    Ok(())
}

#[test]
fn real_sqlite_failures_keep_only_failed_work_and_stop_after_recovery() -> Result<()> {
    let now = Instant::now();
    let mut database = DurableFixture::new()?;
    database.fail(LocalWrite::Preferences, true)?;
    database.fail(LocalWrite::Receipts, true)?;
    let mut retry = LocalRetry::default();
    retry.request(LocalWrites::all(), now);
    let first = drive_due(&mut retry, &mut database, now).unwrap();
    let expected = LocalWrites(LocalWrite::Preferences.bit() | LocalWrite::Receipts.bit());
    assert_eq!(first.failed, expected);
    assert_eq!(retry.pending, expected);
    assert_eq!(database.value(LocalWrite::Reader)?, "retained-page-17-anchor");
    assert_eq!(database.value(LocalWrite::Downloads)?, "paused-download-generation-2");
    assert!(drive_due(&mut retry, &mut database, now + Duration::from_millis(999)).is_none());

    database.fail(LocalWrite::Preferences, false)?;
    let second = drive_due(&mut retry, &mut database, now + Duration::from_secs(1)).unwrap();
    assert_eq!(second.failed, LocalWrites::one(LocalWrite::Receipts));
    assert_eq!(&database.calls[5..], [LocalWrite::Preferences, LocalWrite::Receipts, LocalWrite::Metadata]);
    assert!(drive_due(&mut retry, &mut database, now + Duration::from_millis(2999)).is_none());

    database.fail(LocalWrite::Receipts, false)?;
    let third = drive_due(&mut retry, &mut database, now + Duration::from_secs(3)).unwrap();
    assert!(third.failed.is_empty());
    let saved_calls = database.calls.clone();
    assert!(drive_due(&mut retry, &mut database, now + Duration::from_secs(1000)).is_none());
    assert_eq!(database.calls, saved_calls);
    assert_eq!(database.value(LocalWrite::Receipts)?, "{\"state\":\"unknown\",\"attempts\":1}");
    Ok(())
}

#[test]
fn repeated_storage_errors_back_off_from_one_second_to_the_thirty_second_cap() -> Result<()> {
    let mut now = Instant::now();
    let mut database = DurableFixture::new()?;
    database.fail(LocalWrite::Metadata, true)?;
    let mut retry = LocalRetry::default();
    retry.request(LocalWrites::one(LocalWrite::Metadata), now);
    assert!(drive_due(&mut retry, &mut database, now).unwrap().result().is_err());
    for delay in [1, 2, 4, 8, 16, 30, 30, 30] {
        let next = now + Duration::from_secs(delay);
        let count = database.calls.len();
        assert!(drive_due(&mut retry, &mut database, next - Duration::from_millis(1)).is_none());
        assert_eq!(database.calls.len(), count);
        assert_eq!(retry.retry_at, Some(next));
        assert!(drive_due(&mut retry, &mut database, next).unwrap().result().is_err());
        assert_eq!(database.calls.len(), count + 1);
        now = next;
    }
    Ok(())
}

#[test]
fn repeated_pending_notifications_do_not_bypass_a_failed_receipt_backoff() -> Result<()> {
    let now = Instant::now();
    let mut database = DurableFixture::new()?;
    database.fail(LocalWrite::Receipts, true)?;
    let mut retry = LocalRetry::default();
    retry.request(LocalWrites::one(LocalWrite::Receipts), now);
    drive_due(&mut retry, &mut database, now).unwrap();
    let deadline = retry.retry_at;
    for millis in (1..1000).step_by(17) {
        let tick = now + Duration::from_millis(millis);
        retry.request(LocalWrites::one(LocalWrite::Receipts), tick);
        assert_eq!(retry.retry_at, deadline);
        assert!(drive_due(&mut retry, &mut database, tick).is_none());
    }
    assert_eq!(database.calls, [LocalWrite::Receipts, LocalWrite::Metadata]);
    Ok(())
}

#[test]
fn a_new_receipt_does_not_wait_for_an_unrelated_long_metadata_retry() -> Result<()> {
    let now = Instant::now();
    let mut retry = LocalRetry::default();
    retry.pending = LocalWrites::one(LocalWrite::Metadata);
    retry.retry_at = Some(now + LOCAL_RETRY_MAXIMUM);
    retry.backoff_step = 5;
    retry.request(LocalWrites::one(LocalWrite::Receipts), now);
    assert!(retry.due(now).unwrap().contains(LocalWrite::Receipts));
    assert!(retry.due(now).unwrap().contains(LocalWrite::Metadata));
    assert_eq!(retry.backoff_step, 0);
    Ok(())
}

#[test]
fn a_boundary_checkpoint_bypasses_the_sleep_retry_deadline_and_retains_the_live_anchor() -> Result<()> {
    let now = Instant::now();
    let mut database = DurableFixture::new()?;
    let mut retry = LocalRetry::default();
    retry.completed(LocalWrites::one(LocalWrite::Reader), now);
    assert!(retry.due(now).is_none());
    // Suspend and resume use this immediate path rather than waiting on due().
    let forced = attempt_local_writes(LocalWrites::all(), |write| database.persist(write));
    assert!(forced.failed.is_empty());
    retry.completed(forced.failed, now);
    assert!(retry.pending.is_empty());
    assert_eq!(database.calls, LocalWrite::ORDER);
    assert_eq!(database.value(LocalWrite::Reader)?, "retained-page-17-anchor");
    Ok(())
}

#[test]
fn retry_time_is_measured_after_the_attempt_and_historical_failure_is_not_pending_work() {
    let began = Instant::now();
    let finished = began + Duration::from_secs(5);
    let mut state = LifecycleState { checkpoint_failed: true, ..Default::default() };
    state.local_retry.completed(LocalWrites::one(LocalWrite::Preferences), finished);
    assert!(state.local_retry.due(began + Duration::from_secs(5)).is_none());
    assert!(state.local_retry.due(began + Duration::from_secs(6)).is_some());
    state.local_retry.completed(LocalWrites::default(), finished + Duration::from_secs(1));
    assert!(state.checkpoint_failed);
    assert!(state.local_retry.due(finished + Duration::from_secs(100)).is_none());
}

#[test]
fn a_failed_boundary_still_attempts_receipts_and_the_final_checkpoint() -> Result<()> {
    let mut database = DurableFixture::new()?;
    database.fail(LocalWrite::Reader, true)?;
    database.fail(LocalWrite::Preferences, true)?;
    let attempt = attempt_local_writes(LocalWrites::all(), |write| database.persist(write));
    assert_eq!(database.calls, LocalWrite::ORDER);
    assert_eq!(database.value(LocalWrite::Receipts)?, "{\"state\":\"unknown\",\"attempts\":1}");
    assert_eq!(database.value(LocalWrite::Metadata)?, "checkpoint-complete");
    assert_eq!(attempt.result().unwrap_err().to_string(),
               "reader_position_checkpoint_failed; preferences_checkpoint_failed");
    Ok(())
}

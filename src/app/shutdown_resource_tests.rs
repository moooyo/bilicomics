//! Shutdown still retires work after independent persistence failures.

use super::*;

#[tokio::test]
async fn a_failed_preference_write_does_not_leave_job_admission_open() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let client = BiliClient::offline(directory.path())?;
    let mut app = test_support::app(directory.path(), DeviceProfile::Paperwhite5,
        client, tokio::runtime::Handle::current())?;
    let connection = rusqlite::Connection::open(app.store.path())?;
    connection.execute_batch(
        "CREATE TRIGGER reject_shutdown_preferences BEFORE INSERT ON preferences
         WHEN NEW.key = 'preferences'
         BEGIN SELECT RAISE(FAIL, 'controlled shutdown preference failure'); END;"
    )?;
    let ticket = app.jobs.begin(app.epoch, 1, "shutdown-held-job", false)?;

    let error = app.shutdown().expect_err("The controlled preference write must fail");
    assert!(error.to_string().contains("preferences"));
    assert!(matches!(app.jobs.begin(app.epoch, 2, "shutdown-late-job", false),
        Err(jobs::CapacityError::Closed)));
    assert!(ticket.acquire_delivery().await.is_none());
    Ok(())
}

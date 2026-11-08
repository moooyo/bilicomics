//! Headless approval-state checks; the connected phase is a fixture input.

use super::*;

#[tokio::test]
async fn completed_network_job_keeps_epoch_approval_after_pending_reclamation_and_rejects_stale_proof() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let client = BiliClient::offline(directory.path())?;
    let mut app = super::super::test_support::app(directory.path(), DeviceProfile::Paperwhite5,
        client, tokio::runtime::Handle::current())?;
    app.source_job("network-check", async { Ok(()) }, |app, result| {
        result.expect("The local network-completion fixture must succeed");
        app.network_verified_epoch = Some(app.epoch);
    });
    super::super::resource_job_tests::tick_until(&mut app,
        |app| app.network_check_approved("connected")).await?;
    assert!(!app.pending.contains_key("network-check"));
    assert!(app.network_check_approved("connected"));
    assert!(!app.network_check_approved("connecting"));
    app.invalidate("network-check");
    assert!(!app.network_check_approved("connected"));
    app.network_verified_epoch = Some(app.epoch);
    app.epoch += 1;
    assert!(!app.network_check_approved("connected"));
    Ok(())
}

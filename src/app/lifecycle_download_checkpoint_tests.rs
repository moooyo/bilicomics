use super::*;

use crate::{app::test_support, platform::DeviceProfile, source::BiliClient};
use std::sync::atomic::AtomicBool;

fn staged_task() -> DownloadRecord {
    DownloadRecord { id: "maintenance-task".into(), comic_id: "comic".into(), episode_id: "episode".into(),
        revision: "revision".into(), status: DownloadStatus::Running, generation: 1,
        retained: true, ..Default::default() }
}

fn prepare(directory: &Path) -> Result<App> {
    let client = BiliClient::offline(directory)?;
    let mut app = test_support::app(directory, DeviceProfile::Paperwhite5,
        client.clone(), tokio::runtime::Handle::current())?;
    app.downloads = Some(DownloadManager::new(directory, app.account.clone(), app.epoch,
        Arc::new(client), tokio::runtime::Handle::current())?);
    app.store.save_download(&app.account, &staged_task())?;
    app.begin_suspend_intent_gate();
    let cycle = app.lifecycle_cycle();
    app.download_ui.sleep = Some(DownloadSleepSnapshot { cycle, account: app.account.clone(), epoch: app.epoch,
        resume_requested: false, tasks: BTreeMap::new(), deferred: Vec::new(), demo_ids: BTreeSet::new() });
    Ok(app)
}

#[tokio::test]
async fn background_paused_rows_receive_controller_confirmation_before_auto_resume() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = prepare(directory.path())?;
    let cycle = app.lifecycle_cycle();
    app.stage_sleep_task_checkpoints(cycle, &[staged_task()])?;
    app.store.write_sleep_checkpoint(&app.account, None, &[], &[staged_task()], 10, &AtomicBool::new(false))?;
    assert!(app.download_ui.sleep.as_ref().unwrap().tasks["maintenance-task"].needs_control);
    app.reconcile_download_controls_after_sleep()?;
    let token = &app.download_ui.sleep.as_ref().unwrap().tasks["maintenance-task"];
    assert!(!token.needs_control);
    assert_eq!(token.paused_generation, 3);
    assert_eq!(app.store.load_download(&app.account, "maintenance-task")?.unwrap().generation, 3);
    assert!(app.download_ui.pending_controls.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_foreground_checkpoint_winning_before_background_commit_does_not_pause_twice() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = prepare(directory.path())?;
    let cycle = app.lifecycle_cycle();
    app.stage_sleep_task_checkpoints(cycle, &[staged_task()])?;
    app.pause_downloads_for_sleep(cycle)?;
    assert!(app.download_ui.pending_controls.is_empty());
    app.reconcile_download_controls_after_sleep()?;
    let token = &app.download_ui.sleep.as_ref().unwrap().tasks["maintenance-task"];
    assert!(!token.needs_control);
    assert_eq!(token.paused_generation, 2);
    assert_eq!(app.store.load_download(&app.account, "maintenance-task")?.unwrap().generation, 2);
    Ok(())
}

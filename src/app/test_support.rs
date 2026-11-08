//! Headless controllers for application-boundary tests on the remote worker.

use super::*;

pub(super) fn app(data_dir: &Path, profile: DeviceProfile, client: BiliClient,
    handle: tokio::runtime::Handle) -> Result<App> {
    let store = Store::open(data_dir.join("state.sqlite3"))?;
    let account = "bili_11".to_owned();
    let catalog = CatalogState::prepare(&store, &account, None)?;
    Ok(App {
        ui: slint::Weak::default(),
        options: AppOptions { data_dir: data_dir.to_owned(), profile, demo: false, offline: false,
            initial_screen: String::new(), landscape: false },
        data_dir: data_dir.to_owned(), platform: Arc::new(Platform::new(profile, false)?), handle,
        store, client, account, epoch: 1, sequence: 0, pending: BTreeMap::new(), jobs: JobMailbox::new(),
        demo: None, preferences: Preferences::default(), catalog, account_ui: AccountState::default(),
        download_ui: DownloadState::default(), reading: None, downloads: None, lifecycle: LifecycleState::default(),
        startup_stage: None, startup_resume_comic: None, startup_account: None, startup_covers: VecDeque::new(),
        route: "detail".into(), previous_route: "shelf".into(), auth_return: "detail".into(),
        pending_network: None, network_verified_epoch: None,
        qr_key: None, qr_next_poll: Instant::now(), qr_polling: false,
        cover_files: BTreeMap::new(), cover_images: BTreeMap::new(), cover_sequence: 0,
        cover_pending: BTreeSet::new(), cover_failed: BTreeSet::new(), cover_state: covers::CoverState::default(),
        toast_until: None, dirty: true, view_dirty: ViewDirty::default(), quitting: false,
        online: true, last_clock: "16:23".into(), last_device_query: Instant::now(),
    })
}

pub(super) fn next_job(app: &App, timeout: Duration) -> Result<JobEvent> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(event) = app.jobs.take() { return Ok(event); }
        anyhow::ensure!(Instant::now() < deadline, "Timed out waiting for an application job");
        std::thread::sleep(Duration::from_millis(5));
    }
}

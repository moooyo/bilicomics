//! Background-owned startup data and bounded UI-thread attachment stages.

use super::*;
use crate::reader::{ReaderCore, ReaderSettings};
use std::io::{Cursor, Read};

const MAX_STARTUP_COVER_BYTES: u64 = 8 * 1024 * 1024;
const STARTUP_COVER_MEMORY: usize = 16 * 1024 * 1024;
const COVER_ATTACH_BUDGET: usize = 4 * 1024 * 1024;

/// No Slint object or callback is constructed on the preparation worker.
/// The metadata connection moves to its single UI-thread owner at attachment.
pub struct PreparedApp {
    options: AppOptions,
    data_dir: PathBuf,
    store: Store,
    client: BiliClient,
    account: String,
    preferences: Preferences,
    demo: Option<Arc<DemoData>>,
    catalog: CatalogState,
    account_ui: PreparedAccount,
    covers: VecDeque<PreparedCover>,
    cover_failed: BTreeSet<String>,
    resume_comic: Option<String>,
}

#[derive(Clone, Copy)]
pub(super) enum AttachStage {
    Covers,
    Catalog,
    Account,
    Downloads,
    Route,
    Views,
}

pub(super) struct PreparedCover {
    comic_id: String,
    path: PathBuf,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl App {
    /// Call on a worker after the platform backend has been installed.
    pub fn prepare(options: AppOptions) -> Result<PreparedApp> {
        let data_dir = if options.demo { options.data_dir.join("demo") } else { options.data_dir.clone() };
        std::fs::create_dir_all(&data_dir).context("Create the application data directory")?;
        let session_file = SessionFile::new(&data_dir.join("account"));
        let session = if options.demo { None } else { session_file.load()? };
        let client = if options.demo { BiliClient::offline(&data_dir)? }
            else { BiliClient::new(session.clone(), &data_dir)?.with_session_file(session_file) };
        let mut store = Store::open(data_dir.join("state.sqlite3"))?;
        let account = if options.demo { "design-demo".to_owned() } else {
            session.as_ref().and_then(AuthSession::account_key).map(str::to_owned)
                .or(store.preference::<String>("device", "last_account")?)
                .unwrap_or_else(|| "guest".to_owned())
        };
        let mut preferences: Preferences = store.preference(&account, "preferences")?.unwrap_or_default();
        if options.demo { preferences.onboarded = true; preferences.reader_guidance_seen = true; }
        let demo = if options.demo { Some(Arc::new(demo::load(&data_dir)?)) } else { None };
        if let Some(demo) = &demo {
            store.save_comics(&account, &demo.comics)?;
            for detail in &demo.details { store.save_detail(&account, detail)?; }
            for index in &demo.indices { store.save_index(&account, index)?; }
            seed_demo_resume(&store, &account, demo)?;
        }
        let mut catalog = CatalogState::prepare(&store, &account, demo.as_deref())?;
        let account_ui = PreparedAccount::load(&store, &account, demo.as_deref())?;
        // The first candidate lookup builds the bundled dictionary. Warm it on
        // the worker rather than during the first catalog model render.
        let _ = crate::input::PinyinInput::default().candidates();
        // Warm the small deferred-intent preference before UI state is built.
        let _: Option<serde_json::Value> = store.preference(&account, "queued_download_intents")?;
        let resume_comic = store.latest_anchor(&account)?.map(|(comic, _)| comic);
        let selected = match options.initial_screen.as_str() {
            "reader" | "spread" | "detail" | "purchase" => catalog.comics.first().map(|comic| comic.id.clone()),
            "" if preferences.launch == "resume" => resume_comic.clone(),
            _ => None,
        };
        if let Some(selected) = selected { catalog.detail = store.load_detail(&account, &selected)?; }
        let (covers, cover_failed) = prepare_covers(&data_dir, &catalog.comics, demo.as_deref());
        Ok(PreparedApp { options, data_dir, store, client, account,
            preferences, demo, catalog, account_ui, covers, cover_failed, resume_comic })
    }

    /// Attach already prepared data without repeating filesystem or seed work.
    /// Keep the startup screen visible while advance_startup yields between steps.
    pub fn attach_prepared(
        ui: &AppWindow,
        prepared: PreparedApp,
        platform: Arc<Platform>,
        handle: tokio::runtime::Handle,
    ) -> Result<Rc<RefCell<Self>>> {
        let PreparedApp { options, data_dir, store, client, account,
            preferences, demo, catalog, account_ui, covers, cover_failed, resume_comic } = prepared;
        let cover_files = demo.as_ref().map(|demo| demo.covers.clone()).unwrap_or_default();
        let jobs = JobMailbox::new();
        let app = Rc::new(RefCell::new(Self {
            ui: ui.as_weak(), options: options.clone(), data_dir,
            platform, handle, store, client, account,
            epoch: 1, sequence: 0, pending: BTreeMap::new(), jobs,
            demo, preferences, catalog, account_ui: AccountState::default(),
            download_ui: DownloadState::default(), reading: None, downloads: None,
            lifecycle: LifecycleState::default(),
            startup_stage: Some(AttachStage::Covers), startup_resume_comic: resume_comic,
            startup_account: Some(account_ui), startup_covers: covers,
            route: "shelf".into(), previous_route: "shelf".into(), auth_return: "account".into(),
            pending_network: None, network_verified_epoch: None,
            qr_key: None, qr_next_poll: Instant::now(), qr_polling: false,
            cover_files, cover_images: BTreeMap::new(), cover_sequence: 0,
            cover_pending: BTreeSet::new(), cover_failed,
            cover_state: covers::CoverState::default(),
            toast_until: None, dirty: true, view_dirty: ViewDirty::default(),
            quitting: false, online: options.demo && !options.offline,
            last_clock: if options.demo { "16:23".into() } else {
                (chrono::Utc::now() + chrono::Duration::hours(8)).format("%H:%M").to_string()
            },
            last_device_query: Instant::now() - Duration::from_secs(60),
        }));
        Self::install_sleep_maintenance(&app)?;
        macro_rules! bind {
            ($model:ty, $section:literal) => {{
                let weak = Rc::downgrade(&app);
                ui.global::<$model>().on_action(move |action, value| {
                    if let Some(app) = weak.upgrade() {
                        let mut app = app.borrow_mut();
                        if let Err(error) = app.dispatch($section, action.as_str(), value.as_str()) {
                            eprintln!("bilicomics: application intent failed: {error:#}");
                            app.toast("common_storage_error");
                        }
                    }
                });
            }};
        }
        bind!(ShellModel, "shell");
        bind!(CatalogModel, "catalog");
        bind!(ReaderModel, "reader");
        bind!(AuthModel, "auth");
        bind!(AccountModel, "account");
        bind!(PurchaseModel, "purchase");
        bind!(DownloadModel, "downloads");
        let weak = Rc::downgrade(&app);
        ui.global::<Theme>().on_refresh(move |kind| {
            if let Some(app) = weak.upgrade() {
                if let Ok(app) = app.try_borrow() { app.refresh_feedback(kind.as_str()); }
            }
        });
        Ok(app)
    }

    /// Compatibility wrapper; production startup uses prepare on its worker.
    pub fn attach(ui: &AppWindow, options: AppOptions, platform: Arc<Platform>, handle: tokio::runtime::Handle) -> Result<Rc<RefCell<Self>>> {
        let prepared = Self::prepare(options)?;
        let app = Self::attach_prepared(ui, prepared, platform, handle)?;
        while !app.borrow_mut().advance_startup()? {}
        ui.global::<ShellModel>().set_booting(false);
        Ok(app)
    }

    pub fn is_starting(&self) -> bool { self.startup_stage.is_some() }

    /// Advance one bounded attachment phase, keeping all UI mutation on this thread.
    /// The caller hides the startup screen only after this returns true.
    pub fn advance_startup(&mut self) -> Result<bool> {
        if self.is_suspended() { return Ok(false); }
        let Some(stage) = self.startup_stage else { return Ok(true); };
        match stage {
            AttachStage::Covers => {
                let mut copied = 0;
                while copied < COVER_ATTACH_BUDGET {
                    let Some(cover) = self.startup_covers.pop_front() else { break; };
                    copied += cover.rgba.len();
                    self.remember_cover(cover.comic_id, cover.path, cover.rgba, cover.width, cover.height);
                }
                if self.startup_covers.is_empty() { self.startup_stage = Some(AttachStage::Catalog); }
            }
            AttachStage::Catalog => {
                if self.demo.is_some() {
                    self.initialize_demo_anchor()?;
                    self.load_store();
                }
                self.startup_stage = Some(AttachStage::Account);
            }
            AttachStage::Account => {
                let prepared = self.startup_account.take().context("Account startup data was already consumed")?;
                self.initialize_account_prepared(prepared)?;
                self.startup_stage = Some(AttachStage::Downloads);
            }
            AttachStage::Downloads => {
                self.initialize_downloads()?;
                self.prepare_download_manager();
                self.startup_stage = Some(AttachStage::Route);
            }
            AttachStage::Route => {
                self.initialize_start_route()?;
                self.startup_stage = Some(AttachStage::Views);
            }
            AttachStage::Views => {
                self.schedule_cover_maintenance(true);
                self.refresh_views()?;
                self.startup_stage = None;
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn prepare_download_manager(&mut self) {
        if self.demo.is_some() { return; }
        let data_dir = self.data_dir.clone();
        let account = self.account.clone();
        let epoch = self.epoch;
        let handle = self.handle.clone();
        let source = match self.download_source() {
            Ok(source) => source,
            Err(error) => { self.source_failure(&error); return; }
        };
        self.background("startup-download-manager", move || {
            DownloadManager::new(&data_dir, account, epoch, Arc::new(source), handle)
        }, |app, result| match result {
            Ok(manager) => { app.downloads = Some(manager); app.recover_downloads(); }
            Err(error) => {
                eprintln!("bilicomics: download manager preparation failed: {error:#}");
                app.toast("common_storage_error");
            }
        });
    }

    fn initialize_start_route(&mut self) -> Result<()> {
        let start = match self.options.initial_screen.as_str() {
            "library" => "shelf", "settings" => "account", "spread" => "reader", value => value,
        }.to_owned();
        if start.is_empty() {
            if !self.preferences.onboarded { self.navigate("welcome"); }
            else if self.preferences.launch == "resume" {
                if let Some(comic) = self.startup_resume_comic.take() { self.open_reader_for_comic(&comic)?; }
                else { self.navigate("shelf"); }
            } else { self.navigate("shelf"); }
        } else if start == "reader" || start == "detail" || start == "purchase" {
            let id = self.demo.as_ref().and_then(|demo| demo.comics.first()).map(|comic| comic.id.clone())
                .or_else(|| self.catalog.comics.first().map(|comic| comic.id.clone()));
            if let Some(id) = id {
                self.open_detail(&id)?;
                if start == "reader" { self.open_reader_for_comic(&id)?; }
                if start == "purchase" { self.open_purchase("2860113")?; }
            } else { self.navigate("shelf"); }
        } else { self.navigate(&start); }
        if self.demo.is_none() && !self.options.offline && self.client.session().is_some() {
            self.check_session();
        }
        Ok(())
    }
}

fn prepare_covers(data_dir: &Path, comics: &[Comic], demo: Option<&DemoData>) -> (VecDeque<PreparedCover>, BTreeSet<String>) {
    let mut prepared = VecDeque::new();
    let mut failed = BTreeSet::new();
    let mut retained_bytes = 0;
    for comic in comics.iter().take(covers::COVER_METADATA_LIMIT) {
        let path = demo.and_then(|demo| demo.covers.get(&comic.id)).cloned()
            .unwrap_or_else(|| covers::cached_cover_path(data_dir, &comic.cover_url));
        let decoded = if demo.is_some() { decode_cover(&path) }
            else { covers::read_cached_cover(data_dir, &comic.cover_url) };
        match decoded {
            Ok(Some((rgba, width, height))) => {
                if retained_bytes + rgba.len() > STARTUP_COVER_MEMORY { break; }
                retained_bytes += rgba.len();
                prepared.push_back(PreparedCover { comic_id: comic.id.clone(), path, rgba, width, height });
            }
            Ok(None) => {}
            Err(_) => { failed.insert(comic.id.clone()); }
        }
    }
    (prepared, failed)
}

pub(super) fn decode_cover(path: &Path) -> Result<Option<(Vec<u8>, u32, u32)>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Inspect a cached startup cover"),
    };
    anyhow::ensure!(metadata.is_file() && !metadata.file_type().is_symlink()
        && metadata.len() <= MAX_STARTUP_COVER_BYTES, "Invalid cached startup cover");
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        anyhow::ensure!(metadata.file_attributes() & 0x400 == 0, "Cached cover reparse points are not allowed");
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut encoded = Vec::new();
    options.open(path)?.take(MAX_STARTUP_COVER_BYTES + 1).read_to_end(&mut encoded)?;
    anyhow::ensure!(encoded.len() as u64 <= MAX_STARTUP_COVER_BYTES, "Cached startup cover exceeds its byte limit");
    decode_cover_bytes(encoded).map(Some)
}

pub(super) fn decode_cover_bytes(encoded: Vec<u8>) -> Result<(Vec<u8>, u32, u32)> {
    anyhow::ensure!(encoded.len() as u64 <= MAX_STARTUP_COVER_BYTES, "Cover data exceeds its byte limit");
    let mut reader = image::ImageReader::new(Cursor::new(encoded)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode()?;
    let rgba = if decoded.width() > 480 || decoded.height() > 640 {
        decoded.thumbnail(480, 640).into_rgba8()
    } else {
        decoded.into_rgba8()
    };
    let (width, height) = rgba.dimensions();
    Ok((rgba.into_raw(), width, height))
}

#[cfg(not(windows))]
pub(super) fn publish_cover(temporary: &Path, target: &Path) -> Result<()> {
    std::fs::rename(temporary, target).context("Publish the cached cover")
}

#[cfg(windows)]
pub(super) fn publish_cover(temporary: &Path, target: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let temporary: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // Both NUL-terminated paths remain alive for the synchronous replacement.
    if unsafe { MoveFileExW(temporary.as_ptr(), target.as_ptr(), 0x1 | 0x8) } == 0 {
        return Err(std::io::Error::last_os_error()).context("Publish the cached cover");
    }
    Ok(())
}

fn seed_demo_resume(store: &Store, account: &str, demo: &DemoData) -> Result<()> {
    let Some(comic) = demo.fixture["continue_reading"]["comic_id"].as_u64().map(|id| id.to_string()) else { return Ok(()); };
    if store.load_anchor(account, &comic)?.is_some() { return Ok(()); }
    let Some(episode) = demo.fixture["continue_reading"]["episode_id"].as_u64().map(|id| id.to_string()) else { return Ok(()); };
    let Some(index) = demo.indices.iter().find(|index| index.episode_id == episode && !index.images.is_empty()) else { return Ok(()); };
    let page = demo.fixture["continue_reading"]["page"].as_u64().unwrap_or(1).saturating_sub(1) as usize;
    let mut core = ReaderCore::new(index.clone(), ReaderSettings::default())?;
    core.set_page(page.min(index.images.len() - 1))?;
    store.save_anchor(account, &comic, &core.anchor(now()))
}

#[cfg(test)]
#[path = "startup_session_tests.rs"]
mod session_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_can_cross_the_worker_boundary_and_preserves_demo_isolation() -> Result<()> {
        fn require_send<T: Send>() {}
        require_send::<PreparedApp>();
        let directory = tempfile::tempdir()?;
        let prepared = App::prepare(AppOptions { data_dir: directory.path().to_owned(),
            profile: DeviceProfile::Paperwhite5, demo: true, offline: true,
            initial_screen: String::new(), landscape: false })?;
        assert_eq!(prepared.data_dir, directory.path().join("demo"));
        assert!(prepared.client.session().is_none());
        assert_eq!(prepared.resume_comic.as_deref(), Some("28601"));
        assert!(prepared.demo.is_some());
        assert!(!directory.path().join("account").exists());
        Ok(())
    }

    #[test]
    fn cached_cover_decoding_rejects_oversized_header_geometry() -> Result<()> {
        let source = image::RgbaImage::from_pixel(4097, 1, image::Rgba([255, 255, 255, 255]));
        let mut encoded = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(source).write_to(&mut encoded, image::ImageFormat::Png)?;
        assert!(decode_cover_bytes(encoded.into_inner()).is_err());
        Ok(())
    }
}

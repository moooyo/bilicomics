//! Background-owned metadata preparation and yielding UI attachment.
//!
//! The preparation/attachment boundary follows the sibling BiliComics startup
//! implementation. No Slint object or Platform is sent to a worker.

use super::*;
use crate::platform::DeviceProfile;

pub struct PreparedApp {
    pub(super) options: AppOptions,
    pub(super) data_dir: PathBuf,
    pub(super) store: Store,
    pub(super) disk_cache: crate::cache::CacheManager,
    pub(super) network_control: crate::sources::NetworkControl,
    pub(super) prefs: Preferences,
    pub(super) current_book_id: Option<String>,
    pub(super) interactions: InteractionState,
    pub(super) data_management: data_management::DataManagementState,
    pub(super) downloads: DownloadWorker,
}

impl PreparedApp {
    /// Cancel an unattached controller without blocking the startup scene.
    pub fn shutdown(&self) {
        self.downloads.shutdown();
    }

    /// Drain it only after the event loop, or on its preparation worker.
    pub fn finish_shutdown(&self, timeout: Duration) -> bool {
        self.downloads.wait_stopped(timeout)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AttachStage {
    Route,
    Catalog,
    View,
    Settings,
    Covers,
    Ready,
}

impl App {
    /// Prepare single-owner metadata and dormant services after backend setup.
    /// The SQLite connection is moved to the UI owner once, never shared.
    pub fn prepare(options: AppOptions, profile: DeviceProfile) -> Result<PreparedApp> {
        let data_dir = if options.demo {
            options.data_dir.join("demo")
        } else {
            options.data_dir.clone()
        };
        fs::create_dir_all(&data_dir).context("Create the application data directory")?;
        let database = data_dir.join("library.sqlite3");
        let store = Store::open(&database)?;
        let disk_cache = crate::cache::CacheManager::open(&data_dir)?;
        let network_control = crate::sources::NetworkControl::open(&data_dir)?;
        // Preparation must also be safe when powerd parks an unfinished boot.
        // The UI opens this gate only while it owns an active display.
        network_control.suspend();
        if options.demo {
            crate::demo::seed_for_screen(
                &store,
                &data_dir,
                normalize_route(&options.initial_screen),
            )?;
        }
        if store.sources()?.is_empty() {
            for source in default_sources() {
                store.save_source(&source)?;
            }
        }
        let mut prefs = store.preferences()?;
        if options.landscape {
            prefs.landscape = true;
            prefs.reverse_landscape = false;
        }
        if !profile.has_frontlight() {
            prefs.brightness = 0;
            prefs.warmth = 0;
        }
        let current_book_id = if options.demo {
            Some(crate::demo::primary_id())
        } else {
            store.library_continuation(None)?
        };
        let interactions = InteractionState::load(&store)?;
        let data_management =
            data_management::DataManagementState::open(&data_dir, prefs.write_logs)?;
        let downloads = DownloadWorker::start_initialized(
            data_dir.clone(),
            database,
            crate::downloads::DownloadOptions {
                offline: options.offline || options.demo,
                ..crate::downloads::DownloadOptions::default()
            },
            true,
        )?;
        crate::reader_timing::mark("application_prepared");
        Ok(PreparedApp {
            options,
            data_dir,
            store,
            disk_cache,
            network_control,
            prefs,
            current_book_id,
            interactions,
            data_management,
            downloads,
        })
    }

    pub fn is_starting(&self) -> bool {
        self.startup_stage.is_some()
    }

    /// Coalesce cheap stages, yielding after a slow stage or blocked lifecycle.
    /// The budget is checked between stages; an individual stage is not preempted.
    pub fn advance_startup(&mut self) -> Result<bool> {
        let started = Instant::now();
        loop {
            let previous = self.startup_stage;
            if self.advance_startup_stage()? {
                return Ok(true);
            }
            if self.startup_stage == previous || started.elapsed() >= Duration::from_millis(4) {
                return Ok(false);
            }
        }
    }

    fn advance_startup_stage(&mut self) -> Result<bool> {
        let Some(stage) = self.startup_stage else {
            return Ok(true);
        };
        if self.suspended || self.shutting_down || self.resume_frame_pending {
            return Ok(false);
        }
        match stage {
            AttachStage::Route => {
                crate::reader_timing::mark("startup_route_started");
                if !self.options.resume {
                    self.network_control.resume();
                }
                if self.options.demo && self.route == "search" {
                    self.query = demo_query();
                    self.run_search()?;
                }
                if !self.options.resume && self.route == "reader" {
                    self.route = "detail".into();
                    self.resume_reader()?;
                }
                if !self.options.resume && self.options.initial_screen == "downloads" {
                    self.route = "settings".into();
                    self.open_downloads()?;
                }
                if self.options.resume {
                    self.restore_saved_session()?;
                }
                self.startup_stage = Some(AttachStage::Catalog);
                crate::reader_timing::mark("startup_route_attached");
            }
            AttachStage::Catalog => {
                self.prepare_demo_catalog_route()?;
                self.startup_stage = Some(AttachStage::View);
                crate::reader_timing::mark("startup_catalog_attached");
            }
            AttachStage::View => {
                crate::reader_timing::mark("startup_view_started");
                self.sync_view()?;
                self.startup_stage = Some(AttachStage::Settings);
                crate::reader_timing::mark("startup_view_attached");
            }
            AttachStage::Settings => {
                if self.route == "settings" {
                    self.refresh_setting_details()?;
                }
                self.startup_stage = Some(AttachStage::Covers);
            }
            AttachStage::Covers => {
                if self.refresh_visible_covers()? {
                    self.sync_view()?;
                }
                self.startup_stage = Some(AttachStage::Ready);
            }
            AttachStage::Ready => {
                if !self.options.resume {
                    self.downloads.resume_owned(true)?;
                }
                self.startup_stage = None;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_metadata_can_move_from_the_worker_to_the_ui_thread() {
        fn require_send<T: Send>() {}
        require_send::<PreparedApp>();
    }

    #[test]
    fn preparation_keeps_download_and_source_work_dormant() {
        let directory = tempfile::tempdir().unwrap();
        let prepared = App::prepare(
            AppOptions {
                data_dir: directory.path().into(),
                demo: false,
                offline: true,
                initial_screen: "library".into(),
                landscape: false,
                resume: false,
            },
            DeviceProfile::Pw5,
        )
        .unwrap();
        assert!(prepared.network_control.is_suspended());
        assert_eq!(prepared.downloads.active_count(), 0);
        prepared.downloads.shutdown();
        assert!(prepared.downloads.wait_stopped(Duration::from_secs(2)));
    }

    #[test]
    fn early_boot_exit_or_sleep_preserves_the_existing_navigation_checkpoint() {
        use crate::app::tests::DemoRouteFixture;

        for suspend in [false, true] {
            let mut fixture = DemoRouteFixture::new("settings", DeviceProfile::Pw5, false);
            let saved = NavigationSession {
                route: "history".into(),
                page: 7,
                ..NavigationSession::default()
            };
            fixture.app.store.save_session(&saved).unwrap();
            let mut saved_preferences = fixture.app.prefs.clone();
            saved_preferences.night_mode = !saved_preferences.night_mode;
            fixture
                .app
                .store
                .save_preferences(&saved_preferences)
                .unwrap();
            fixture.app.startup_stage = Some(AttachStage::Route);
            if suspend {
                fixture.app.prepare_suspend().unwrap();
            }
            fixture.app.shutdown();
            assert_eq!(fixture.app.store.session().unwrap(), Some(saved));
            assert_eq!(
                serde_json::to_value(fixture.app.store.preferences().unwrap()).unwrap(),
                serde_json::to_value(saved_preferences).unwrap(),
            );
        }
    }
}

mod account;
mod auth;
mod catalog;
mod covers;
mod download_ui;
mod jobs;
mod lifecycle;
mod lifecycle_maintenance;
mod reading;
mod startup;
mod views;
#[cfg(test)]
mod test_support;

pub use startup::PreparedApp;

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, VecDeque},
    future::Future,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{
    demo::{self, DemoData},
    domain::{Comic, ComicDetail, Episode},
    downloads::DownloadManager,
    i18n,
    platform::{DeviceProfile, Platform, RefreshReason},
    source::{self, AuthSession, BiliClient, ErrorKind, SessionFile},
    store::Store,
    ui::{AccountModel, AppWindow, AuthModel, CatalogModel, ComicItem, DownloadModel, PurchaseModel, ReaderModel, ShellModel, Theme},
};

use account::{AccountState, PreparedAccount};
use catalog::CatalogState;
use download_ui::DownloadState;
use lifecycle::LifecycleState;
use reading::ReadingState;
use startup::{AttachStage, PreparedCover};
use jobs::{JobEvent, JobMailbox};
use views::{View, ViewDirty};

#[derive(Clone)]
pub struct AppOptions {
    pub data_dir: PathBuf,
    pub profile: DeviceProfile,
    pub demo: bool,
    pub offline: bool,
    pub initial_screen: String,
    pub landscape: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Preferences {
    pub mode: String,
    pub fit: String,
    pub direction: String,
    pub every: String,
    pub landscape: String,
    pub cover_alone: bool,
    pub dither: bool,
    pub footer: bool,
    pub smart: bool,
    pub ahead: u32,
    pub concurrency: u32,
    pub preload: u32,
    pub wifi: String,
    pub cache_limit: u32,
    pub launch: String,
    pub sleep_cover: bool,
    pub flashui: bool,
    pub orientation: String,
    pub onboarded: bool,
    pub reader_guidance_seen: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            mode: "page".into(), fit: "page".into(), direction: "ltr".into(),
            every: "smart".into(), landscape: "spread".into(), cover_alone: true,
            dither: true, footer: true, smart: true, ahead: 2, concurrency: 2,
            preload: 3, wifi: "auto".into(), cache_limit: 512, launch: "resume".into(),
            sleep_cover: false, flashui: true, orientation: "auto".into(),
            onboarded: false, reader_guidance_seen: false,
        }
    }
}

#[cfg(test)]
mod preference_tests {
    use super::*;

    #[test]
    fn retired_inversion_preference_does_not_affect_loading_or_persistence() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path().join("state.sqlite3"))?;
        store.set_preference("bili_1", "preferences", &serde_json::json!({
            "invert": true, "direction": "rtl", "launch": "shelf", "cache_limit": 1024,
        }))?;

        let preferences: Preferences = store.preference("bili_1", "preferences")?.unwrap();
        assert_eq!(preferences.direction, "rtl");
        assert_eq!(preferences.launch, "shelf");
        assert_eq!(preferences.cache_limit, 1024);
        assert_eq!(preferences.mode, Preferences::default().mode);

        store.set_preference("bili_1", "preferences", &preferences)?;
        let saved: serde_json::Value = store.preference("bili_1", "preferences")?.unwrap();
        assert!(saved.get("invert").is_none());
        assert_eq!(saved["direction"], "rtl");
        assert_eq!(saved["launch"], "shelf");
        assert_eq!(saved["cache_limit"], 1024);
        Ok(())
    }
}

struct CachedCover {
    image: slint::Image,
    bytes: usize,
    used: u64,
}

const COVER_IMAGE_BUDGET: usize = 64 * 1024 * 1024;

pub struct App {
    ui: slint::Weak<AppWindow>,
    options: AppOptions,
    data_dir: PathBuf,
    platform: Arc<Platform>,
    handle: tokio::runtime::Handle,
    store: Store,
    client: BiliClient,
    account: String,
    epoch: u64,
    sequence: u64,
    pending: BTreeMap<String, u64>,
    jobs: JobMailbox,
    demo: Option<Arc<DemoData>>,
    preferences: Preferences,
    catalog: CatalogState,
    account_ui: AccountState,
    download_ui: DownloadState,
    reading: Option<ReadingState>,
    downloads: Option<DownloadManager>,
    lifecycle: LifecycleState,
    startup_stage: Option<AttachStage>,
    startup_resume_comic: Option<String>,
    startup_account: Option<PreparedAccount>,
    startup_covers: VecDeque<PreparedCover>,
    route: String,
    previous_route: String,
    auth_return: String,
    pending_network: Option<(String, String, String)>,
    network_verified_epoch: Option<u64>,
    qr_key: Option<String>,
    qr_next_poll: Instant,
    qr_polling: bool,
    cover_files: BTreeMap<String, PathBuf>,
    cover_images: BTreeMap<(String, String), CachedCover>,
    cover_sequence: u64,
    cover_pending: BTreeSet<String>,
    cover_failed: BTreeSet<String>,
    cover_state: covers::CoverState,
    toast_until: Option<Instant>,
    dirty: bool,
    view_dirty: ViewDirty,
    quitting: bool,
    online: bool,
    last_clock: String,
    last_device_query: Instant,
}

impl App {
    pub fn dispatch(&mut self, section: &str, action: &str, value: &str) -> Result<()> {
        if section == "lifecycle" {
            return match action {
                "suspend" => self.suspend(),
                "resume" => self.resume(),
                _ => anyhow::bail!("Unsupported lifecycle action"),
            };
        }
        if self.is_suspended() { return Ok(()); }
        if self.is_starting() {
            if section == "shell" && action == "exit" { self.request_quit(); }
            return Ok(());
        }
        match section {
            "shell" => match action {
                "navigate" => self.navigate(value),
                "back" => self.go_back()?,
                "exit" => self.request_quit(),
                _ => {}
            },
            "catalog" | "library" | "detail" | "search" | "store" => self.catalog_action(action, value)?,
            "reader" => self.reader_action(action, value)?,
            "auth" | "login" => self.auth_action(action, value)?,
            "account" => self.account_action(action, value)?,
            "purchase" => self.purchase_action(action, value)?,
            "downloads" => self.download_action(action, value)?,
            _ => anyhow::bail!("Unsupported application section"),
        }
        self.dirty = true;
        if content_refresh_action(section, action) {
            let _ = self.platform.request_refresh(RefreshReason::Content);
        }
        self.refresh_views()?;
        Ok(())
    }

    pub fn tick(&mut self) -> Result<()> {
        if self.is_suspended() { return self.tick_suspended(); }
        if self.is_starting() {
            self.advance_startup()?;
            return Ok(());
        }
        let mut jobs_applied = false;
        for _ in 0..64 {
            let Some(event) = self.jobs.take() else { break; };
            if event.epoch == self.epoch && self.pending.get(&event.key) == Some(&event.sequence) {
                self.pending.remove(&event.key);
                self.view_dirty.job(&event.key);
                let was_online = self.online;
                (event.apply)(self);
                jobs_applied = true;
                if self.online != was_online { self.view_dirty.mark(View::Reader); }
            }
        }
        let epoch = self.epoch;
        let jobs = &self.jobs;
        self.pending.retain(|key, sequence| jobs.is_current(epoch, *sequence, key));
        self.tick_auth()?;
        self.tick_account()?;
        self.tick_downloads()?;
        if self.toast_until.is_some_and(|until| Instant::now() >= until) {
            if let Some(ui) = self.ui.upgrade() { ui.global::<ShellModel>().set_toast("".into()); }
            self.toast_until = None;
        }
        let clock = if self.demo.is_some() { demo_clock(&self.route).to_owned() } else {
            (chrono::Utc::now() + chrono::Duration::hours(8)).format("%H:%M").to_string()
        };
        if self.last_clock != clock {
            self.last_clock = clock;
            if let Some(ui) = self.ui.upgrade() {
                ui.global::<ShellModel>().set_clock(self.last_clock.clone().into());
                ui.global::<ReaderModel>().set_clock(self.last_clock.clone().into());
            }
        }
        if self.last_device_query.elapsed() >= Duration::from_secs(60) {
            self.last_device_query = Instant::now();
            self.query_device_status();
        }
        if self.cover_retry_due() {
            for view in [View::Catalog, View::Downloads, View::Purchase] { self.view_dirty.mark(view); }
        }
        self.schedule_cover_maintenance(false);
        if jobs_applied || self.dirty || self.view_dirty.needs_route(&self.route) { self.refresh_views()?; }
        Ok(())
    }

    fn refresh_views(&mut self) -> Result<()> {
        let Some(ui) = self.ui.upgrade() else { return Ok(()); };
        let shell = ui.global::<ShellModel>();
        shell.set_suspended(self.is_suspended());
        if self.is_suspended() { return Ok(()); }
        if self.dirty {
            self.view_dirty.all();
            self.dirty = false;
        }
        shell.set_route(self.route.clone().into());
        shell.set_demo(self.demo.is_some());
        shell.set_clock(if self.last_clock.is_empty() { "16:23".into() } else { self.last_clock.clone().into() });
        shell.set_online(self.online);
        shell.set_network(if self.options.offline { tr("common_offline") } else if self.online { "Wi-Fi".into() } else { tr("common_unknown") });
        if self.demo.is_some() { shell.set_battery(if self.options.profile.is_scribe() { "91%" } else { "82%" }.into()); }
        if let Some(view) = self.view_dirty.take(&self.route) {
            let result = match view {
                View::Catalog => self.refresh_catalog_view(&ui),
                View::Account => self.refresh_account_view(&ui),
                View::Purchase => self.refresh_purchase_view(&ui),
                View::Downloads => self.refresh_download_view(&ui),
                View::Reader => {
                    self.refresh_catalog_local_snapshot();
                    self.refresh_reader_view(&ui)
                }
            };
            if result.is_err() { self.view_dirty.mark(view); }
            result?;
        }
        Ok(())
    }

    fn navigate(&mut self, route: &str) {
        if self.is_suspended() { return; }
        let route = match route { "library" => "shelf", "settings" => "account", "me" => "account", other => other };
        if !["shelf", "store", "search", "detail", "reader", "downloads", "account", "purchase", "welcome", "login"].contains(&route) { return; }
        if ["welcome", "login"].contains(&self.route.as_str()) && !["welcome", "login"].contains(&route) {
            self.client.cancel_login(); self.qr_key = None; self.qr_polling = false;
            self.invalidate("qr-generate"); self.invalidate("qr-poll");
        }
        if self.route == "reader" && route != "reader" { let _ = self.close_reader(); }
        if self.route != route { self.previous_route = self.route.clone(); }
        self.route = route.to_owned();
        if let Some(ui) = self.ui.upgrade() { ui.global::<ShellModel>().set_route(route.into()); }
        let _ = self.platform.request_refresh(RefreshReason::EnterScreen);
        self.dirty = true;
        match route {
            "welcome" | "login" => self.start_login(),
            "store" => self.load_store(),
            "shelf" if self.demo.is_none() && !self.options.offline && self.client.session().is_some() => self.sync_shelf(),
            _ => {}
        }
    }

    fn go_back(&mut self) -> Result<()> {
        let target = match self.route.as_str() {
            "reader" => "detail".to_owned(),
            "search" => "store".to_owned(),
            "purchase" => "detail".to_owned(),
            "login" => self.auth_return.clone(),
            "detail" if self.previous_route == "search" || self.previous_route == "store" => self.previous_route.clone(),
            _ => "shelf".to_owned(),
        };
        self.navigate(&target);
        Ok(())
    }

    fn require_network(&mut self, section: &str, action: &str, value: &str) -> bool {
        if self.is_suspended() { return false; }
        if !self.options.offline || self.demo.is_some() && self.online { return true; }
        self.pending_network = Some((section.into(), action.into(), value.into()));
        if let Some(ui) = self.ui.upgrade() {
            let auth = ui.global::<AuthModel>();
            auth.set_net_visible(true);
            auth.set_net_phase("idle".into());
            auth.set_net_action(self.network_action_label(section, action, value).into());
            auth.set_net_message("".into());
        }
        false
    }

    fn network_action_label(&self, section: &str, action: &str, value: &str) -> String {
        if matches!(action, "comic.open" | "comic.read") {
            if let Some(comic) = self.catalog.comics.iter().find(|comic| comic.id == value) {
                return i18n::format("network_open_title", &[("title", comic.title.clone())]);
            }
            return i18n::text("network_open_comic");
        }
        let key = if action.starts_with("search.") || section == "search" { "network_search" }
            else if section == "downloads" || action.starts_with("detail.download") { "network_download" }
            else if section == "purchase" || action == "recharge" { "network_purchase" }
            else if section == "reader" || action == "episode.open" { "network_read" }
            else if action.starts_with("store.") || section == "store" { "network_store" }
            else if action == "shelf.sync" { "network_sync" }
            else if action == "detail.favorite" { "network_favorite" }
            else if action == "refresh-wallet" { "network_wallet" }
            else { "network_continue" };
        i18n::text(key)
    }

    fn require_session(&mut self) -> bool {
        if self.is_suspended() { return false; }
        if self.demo.is_some() || self.client.session().and_then(|session| session.account_key().map(str::to_owned)).is_some() { return true; }
        self.auth_return = self.route.clone();
        self.navigate("login");
        false
    }

    fn source_job<T, F, A>(&mut self, key: &str, future: F, apply: A)
    where T: Send + 'static, F: Future<Output = source::Result<T>> + Send + 'static,
        A: FnOnce(&mut Self, source::Result<T>) + Send + 'static,
    {
        if self.is_suspended() { return; }
        // Submitted financial work persists its observation before publishing UI
        // state. Account changes may invalidate the UI, never that observation.
        let durable = key.starts_with("purchase-submit:") || key.starts_with("recharge-create:");
        let (epoch, sequence, ticket) = match self.begin_job(key, durable) {
            Ok(job) => job,
            Err(_) => {
                apply(self, Err(source::SourceError::capability("application_job_capacity_reached")));
                return;
            }
        };
        self.source_job_reserved(key, (epoch, sequence, ticket), future, apply);
    }

    fn source_job_reserved<T, F, A>(&mut self, key: &str,
        admission: (u64, u64, jobs::JobTicket), future: F, apply: A)
    where T: Send + 'static, F: Future<Output = source::Result<T>> + Send + 'static,
        A: FnOnce(&mut Self, source::Result<T>) + Send + 'static,
    {
        let (epoch, sequence, ticket) = admission;
        let durable = key.starts_with("purchase-submit:") || key.starts_with("recharge-create:");
        let receipt_signal = durable.then(|| self.sleep_receipt_signal()).flatten();
        let key = key.to_owned();
        self.handle.spawn(async move {
            // Protected submissions must not wait behind paused UI results.
            // Their independent durable admission bounds execution; reserve UI
            // delivery only after their observed outcome has been persisted.
            let delivery = if durable { None } else {
                let Some(delivery) = ticket.acquire_delivery().await else { return; };
                Some(delivery)
            };
            let worker = tokio::spawn(future);
            let _abort_on_drop = AbortJobOnDrop(worker.abort_handle());
            let joined = if durable {
                worker.await
            } else {
                tokio::select! {
                    biased;
                    _ = ticket.cancelled() => return,
                    result = worker => result,
                }
            };
            let result = joined.unwrap_or_else(|_| Err(source::SourceError::new(ErrorKind::Unavailable, "application_source_worker_failed")));
            if let Some(signal) = receipt_signal { signal.notify_one(); }
            let delivery = match delivery {
                Some(delivery) => delivery,
                None => tokio::select! {
                    biased;
                    _ = ticket.cancelled() => return,
                    delivery = ticket.acquire_delivery() => {
                        let Some(delivery) = delivery else { return; };
                        delivery
                    }
                }
            };
            ticket.publish(JobEvent { epoch, sequence, key, apply: Box::new(move |app| {
                if result.is_ok() { app.online = true; }
                apply(app, result);
            }) }, delivery);
        });
    }

    fn begin_job(&mut self, key: &str, durable: bool) -> Result<(u64, u64, jobs::JobTicket)> {
        let sequence = self.sequence.checked_add(1).context("Application job sequence exhausted")?;
        let ticket = self.jobs.begin(self.epoch, sequence, key, durable)?;
        self.sequence = sequence;
        self.pending.insert(key.to_owned(), sequence);
        Ok((self.epoch, sequence, ticket))
    }

    fn background<T, F, A>(&mut self, key: &str, work: F, apply: A)
    where T: Send + 'static, F: FnOnce() -> Result<T> + Send + 'static,
        A: FnOnce(&mut Self, Result<T>) + Send + 'static,
    {
        self.background_after(key, Duration::ZERO, work, apply);
    }

    fn background_after<T, F, A>(&mut self, key: &str, delay: Duration, work: F, apply: A)
    where T: Send + 'static, F: FnOnce() -> Result<T> + Send + 'static,
        A: FnOnce(&mut Self, Result<T>) + Send + 'static,
    {
        if self.is_suspended() { return; }
        let (epoch, sequence, ticket) = match self.begin_job(key, false) {
            Ok(job) => job,
            Err(error) => { apply(self, Err(error)); return; }
        };
        self.background_reserved_after(key, (epoch, sequence, ticket), delay, work, apply);
    }

    fn background_reserved<T, F, A>(&mut self, key: &str,
        admission: (u64, u64, jobs::JobTicket), work: F, apply: A)
    where T: Send + 'static, F: FnOnce() -> Result<T> + Send + 'static,
        A: FnOnce(&mut Self, Result<T>) + Send + 'static,
    {
        self.background_reserved_after(key, admission, Duration::ZERO, work, apply);
    }

    fn background_reserved_after<T, F, A>(&mut self, key: &str,
        admission: (u64, u64, jobs::JobTicket), delay: Duration, work: F, apply: A)
    where T: Send + 'static, F: FnOnce() -> Result<T> + Send + 'static,
        A: FnOnce(&mut Self, Result<T>) + Send + 'static,
    {
        let (epoch, sequence, ticket) = admission;
        let memory_cost = background_memory_cost(key);
        let key = key.to_owned();
        self.handle.spawn(async move {
            if !delay.is_zero() {
                tokio::select! {
                    biased;
                    _ = ticket.cancelled() => return,
                    _ = tokio::time::sleep(delay) => {}
                }
            }
            let Some(delivery) = ticket.acquire_delivery().await else { return; };
            let computation = source::work::run_blocking(memory_cost, move |cancel| {
                // The result slot lives inside the real worker. Dropping its
                // awaiting future cannot admit more unconsumed pixel results.
                let result = if cancel.is_cancelled() {
                    Err(anyhow::anyhow!("Application worker cancelled"))
                } else {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("Application worker panicked")))
                };
                (delivery, result)
            });
            let outcome = tokio::select! {
                biased;
                _ = ticket.cancelled() => return,
                result = computation => result,
            };
            let (delivery, result) = match outcome {
                Ok(result) => result,
                Err(error) => {
                    let Some(delivery) = ticket.acquire_delivery().await else { return; };
                    (delivery, Err(anyhow::anyhow!("Application worker unavailable: {error}")))
                }
            };
            ticket.publish(JobEvent { epoch, sequence, key,
                apply: Box::new(move |app| apply(app, result)) }, delivery);
        });
    }

    fn invalidate(&mut self, key: &str) {
        if key == "network-check" { self.network_verified_epoch = None; }
        self.jobs.invalidate(key);
        self.pending.remove(key);
    }

    fn cover_background<T, F, A>(&mut self, key: &str, work: F, apply: A)
    where T: Send + 'static, F: FnOnce() -> Result<T> + Send + 'static,
        A: FnOnce(&mut Self, Result<T>) + Send + 'static,
    {
        self.background(key, work, apply);
    }

    fn source_failure(&mut self, error: &source::SourceError) {
        if error.kind == ErrorKind::Cancelled { return; }
        let key = match error.kind {
            ErrorKind::Authentication | ErrorKind::AccountMismatch => "common_session_expired",
            ErrorKind::Network | ErrorKind::Http => { self.online = false; "common_network_error" },
            ErrorKind::Storage => "common_storage_error",
            ErrorKind::Capability => "common_not_supported",
            ErrorKind::Locked => "common_locked",
            ErrorKind::PurchaseUnknown => "common_payment_pending",
            _ => "common_source_error",
        };
        self.toast(key);
    }

    fn toast(&mut self, key: &str) { self.toast_text(i18n::text(key)); }
    fn toast_text(&mut self, text: String) {
        if let Some(ui) = self.ui.upgrade() { ui.global::<ShellModel>().set_toast(text.into()); }
        self.toast_until = Some(Instant::now() + Duration::from_secs(3));
    }
    fn save_preferences(&self) -> Result<()> { self.store.set_preference(&self.account, "preferences", &self.preferences) }
    fn is_scribe(&self) -> bool { self.options.profile.is_scribe() }

    fn download_source(&self) -> source::Result<BiliClient> {
        if self.client.credential_context().is_some_and(|(account, _)| account == self.account) {
            self.client.bound_to_account(&self.account)
        } else { BiliClient::offline(&self.data_dir) }
    }

    fn pause_download_metadata(&self) -> Result<()> {
        for record in self.store.downloads(&self.account)?.iter().filter(|record|
            matches!(record.status, crate::store::DownloadStatus::Running | crate::store::DownloadStatus::Queued))
        {
            self.store.update_download_state(&self.account, &record.id, crate::store::DownloadStatus::Paused, None, now())?;
        }
        Ok(())
    }

    fn comic_view(&mut self, comic: &Comic) -> ComicItem {
        let cover = self.cover_image(comic);
        let anchor = self.store.load_anchor(&self.account, &comic.id).ok().flatten();
        let subtitle = self.demo.as_ref().and_then(|demo| demo.fixture.get("comics").and_then(|value| value.as_array()))
            .and_then(|items| items.iter().find(|value| value.get("id").and_then(|value| value.as_u64()).map(|id| id.to_string()) == Some(comic.id.clone())))
            .and_then(|value| value.get("read_subtitle").and_then(|value| value.as_str())).map(str::to_owned)
            .unwrap_or_else(|| anchor.as_ref().map(|anchor| i18n::format("common_progress", &[
                ("episode", self.episode_order(&anchor.episode_id)),
                ("page", (anchor.page_index + 1).to_string()),
                ("total", self.store.load_index(&self.account, &anchor.episode_id, Some(&anchor.revision)).ok().flatten().map_or_else(|| "?".into(), |index| index.images.len().to_string())),
            ])).unwrap_or_else(|| i18n::text("common_no_reading")));
        let fixture = self.demo.as_ref().and_then(|demo| demo.fixture["comics"].as_array())
            .and_then(|items| items.iter().find(|item| item["id"].as_u64().map(|id| id.to_string()) == Some(comic.id.clone())));
        let fixture_unchanged = anchor.as_ref().zip(fixture).is_some_and(|(anchor, fixture)| {
            let episode = fixture["read_episode"].as_u64().unwrap_or_default();
            let fixture_episode = format!("{}{:02}", comic.id, episode);
            fixture["read_page"].as_u64() == Some(anchor.page_index as u64 + 1) && anchor.episode_id == fixture_episode
        });
        let subtitle = if let Some(anchor) = anchor.as_ref().filter(|_| !fixture_unchanged) {
            let episode = self.store.load_detail(&self.account, &comic.id).ok().flatten()
                .and_then(|detail| detail.episodes.into_iter().find(|episode| episode.id == anchor.episode_id))
                .map(|episode| display_order(episode.order)).unwrap_or_else(|| "?".into());
            i18n::format("common_progress", &[
                ("episode", episode), ("page", (anchor.page_index + 1).to_string()),
                ("total", self.store.load_index(&self.account, &anchor.episode_id, Some(&anchor.revision)).ok().flatten()
                    .map_or_else(|| "?".into(), |index| index.images.len().to_string())),
            ])
        } else { subtitle };
        let downloaded = self.comic_has_download(comic);
        let meta = if comic.finished { i18n::format("common_chapters", &[("count", comic.chapter_count.to_string())]) }
            else { comic.latest_episode_title.clone() };
        ComicItem { id: comic.id.clone().into(), title: comic.title.clone().into(), author: comic.author.clone().into(), cover,
            subtitle: subtitle.clone().into(), meta: meta.into(), description: comic.description.clone().into(),
            read_label: subtitle.into(), badge: if comic.extra["updated"].as_bool().unwrap_or(false) { tr("common_updated") } else { "".into() },
            downloaded, progress: anchor.as_ref().map_or(comic.progress as f32 / 100.0, |anchor| {
                let total = self.store.load_index(&self.account, &anchor.episode_id, Some(&anchor.revision)).ok().flatten().map_or(1, |index| index.images.len());
                (anchor.page_index + 1) as f32 / total.max(1) as f32
            }), in_shelf: comic.favorite }
    }

    fn episode_order(&self, id: &str) -> String {
        self.catalog.detail.as_ref().and_then(|detail| detail.episodes.iter().find(|episode| episode.id == id))
            .map(|episode| display_order(episode.order)).unwrap_or_else(|| "?".into())
    }

    fn refresh_feedback(&self, kind: &str) {
        if self.is_suspended() { return; }
        let _ = self.platform.request_refresh(feedback_reason(kind));
    }

    fn query_device_status(&mut self) {
        if self.is_suspended() || self.demo.is_some() { return; }
        let platform = self.platform.clone();
        let directory = self.data_dir.clone();
        self.background("device-status", move || Ok((platform.battery_percentage(), available_bytes(&directory))), |app, result| {
            if let Ok((battery, free)) = result {
                if let Ok(value) = battery {
                    if let Some(ui) = app.ui.upgrade() { ui.global::<ShellModel>().set_battery(format!("{value}%").into()); }
                }
                if let Some(free) = free { app.download_ui.set_free_space(free); }
            }
        });
    }

    pub fn request_quit(&mut self) { self.cancel_sleep_maintenance(); self.quitting = true; }
    pub fn is_quitting(&self) -> bool { self.quitting }
    pub fn is_offline(&self) -> bool { self.options.offline }
    pub fn resource_snapshot(&self) -> serde_json::Value {
        let mut snapshot = self.reader_resource_snapshot();
        snapshot["cover_resident_count"] = self.cover_images.len().into();
        snapshot["cover_resident_bytes"] = self.cover_images.values().map(|cover| cover.bytes).sum::<usize>().into();
        snapshot["cover_image_budget_bytes"] = COVER_IMAGE_BUDGET.into();
        snapshot["pending_job_keys"] = self.pending.len().into();
        snapshot
    }

    pub fn flush_observed_receipts(&mut self) -> Result<()> { self.flush_account_receipts() }
    pub fn shutdown(&mut self) -> Result<()> {
        self.cancel_sleep_maintenance();
        self.save_reading_position()?;
        self.save_preferences()?;
        self.flush_account_receipts()?;
        if let Some(manager) = &mut self.downloads { manager.pause_all()?; }
        self.store.checkpoint()?;
        self.jobs.close();
        Ok(())
    }
}

impl Drop for App {
    fn drop(&mut self) { self.cancel_sleep_maintenance(); self.jobs.close(); }
}

struct AbortJobOnDrop(tokio::task::AbortHandle);

impl Drop for AbortJobOnDrop {
    fn drop(&mut self) { self.0.abort(); }
}

fn background_memory_cost(key: &str) -> u64 {
    // Conservative in-flight reservations, separate from the decoded-source
    // and UI image caches. Local metadata work does not reserve a full frame.
    if key.starts_with("reader-render") || key.starts_with("reader-thumbnails")
        || matches!(key, "startup-download-manager" | "download-recover" | "download-remove" | "catalog:local-snapshot") {
        source::work::HEAVY_WORK_BYTES
    } else if key.starts_with("cover-local:") || key.starts_with("cover-prepare:") {
        source::work::COVER_WORK_BYTES
    } else {
        source::work::METADATA_WORK_BYTES
    }
}

fn feedback_reason(kind: &str) -> RefreshReason {
    match kind {
        "press" => RefreshReason::Press,
        "keyboard" | "keyboard-release" | "key" => RefreshReason::Keyboard,
        "content" => RefreshReason::Content,
        _ => RefreshReason::FeedbackReleased,
    }
}

fn content_refresh_action(section: &str, action: &str) -> bool {
    match section {
        "catalog" | "library" | "detail" | "search" | "store" => matches!(action,
            "shelf.page" | "shelf.filter" | "shelf.view" | "store.page" | "store.category"
            | "search.page" | "episode.page" | "episode.filter" | "episode.sort" | "detail.locate"),
        "downloads" => action == "page",
        "account" => matches!(action, "section" | "back"),
        _ => false,
    }
}

#[cfg(test)]
mod feedback_tests {
    use super::*;

    #[test]
    fn keyboard_and_release_intents_do_not_consume_list_refresh_budget() {
        for kind in ["release", "unknown-feedback"] {
            assert_eq!(feedback_reason(kind), RefreshReason::FeedbackReleased);
        }
        assert_eq!(feedback_reason("keyboard"), RefreshReason::Keyboard);
        assert_eq!(feedback_reason("keyboard-release"), RefreshReason::Keyboard);
        assert_eq!(feedback_reason("key"), RefreshReason::Keyboard);
        for action in ["search.key", "search.candidate", "search.keyboard", "search.clear"] {
            assert!(!content_refresh_action("catalog", action));
        }
        assert!(content_refresh_action("catalog", "shelf.page"));
        assert!(content_refresh_action("catalog", "episode.filter"));
        assert!(content_refresh_action("downloads", "page"));
        assert!(!content_refresh_action("reader", "turn"));
    }
}

pub(super) fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> { Rc::new(VecModel::from(items)).into() }
pub(super) fn tr(key: &str) -> SharedString { i18n::text(key).into() }
pub(super) fn display_order(order: f64) -> String {
    if order.fract() == 0.0 { format!("{order:.0}") } else { order.to_string() }
}
pub(super) fn now() -> i64 { chrono::Utc::now().timestamp() }
pub(super) fn load_image(path: &Path) -> slint::Image { slint::Image::load_from_path(path).unwrap_or_default() }
pub(super) fn hex_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn demo_clock(route: &str) -> &'static str {
    match route {
        "welcome" => "16:20", "detail" => "16:24", "store" => "16:25", "search" => "16:26",
        "reader" => "16:27", "downloads" => "16:28", "purchase" => "16:29", "account" => "16:30", "login" => "16:31", _ => "16:23",
    }
}

fn available_bytes(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let path = CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut status = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // The initialized structure is read only after the platform call succeeds.
        if unsafe { libc::statvfs(path.as_ptr(), status.as_mut_ptr()) } != 0 { return None; }
        let status = unsafe { status.assume_init() };
        Some((status.f_bavail as u64).saturating_mul(status.f_frsize as u64))
    }
    #[cfg(not(unix))]
    { let _ = path; None }
}

#[cfg(test)]
#[path = "resource_job_tests.rs"]
mod resource_job_tests;

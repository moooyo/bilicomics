use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use slint::ComponentHandle;

use super::{App, View, display_order, hex_digest, model, now, tr};
use crate::{
    demo::DemoData,
    domain::{Comic, ComicDetail, Episode, EpisodeAccess, PageDescriptor},
    i18n,
    input::{InputIntent, InputLanguage, PinyinInput},
    source::{CategoryMetadata, CategoryOption},
    store::{PageReceipt, Store},
    ui::{AppWindow, CatalogModel, CategoryItem, ComicItem, EpisodeItem, SearchHistoryItem},
};

const SHELF_BATCH: u32 = 100;
const SEARCH_BATCH: u32 = 100;
const MAX_SHELF_BATCHES: u32 = 20;
const AUTH_RETURN_LIFETIME: Duration = Duration::from_secs(600);
const LOCAL_SCAN_INTERVAL: Duration = Duration::from_secs(30);
const LOCAL_SCAN_YIELD: Duration = Duration::from_millis(25);

#[path = "catalog_inventory.rs"]
mod inventory;
use inventory::InventoryScan;

#[cfg(test)]
#[path = "catalog_recheck_tests.rs"]
mod catalog_recheck_tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogViewRoute {
    Shelf,
    Store,
    Search,
    Detail,
}

impl CatalogViewRoute {
    fn for_route(route: &str) -> Option<Self> {
        match route {
            "shelf" => Some(Self::Shelf),
            "store" => Some(Self::Store),
            "search" => Some(Self::Search),
            "detail" => Some(Self::Detail),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct HistoryEntry {
    query: String,
    count: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct CatalogPreferences {
    view: String,
    filter: String,
    shelf_page: usize,
    category: String,
    category_label: String,
    store_page: usize,
}

impl Default for CatalogPreferences {
    fn default() -> Self {
        Self { view: "grid".into(), filter: "all".into(), shelf_page: 1,
            category: "all".into(), category_label: String::new(), store_page: 1 }
    }
}

/// A navigation intent carries identities, never a previous account's access state.
#[derive(Clone)]
pub(super) struct CatalogAuthReturn {
    comic_id: String,
    selected: BTreeSet<String>,
    selecting: bool,
    destination_account: String,
    captured: Instant,
}

impl CatalogAuthReturn {
    fn selection_from(&self, detail: &ComicDetail, timestamp: i64) -> BTreeSet<String> {
        if detail.comic.id != self.comic_id { return BTreeSet::new(); }
        detail.episodes.iter().filter(|episode| episode.comic_id == self.comic_id
            && self.selected.contains(&episode.id) && episode.readable_at(timestamp))
            .map(|episode| episode.id.clone()).collect()
    }
}

pub(super) struct CatalogState {
    pub(super) comics: Vec<Comic>,
    pub(super) detail: Option<ComicDetail>,
    shelf_ids: BTreeSet<String>,
    history_comics: Vec<Comic>,
    store_comics: Vec<Comic>,
    results: Vec<Comic>,
    categories: Vec<CategoryOption>,
    history: Vec<HistoryEntry>,
    saved: CatalogPreferences,
    input: PinyinInput,
    submitted_query: String,
    id_input: bool,
    shelf_menu: bool,
    category_picker: bool,
    keyboard: bool,
    search_page: usize,
    detail_page: usize,
    detail_filter: String,
    detail_descending: bool,
    selecting: bool,
    selected: BTreeSet<String>,
    shelf_syncing: bool,
    store_loading: bool,
    detail_loading: bool,
    search_loading: bool,
    category_loading: bool,
    shelf_has_more: bool,
    store_has_more: bool,
    search_has_more: bool,
    shelf_server_page: u32,
    store_server_page: u32,
    search_server_page: u32,
    favorite_pending: BTreeSet<String>,
    last_sync: i64,
    status: String,
    local_snapshot: LocalSnapshot,
    local_dirty: bool,
    local_loading: bool,
    local_generation: u64,
    local_checked: Instant,
    local_full_checked: Option<Instant>,
    local_scan_full: bool,
    local_attempted: Instant,
    local_failed: bool,
    local_scan: Option<InventoryScan>,
    cache_maintenance_pending: bool,
    cache_retry_at: Option<Instant>,
    cache_retry_attempts: u8,
    auth_return_pending: Option<CatalogAuthReturn>,
    auth_detail_pending: Option<CatalogAuthReturn>,
}

impl Default for CatalogState {
    fn default() -> Self {
        Self {
            comics: Vec::new(), detail: None, shelf_ids: BTreeSet::new(),
            history_comics: Vec::new(), store_comics: Vec::new(), results: Vec::new(),
            categories: Vec::new(), history: Vec::new(), saved: CatalogPreferences::default(),
            input: PinyinInput::default(), submitted_query: String::new(), id_input: false,
            shelf_menu: false, category_picker: false, keyboard: true,
            search_page: 1, detail_page: 1, detail_filter: "all".into(), detail_descending: false,
            selecting: false, selected: BTreeSet::new(), shelf_syncing: false,
            store_loading: false, detail_loading: false, search_loading: false,
            category_loading: false, shelf_has_more: false, store_has_more: false,
            search_has_more: false, shelf_server_page: 1, store_server_page: 1,
            search_server_page: 1, favorite_pending: BTreeSet::new(), last_sync: 0,
            status: String::new(),
            local_snapshot: LocalSnapshot::default(), local_dirty: false, local_loading: false,
            local_generation: 0, local_checked: Instant::now(), local_attempted: Instant::now(),
            local_full_checked: None, local_scan_full: false,
            local_failed: false, local_scan: None,
            cache_maintenance_pending: false, cache_retry_at: None, cache_retry_attempts: 0,
            auth_return_pending: None, auth_detail_pending: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct LocalEpisode {
    known: bool,
    revision: String,
    present: usize,
    total: usize,
    retained: bool,
    bytes: u64,
}

#[derive(Default)]
struct LocalSnapshot {
    episodes: BTreeMap<String, LocalEpisode>,
    episode_comics: BTreeMap<String, String>,
    downloaded_comics: BTreeSet<String>,
    checked_all: bool,
}

impl LocalSnapshot {
    fn inventory_for_revision(&self, episode_id: &str, revision: &str) -> Option<(u32, u32, bool)> {
        let local = self.episodes.get(episode_id).filter(|local| local.known && local.revision == revision)?;
        Some((local.present.try_into().ok()?, local.total.try_into().ok()?, local.retained))
    }

    fn from_demo(comics: &[Comic], demo: &DemoData) -> Self {
        let mut snapshot = Self::default();
        for detail in &demo.details {
            for episode in &detail.episodes {
                let total = episode.extra["page_count"].as_u64().unwrap_or(0) as usize;
                let retained = episode.extra["downloaded"].as_bool().unwrap_or(false);
                snapshot.episodes.insert(episode.id.clone(), LocalEpisode {
                    known: true, present: if retained { total } else { episode.extra["downloaded_pages"].as_u64().unwrap_or(0) as usize },
                    total, retained, bytes: 0,
                    revision: demo.indices.iter().find(|index| index.episode_id == episode.id).map(|index| index.revision.clone()).unwrap_or_default(),
                });
                snapshot.episode_comics.insert(episode.id.clone(), detail.comic.id.clone());
            }
        }
        // Fixture presentation state is isolated from real page receipts.
        snapshot.downloaded_comics = comics.iter().filter(|comic| comic.extra["downloaded"].as_bool().unwrap_or(false))
            .map(|comic| comic.id.clone()).collect();
        snapshot.checked_all = true;
        snapshot
    }

    fn invalidate_episode(&mut self, comic_id: &str, episode_id: &str) {
        self.checked_all = false;
        self.episodes.remove(episode_id);
        self.episode_comics.insert(episode_id.to_owned(), comic_id.to_owned());
        self.downloaded_comics.remove(comic_id);
        if self.episode_comics.iter().any(|(episode, comic)| comic == comic_id
            && self.episodes.get(episode).is_some_and(|local| local.retained))
        { self.downloaded_comics.insert(comic_id.to_owned()); }
    }

    fn install_episode(&mut self, comic_id: Option<String>, episode_id: String, local: LocalEpisode) {
        let retained = local.retained;
        self.episodes.insert(episode_id.clone(), local);
        let Some(comic_id) = comic_id else { return; };
        self.episode_comics.insert(episode_id, comic_id.clone());
        if retained {
            self.downloaded_comics.insert(comic_id);
        } else if self.downloaded_comics.contains(&comic_id)
            && !self.episode_comics.iter().any(|(episode, owner)| *owner == comic_id
                && self.episodes.get(episode).is_some_and(|local| local.retained))
        {
            self.downloaded_comics.remove(&comic_id);
        }
    }

    fn retain_episodes(&mut self, live: &BTreeSet<String>) {
        self.episodes.retain(|episode, _| live.contains(episode));
        self.episode_comics.retain(|episode, _| live.contains(episode));
        self.downloaded_comics = self.episode_comics.iter().filter_map(|(episode, comic)|
            self.episodes.get(episode).is_some_and(|local| local.retained).then(|| comic.clone())).collect();
    }
}

impl CatalogState {
    pub(super) fn defer_cache_maintenance(&mut self) {
        self.cache_maintenance_pending = true;
        self.cache_retry_attempts = self.cache_retry_attempts.saturating_add(1).min(6);
        self.cache_retry_at = Some(Instant::now() + Duration::from_millis(
            250_u64 << self.cache_retry_attempts.saturating_sub(1)));
    }

    pub(super) fn prepare(store: &Store, account: &str, demo: Option<&DemoData>) -> Result<Self> {
        let saved = store.preference(account, "catalog_view")?.unwrap_or_default();
        let stored_history: Option<Vec<HistoryEntry>> = store.preference(account, "search_history")?;
        let mut state = CatalogState { saved, ..Default::default() };
        if !["grid", "list"].contains(&state.saved.view.as_str()) { state.saved.view = "grid".into(); }
        if !["all", "updated", "downloaded", "finished"].contains(&state.saved.filter.as_str()) { state.saved.filter = "all".into(); }
        state.saved.shelf_page = state.saved.shelf_page.max(1);
        state.saved.store_page = state.saved.store_page.max(1);
        state.comics = store.load_comics(account)?;
        let shelf = store.load_catalog(account, "shelf")?;
        let shelf_initialized = store.preference::<bool>(account, "catalog_shelf_initialized")?.unwrap_or(false);
        state.shelf_ids = if shelf.is_empty() && !shelf_initialized {
            state.comics.iter().filter(|comic| comic.favorite).map(|comic| comic.id.clone()).collect()
        } else { shelf.iter().map(|comic| comic.id.clone()).collect() };
        state.history_comics = store.load_catalog(account, "history")?;
        state.store_comics = store.load_catalog(account, &store_scope(&state.saved.category))?;
        state.categories = store.preference(account, "catalog_categories")?.unwrap_or_default();
        state.last_sync = store.preference(account, "catalog_last_sync")?.unwrap_or(0);
        state.history = stored_history.clone().unwrap_or_default();
        if let Some(demo) = demo {
            state.comics = demo.comics.clone();
            state.shelf_ids = state.comics.iter().filter(|comic| comic.favorite).map(|comic| comic.id.clone()).collect();
            state.categories = demo.fixture["categories"].as_array().into_iter().flatten().enumerate()
                .filter_map(|(index, value)| value.as_str().map(|name| CategoryOption {
                    id: if index == 0 { "all".into() } else { format!("demo:{index}") }, name: name.into(),
                })).collect();
            if stored_history.is_none() {
                state.history = demo.fixture["recent_searches"].as_array().into_iter().flatten()
                    .filter_map(|raw| raw["query"].as_str().map(|query| HistoryEntry {
                        query: query.into(), count: raw["count"].as_u64().map(|count| count as usize),
                    })).collect();
            }
        }
        if state.saved.category.is_empty() { state.saved.category = "all".into(); }
        if state.saved.category_label.is_empty() { state.saved.category_label = i18n::text("cat_all_recommendations"); }
        if let Some(demo) = demo {
            state.local_snapshot = LocalSnapshot::from_demo(&state.comics, demo);
        } else {
            // Metadata can attach immediately. Only completed background scans
            // may turn unknown local availability into an offline-ready claim.
            state.local_dirty = true;
        }
        state.local_checked = Instant::now();
        state.local_attempted = state.local_checked;
        Ok(state)
    }

    pub(super) fn startup_cover_comics(&self, route: &str, scribe: bool, resume: Option<&str>, demo: bool) -> Vec<Comic> {
        match route {
            "detail" | "purchase" => self.detail.as_ref().map(|detail| vec![detail.comic.clone()]).unwrap_or_default(),
            "store" => page_slice(&self.store_comics, self.saved.store_page, if scribe { 15 } else { 6 }),
            "search" => self.comics.iter().take(6).cloned().collect(),
            "shelf" => {
                let mut shelf: Vec<_> = self.comics.iter().filter(|comic| self.shelf_ids.contains(&comic.id))
                    .filter(|comic| !demo || scribe || comic.extra["shelf_order"].as_u64().is_some_and(|order| order < 12))
                    .filter(|comic| (!scribe && self.saved.filter != "all") || Some(comic.id.as_str()) != resume)
                    .filter(|comic| match self.saved.filter.as_str() {
                        "updated" => comic.extra["updated"].as_bool().unwrap_or(false),
                        "downloaded" => self.local_snapshot.downloaded_comics.contains(&comic.id),
                        "finished" => comic.finished,
                        _ => true,
                    }).cloned().collect();
                if demo { shelf.sort_by_key(|comic| comic.extra["shelf_order"].as_u64().unwrap_or(u64::MAX)); }
                let per_page = match (scribe, self.saved.view.as_str()) {
                    (true, "list") => 6, (true, _) => 10, (false, "list") => 7, _ => 6,
                };
                let mut visible = page_slice(&shelf, self.saved.shelf_page.min(page_count(shelf.len(), per_page)), per_page);
                if let Some(comic) = resume.and_then(|id| self.comics.iter().find(|comic| comic.id == id)) {
                    visible.push(comic.clone());
                }
                visible
            }
            _ => Vec::new(),
        }
    }
}

impl App {
    pub(super) fn initialize_catalog(&mut self) -> Result<()> {
        self.catalog = CatalogState::prepare(&self.store, &self.account, self.demo.as_deref())?;
        self.trim_catalog_cache();
        if self.demo.is_some() { self.load_store(); }
        Ok(())
    }

    fn trim_catalog_cache(&mut self) {
        if self.demo.is_some() { return; }
        self.catalog.cache_maintenance_pending = true;
        self.maintain_catalog_cache();
    }

    pub(super) fn maintain_catalog_cache(&mut self) {
        if self.demo.is_some() || self.is_suspended() || !self.catalog.cache_maintenance_pending
            || self.catalog.cache_retry_at.is_some_and(|at| Instant::now() < at)
        { return; }
        let mut protected: BTreeSet<String> = self.catalog.results.iter().chain(&self.catalog.store_comics)
            .map(|comic| comic.id.clone()).collect();
        if let Some(detail) = &self.catalog.detail { protected.insert(detail.comic.id.clone()); }
        if let Some((comic, _)) = self.active_reader_identity() { protected.insert(comic.to_owned()); }
        for intent in [&self.catalog.auth_return_pending, &self.catalog.auth_detail_pending].into_iter().flatten() {
            protected.insert(intent.comic_id.clone());
        }
        protected.extend(self.catalog.favorite_pending.iter().cloned());
        match self.store.try_prune_catalog_cache(&self.account, &protected.into_iter().collect::<Vec<_>>()) {
            Ok(Some(evicted)) => {
                self.catalog.cache_maintenance_pending = false;
                self.catalog.cache_retry_at = None;
                self.catalog.cache_retry_attempts = 0;
                if evicted.is_empty() { return; }
                let evicted: BTreeSet<_> = evicted.into_iter().collect();
                self.catalog.comics.retain(|comic| !evicted.contains(&comic.id));
                let live: BTreeSet<_> = self.catalog.local_snapshot.episode_comics.iter()
                    .filter_map(|(episode, comic)| (!evicted.contains(comic)).then(|| episode.clone())).collect();
                self.catalog.local_snapshot.retain_episodes(&live);
                self.request_catalog_local_refresh();
            }
            Ok(None) => self.catalog.defer_cache_maintenance(),
            Err(error) => {
                // Cache maintenance is optional. Its failed transaction leaves
                // the existing library intact and can be retried next update.
                if self.catalog.cache_retry_attempts == 0 {
                    eprintln!("bilicomics: catalog cache maintenance deferred: {error:#}");
                }
                self.catalog.defer_cache_maintenance();
            }
        }
    }

    pub(super) fn capture_catalog_auth_return(&self) -> Option<CatalogAuthReturn> {
        let detail = self.catalog.detail.as_ref()?;
        let comic_id = normalize_comic_id(&detail.comic.id)?;
        // This in-memory intent must preserve every selected identity across
        // authentication; selection is rechecked against the fresh directory.
        let selected = self.catalog.selected.iter().filter(|id| id.len() <= 18
            && !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())
            && id.bytes().any(|byte| byte != b'0')).cloned().collect();
        Some(CatalogAuthReturn { comic_id, selected, selecting: self.catalog.selecting,
            destination_account: String::new(), captured: Instant::now() })
    }

    pub(super) fn stage_catalog_auth_return(&mut self, intent: Option<CatalogAuthReturn>) {
        self.catalog.auth_return_pending = intent.map(|mut intent| {
            intent.destination_account = self.account.clone();
            intent
        });
        self.catalog.auth_detail_pending = None;
    }

    pub(super) fn clear_catalog_auth_context(&mut self) {
        self.catalog = CatalogState::default();
        self.pending.retain(|key, _| !key.starts_with("catalog:"));
        self.dirty = true;
    }

    pub(super) fn finish_catalog_auth_return(&mut self) -> Result<bool> {
        let Some(mut intent) = self.catalog.auth_return_pending.take() else { return Ok(false); };
        if intent.destination_account != self.account { return Ok(false); }
        if intent.captured.elapsed() > AUTH_RETURN_LIFETIME {
            intent.selected.clear();
            intent.selecting = false;
        }
        let comic_id = intent.comic_id.clone();
        self.open_detail(&comic_id)?;
        if self.catalog.detail.is_none() {
            self.catalog.detail = Some(ComicDetail { comic: Comic { id: comic_id.clone(),
                title: i18n::text("cat_loading_detail"), ..Default::default() }, ..Default::default() });
        }
        // Only comic and episode IDs survive; entitlement comes only from the
        // new account's subsequent successful ComicDetail response.
        if self.demo.is_none() {
            self.catalog.auth_detail_pending = Some(intent);
            self.catalog.status = i18n::text("cat_login_refresh_detail");
            self.require_network("catalog", "comic.open", &comic_id);
        }
        self.catalog.selecting = false;
        self.catalog.selected.clear();
        Ok(true)
    }

    fn apply_catalog_auth_return(&mut self) {
        let Some(intent) = self.catalog.auth_detail_pending.take() else { return; };
        if intent.destination_account != self.account || intent.captured.elapsed() > AUTH_RETURN_LIFETIME
            || self.route != "detail"
        { return; }
        let Some(detail) = self.catalog.detail.as_ref().filter(|detail| detail.comic.id == intent.comic_id) else { return; };
        self.catalog.status.clear();
        if !intent.selecting { return; }
        let selected = intent.selection_from(detail, now());
        let first = detail.episodes.iter().position(|episode| selected.contains(&episode.id));
        self.catalog.selected = selected;
        self.catalog.selecting = true;
        self.catalog.detail_filter = "all".into();
        self.catalog.detail_descending = false;
        if let Some(position) = first { self.catalog.detail_page = position / if self.is_scribe() { 9 } else { 7 } + 1; }
        self.catalog.status.clear();
        self.toast("cat_login_reconfirm");
    }

    pub(super) fn refresh_catalog_view(&mut self, ui: &AppWindow) -> Result<()> {
        let Some(route) = CatalogViewRoute::for_route(&self.route) else { return Ok(()); };
        self.refresh_catalog_local_snapshot();
        let presentation = ui.global::<CatalogModel>();
        presentation.set_status_message(self.catalog.status.clone().into());
        // Hidden global models keep their current values. Entry to a catalog
        // route rebuilds only that route from the authoritative Rust state.
        match route {
            CatalogViewRoute::Shelf => self.refresh_catalog_shelf_view(ui),
            CatalogViewRoute::Store => self.refresh_catalog_store_view(ui),
            CatalogViewRoute::Search => self.refresh_catalog_search_view(ui),
            CatalogViewRoute::Detail => self.refresh_catalog_detail_view(ui),
        }
    }

    fn refresh_catalog_shelf_view(&mut self, ui: &AppWindow) -> Result<()> {
        let presentation = ui.global::<CatalogModel>();
        let resume_id = self.catalog_resume_id()?;
        let mut shelf: Vec<Comic> = self.catalog.comics.iter()
            .filter(|comic| self.catalog.shelf_ids.contains(&comic.id)).cloned().collect();
        if self.demo.is_some() {
            shelf.sort_by_key(|comic| comic.extra["shelf_order"].as_u64().unwrap_or(u64::MAX));
            if !self.is_scribe() { shelf.retain(|comic| comic.extra["shelf_order"].as_u64().is_some_and(|order| order < 12)); }
        } else {
            for comic in &mut shelf {
                if let Some(anchor) = self.store.load_anchor(&self.account, &comic.id)? { comic.last_read_at = comic.last_read_at.max(anchor.updated_at); }
            }
            shelf.sort_by(|left, right| right.last_read_at.cmp(&left.last_read_at)
                .then_with(|| right.updated_at.cmp(&left.updated_at)));
        }
        if self.is_scribe() { shelf.retain(|comic| Some(&comic.id) != resume_id.as_ref()); }
        let full_shelf_count = shelf.len();
        let filter_counts = [
            full_shelf_count,
            shelf.iter().filter(|comic| comic.extra["updated"].as_bool().unwrap_or(false)).count(),
            shelf.iter().filter(|comic| self.comic_has_download(comic)).count(),
            shelf.iter().filter(|comic| comic.finished).count(),
        ];
        let inventory_pending = !self.catalog.local_snapshot.checked_all;
        presentation.set_filter_counts(model(filter_counts.into_iter().enumerate().map(|(index, count)| {
            if index == 2 && inventory_pending { tr("common_unknown") }
            else { i18n::format("cat_comic_count", &[("count", count.to_string())]).into() }
        }).collect()));
        let local_downloads = if self.demo.is_some() && !self.is_scribe() {
            self.catalog.comics.iter().filter(|comic| self.comic_has_download(comic)).count()
        } else { self.catalog.local_snapshot.downloaded_comics.len() };
        presentation.set_local_download_count(if inventory_pending { -1 } else { local_downloads.try_into().unwrap_or(i32::MAX) });
        presentation.set_shelf_offline(self.options.offline && !self.online);
        let filter = self.catalog.saved.filter.clone();
        shelf.retain(|comic| match filter.as_str() {
            "updated" => comic.extra["updated"].as_bool().unwrap_or(false),
            "downloaded" => self.comic_has_download(comic),
            "finished" => comic.finished,
            _ => true,
        });
        if filter == "all" { shelf.retain(|comic| Some(&comic.id) != resume_id.as_ref()); }
        let shelf_per = self.shelf_per_page();
        let shelf_pages = page_count(shelf.len(), shelf_per);
        let presented_page = self.catalog.saved.shelf_page.clamp(1, shelf_pages);
        if filter != "downloaded" || !inventory_pending { self.catalog.saved.shelf_page = presented_page; }
        let visible_shelf = page_slice(&shelf, presented_page, shelf_per);
        let mut shelf_views = Vec::with_capacity(visible_shelf.len());
        for comic in visible_shelf { shelf_views.push(self.shelf_comic_view(&comic)); }
        presentation.set_shelf_items(model(shelf_views));
        presentation.set_shelf_view(self.catalog.saved.view.clone().into());
        presentation.set_shelf_filter(filter.clone().into());
        presentation.set_shelf_menu_open(self.catalog.shelf_menu);
        presentation.set_shelf_page(presented_page as i32);
        presentation.set_shelf_pages((shelf_pages + usize::from(self.catalog.shelf_has_more)) as i32);
        let authenticated = self.demo.is_some() || self.client.session()
            .and_then(|session| session.account_key().map(str::to_owned)).is_some();
        let state = if !authenticated { "signed-out" }
            else if self.catalog.shelf_syncing && full_shelf_count == 0 { "syncing" }
            else if full_shelf_count == 0 && resume_id.is_none() { "empty" } else { "ready" };
        presentation.set_shelf_state(state.into());
        if self.catalog.status.is_empty() && state == "syncing" {
            presentation.set_status_message(tr("cat_first_sync_message"));
        }
        let count_key = if self.catalog.shelf_has_more { "cat_loaded_count" }
            else if filter != "all" { "cat_filtered_count" } else { "cat_remaining_count" };
        presentation.set_shelf_count_label(if filter == "downloaded" && inventory_pending { tr("cat_local_download_checking") }
            else { i18n::format(count_key, &[
            ("count", shelf.len().to_string()), ("total", full_shelf_count.to_string()),
        ]).into() });
        presentation.set_sync_label(if self.demo.is_some() && self.catalog.last_sync > 0 {
                let count = self.catalog.comics.iter().filter(|comic| self.catalog.shelf_ids.contains(&comic.id))
                    .filter(|comic| self.is_scribe() || comic.extra["shelf_order"].as_u64().is_some_and(|order| order < 12)).count();
                i18n::format("cat_just_synced", &[("count", count.to_string())]).into()
            } else if self.demo.is_some() { tr("cat_demo_sync_time") }
            else if self.catalog.last_sync > 0 {
                let time = chrono::DateTime::from_timestamp(self.catalog.last_sync, 0)
                    .map(|time| (time + chrono::Duration::hours(8)).format("%H:%M").to_string()).unwrap_or_default();
                i18n::format("cat_sync_time", &[("time", time)]).into()
            } else { tr("cat_not_synced") });
        if let Some(id) = resume_id {
            if let Some(comic) = self.catalog.comics.iter().find(|comic| comic.id == id).cloned() {
                let resume = self.resume_comic_view(&comic)?;
                presentation.set_resume(resume);
                presentation.set_has_resume(true);
            } else { presentation.set_has_resume(false); }
        } else { presentation.set_has_resume(false); }
        Ok(())
    }

    fn refresh_catalog_store_view(&mut self, ui: &AppWindow) -> Result<()> {
        let presentation = ui.global::<CatalogModel>();
        let store_per = if self.is_scribe() { 15 } else { 6 };
        let store_pages = page_count(self.catalog.store_comics.len(), store_per);
        self.catalog.saved.store_page = self.catalog.saved.store_page.clamp(1, store_pages);
        let store_comics = page_slice(&self.catalog.store_comics, self.catalog.saved.store_page, store_per);
        let mut store_views = Vec::with_capacity(store_comics.len());
        for comic in store_comics {
            let mut item = self.comic_view(&comic);
            item.meta = self.catalog_status(&comic).into();
            item.in_shelf = self.catalog.shelf_ids.contains(&comic.id);
            store_views.push(item);
        }
        presentation.set_store_items(model(store_views));
        presentation.set_store_page(self.catalog.saved.store_page as i32);
        presentation.set_store_pages((store_pages + usize::from(self.catalog.store_has_more)) as i32);
        let all_label = i18n::text("cat_all_recommendations");
        let mut categories = vec![CategoryItem { id: "all".into(), label: all_label.clone().into(), selected: self.catalog.saved.category == "all" }];
        categories.extend(self.catalog.categories.iter().filter(|category| category.id != "all").take(11).map(|category| CategoryItem {
            id: category.id.clone().into(), label: category.name.clone().into(), selected: category.id == self.catalog.saved.category,
        }));
        presentation.set_categories(model(categories));
        presentation.set_category_label(self.catalog.saved.category_label.clone().into());
        let category_key = if self.options.offline { "cat_saved_recommendations" }
            else if self.catalog.saved.category == "all" { "cat_recommendation_count" } else { "cat_category_count" };
        presentation.set_category_meta(i18n::format(category_key, &[("count", self.catalog.store_comics.len().to_string())]).into());
        if self.is_scribe() && !self.options.offline && !self.catalog.store_comics.is_empty() {
            presentation.set_category_meta(format!("{} · {}", i18n::format(category_key, &[("count", self.catalog.store_comics.len().to_string())]), i18n::text("cat_saved_browsable")).into());
        }
        presentation.set_category_picker_open(self.catalog.category_picker);
        Ok(())
    }

    fn refresh_catalog_search_view(&mut self, ui: &AppWindow) -> Result<()> {
        let presentation = ui.global::<CatalogModel>();
        presentation.set_query(self.catalog.input.query.clone().into());
        presentation.set_composition(self.catalog.input.composition.clone().into());
        presentation.set_keyboard_visible(self.catalog.keyboard);
        presentation.set_search_state(if self.catalog.search_loading { "loading" } else if self.catalog.keyboard { "recent" } else { "results" }.into());
        if self.catalog.keyboard {
            self.refresh_catalog_search_input(ui)
        } else {
            self.refresh_catalog_search_results(ui)
        }
    }

    fn refresh_catalog_search_input(&mut self, ui: &AppWindow) -> Result<()> {
        self.catalog.input.set_candidate_limit(if self.is_scribe() { 7 } else { 6 });
        let presentation = ui.global::<CatalogModel>();
        let candidate_pool = self.catalog.input.all_candidates();
        let candidate_limit = self.catalog.input.candidate_limit();
        presentation.set_candidates(model(candidate_pool.iter().skip(self.catalog.input.candidate_page.saturating_mul(candidate_limit))
            .take(candidate_limit).map(|candidate| candidate.text.clone().into()).collect()));
        presentation.set_candidate_page(self.catalog.input.candidate_page.try_into().unwrap_or(i32::MAX));
        presentation.set_candidate_pages(candidate_pool.len().div_ceil(candidate_limit).max(1).try_into().unwrap_or(i32::MAX));
        presentation.set_latin(self.catalog.input.language == InputLanguage::English);
        presentation.set_numeric(self.catalog.input.language == InputLanguage::Numeric);
        presentation.set_history(model(self.catalog.history.iter().take(if self.is_scribe() { 5 } else { 4 }).map(|entry| SearchHistoryItem {
            query: entry.query.clone().into(), meta: entry.count.map(|count| i18n::format("cat_comic_count", &[("count", count.to_string())])).unwrap_or_default().into(),
        }).collect()));
        let suggestion_query = self.catalog.input.query.trim().to_lowercase();
        let suggestions: Vec<Comic> = if suggestion_query.is_empty() { Vec::new() } else {
            self.catalog.comics.iter().filter(|comic| matches_query(comic, &suggestion_query)).take(6).cloned().collect()
        };
        let suggestion_views = suggestions.iter().map(|comic| self.comic_view(comic)).collect();
        presentation.set_suggestions(model(suggestion_views));
        Ok(())
    }

    fn refresh_catalog_search_results(&mut self, ui: &AppWindow) -> Result<()> {
        let presentation = ui.global::<CatalogModel>();
        let search_per = if self.is_scribe() { 5 } else { 5 };
        let search_pages = page_count(self.catalog.results.len(), search_per);
        self.catalog.search_page = self.catalog.search_page.clamp(1, search_pages);
        let search_comics = page_slice(&self.catalog.results, self.catalog.search_page, search_per);
        let mut search_views = Vec::with_capacity(search_comics.len());
        for comic in search_comics {
            let mut item = self.comic_view(&comic);
            item.subtitle = if self.is_scribe() { join_nonempty(&[&comic.author, &comic.genre, &self.status_only(&comic)]) }
                else { join_nonempty(&[&comic.author, &comic.genre]) }.into();
            item.meta = self.status_only(&comic).into();
            item.read_label = self.continue_label(&comic).into();
            item.in_shelf = self.catalog.shelf_ids.contains(&comic.id);
            search_views.push(item);
        }
        presentation.set_search_items(model(search_views));
        presentation.set_search_page(self.catalog.search_page as i32);
        presentation.set_search_pages((search_pages + usize::from(self.catalog.search_has_more)) as i32);
        presentation.set_search_result_label(i18n::format(if self.catalog.search_has_more { "cat_search_loaded_label" } else { "cat_search_result_label" }, &[
            ("query", self.catalog.submitted_query.clone()), ("count", self.catalog.results.len().to_string()),
        ]).into());
        presentation.set_empty_search_title(i18n::format("cat_no_results_query", &[("query", self.catalog.submitted_query.clone())]).into());
        Ok(())
    }

    fn refresh_catalog_detail_view(&mut self, ui: &AppWindow) -> Result<()> {
        let presentation = ui.global::<CatalogModel>();
        if let Some(detail) = self.catalog.detail.clone() {
            let mut item = self.comic_view(&detail.comic);
            if self.demo.is_some() {
                let variant = self.data_dir.join("fixtures").join("covers").join(format!("cover-{}-detail.png", detail.comic.id));
                if variant.is_file() { item.cover = super::load_image(&variant); }
            }
            item.subtitle = join_nonempty(&[&detail.comic.author, &detail.comic.genre]).into();
            item.meta = if self.demo.is_some() { i18n::format("cat_updating_to", &[("episode", display_order(detail.comic.latest_order))]) }
                else { self.status_only(&detail.comic) }.into();
            item.read_label = self.continue_label(&detail.comic).into();
            item.in_shelf = self.catalog.shelf_ids.contains(&detail.comic.id);
            presentation.set_detail(item);
            presentation.set_detail_genres(model(detail.comic.genre.split(['/', '·']).map(str::trim).filter(|genre| !genre.is_empty()).take(2).map(Into::into).collect()));
            presentation.set_detail_id_label(i18n::format("cat_comic_id", &[("id", detail.comic.id.clone())]).into());
            presentation.set_detail_total(detail.episodes.len() as i32);
            let episode_rows = self.filtered_episodes(&detail)?;
            let per = if self.is_scribe() { 9 } else { 7 };
            let pages = page_count(episode_rows.len(), per);
            self.catalog.detail_page = self.catalog.detail_page.clamp(1, pages);
            let rows = page_slice(&episode_rows, self.catalog.detail_page, per).iter().map(|episode| self.episode_view(&detail, episode)).collect::<Result<Vec<_>>>()?;
            presentation.set_episodes(model(rows));
            presentation.set_episode_page(self.catalog.detail_page as i32);
            presentation.set_episode_pages(pages as i32);
        } else {
            self.catalog.detail_page = 1;
            presentation.set_detail(ComicItem::default());
            presentation.set_episodes(model(Vec::new()));
            presentation.set_detail_id_label("".into());
            presentation.set_detail_total(0);
            presentation.set_detail_genres(model(Vec::new()));
            presentation.set_episode_page(1);
            presentation.set_episode_pages(1);
        }
        presentation.set_episode_filter(self.catalog.detail_filter.clone().into());
        presentation.set_episode_descending(self.catalog.detail_descending);
        presentation.set_download_selecting(self.catalog.selecting);
        presentation.set_selection_count(self.catalog.selected.len() as i32);
        presentation.set_selection_size(self.selection_size_label()?.into());
        Ok(())
    }

    pub(super) fn catalog_action(&mut self, action: &str, value: &str) -> Result<()> {
        if matches!(action, "detail.download" | "detail.select-page" | "episode.select") {
            self.catalog.auth_detail_pending = None;
        }
        match action {
            "navigate" => {
                if value == "login" { self.auth_return = self.route.clone(); }
                self.navigate(value);
            }
            "comic.open" => {
                if self.catalog_tile_network_required(value) && !self.require_network("catalog", action, value) { return Ok(()); }
                self.open_detail(value)?;
            }
            "comic.cover" => {
                if self.catalog_tile_network_required(value) && !self.require_network("catalog", action, value) { return Ok(()); }
                let comic = self.catalog.comics.iter().find(|comic| comic.id == value).cloned().or(self.store.comic(&self.account, value)?);
                let has_history = self.store.load_anchor(&self.account, value)?.is_some()
                    || comic.as_ref().is_some_and(|comic| comic.last_episode_id.is_some()
                        || comic.extra["read_episode"].as_f64().is_some_and(|episode| episode > 0.0));
                if has_history { self.open_reader_for_comic(value)?; } else { self.open_detail(value)?; }
            }
            "comic.read" => self.open_reader_for_comic(value)?,
            "shelf.menu" => self.catalog.shelf_menu = match value { "close" => false, "open" => true, _ => !self.catalog.shelf_menu },
            "shelf.view" if ["grid", "list"].contains(&value) => {
                self.catalog.saved.view = value.into(); self.catalog.saved.shelf_page = 1; self.catalog.shelf_menu = false; self.persist_catalog_view()?;
            }
            "shelf.filter" => {
                let filter = match value { "upd" => "updated", "dl" => "downloaded", "done" => "finished", other => other };
                if ["all", "updated", "downloaded", "finished"].contains(&filter) {
                    self.catalog.saved.filter = filter.into(); self.catalog.saved.shelf_page = 1; self.catalog.shelf_menu = false; self.persist_catalog_view()?;
                }
            }
            "shelf.sync" => { self.catalog.shelf_menu = false; self.sync_shelf(); }
            "shelf.page" => {
                let count = self.visible_shelf_count()?;
                let pages = page_count(count, self.shelf_per_page());
                if page_delta(value) > 0 && self.catalog.saved.shelf_page >= pages && self.catalog.shelf_has_more { self.load_more_shelf(); }
                else { self.catalog.saved.shelf_page = change_page(self.catalog.saved.shelf_page, value, pages); self.persist_catalog_view()?; }
            }
            "store.picker" => self.catalog.category_picker = match value { "close" => false, "open" => true, _ => !self.catalog.category_picker },
            "store.category" => {
                let category = if value == "all" { Some(("all".to_owned(), i18n::text("cat_all_recommendations"))) }
                    else { self.catalog.categories.iter().find(|entry| entry.id == value).map(|entry| (entry.id.clone(), entry.name.clone())) };
                if let Some((id, label)) = category {
                    let cached = self.store.load_catalog(&self.account, &store_scope(&id))?;
                    if self.options.offline && cached.is_empty() && !self.require_network("catalog", action, value) { return Ok(()); }
                    self.catalog.saved.category = id; self.catalog.saved.category_label = label; self.catalog.saved.store_page = 1;
                    self.catalog.category_picker = false; self.catalog.store_comics = cached;
                    self.catalog.store_has_more = false; self.catalog.store_loading = false;
                    self.invalidate("catalog:store"); self.persist_catalog_view()?; self.load_store();
                }
            }
            "store.page" => {
                let pages = page_count(self.catalog.store_comics.len(), if self.is_scribe() { 15 } else { 6 });
                if page_delta(value) > 0 && self.catalog.saved.store_page >= pages && self.catalog.store_has_more { self.load_more_category(); }
                else { self.catalog.saved.store_page = change_page(self.catalog.saved.store_page, value, pages); self.persist_catalog_view()?; }
            }
            "search.open" => {
                if !self.require_network("catalog", action, value) { return Ok(()); }
                self.catalog.keyboard = true; self.catalog.id_input = false; self.catalog.status.clear(); self.navigate("search");
            }
            "search.keyboard" => {
                if value == "show" { self.catalog.keyboard = true; self.invalidate("catalog:search"); self.catalog.search_loading = false; }
                else if !self.catalog.input.query.trim().is_empty() || !self.catalog.input.composition.is_empty() {
                    self.catalog.input.commit_raw(); self.run_catalog_search()?;
                }
            }
            "search.key" => {
                let key = match value { "back" => "backspace", "num" => "numeric", "lang" => "language", other => other };
                if self.catalog.input.query.len() < 300 || matches!(key, "backspace" | "clear" | "enter" | "submit") {
                    if self.catalog.input.press(key) == InputIntent::Submit { self.run_catalog_search()?; }
                }
            }
            "search.candidate" => {
                if let Some(candidate) = value.parse::<usize>().ok().and_then(|index| self.catalog.input.candidates().get(index).cloned()) { self.catalog.input.choose(&candidate); }
            }
            "search.candidate-page" => {
                self.catalog.input.press(if page_delta(value) < 0 { "candidate-prev" } else { "candidate-next" });
            }
            "search.clear" => {
                self.catalog.input.clear(); self.catalog.keyboard = true; self.catalog.id_input = false;
                self.catalog.status.clear(); self.catalog.search_loading = false; self.catalog.search_has_more = false;
                self.catalog.search_page = 1; self.catalog.results.clear(); self.catalog.submitted_query.clear();
                self.invalidate("catalog:search");
            }
            "search.history" => { self.catalog.input.set_query(value); self.catalog.id_input = false; self.run_catalog_search()?; }
            "search.history.clear" => { self.catalog.history.clear(); self.persist_search_history()?; }
            "search.by-id" => {
                if is_comic_id_query(self.catalog.input.query.trim()) {
                    let id = self.catalog.input.query.clone(); self.open_detail(&id)?;
                } else {
                    self.catalog.id_input = true; self.catalog.input.clear(); self.catalog.input.language = InputLanguage::Numeric;
                    self.catalog.keyboard = true; self.toast("cat_enter_comic_id");
                }
            }
            "search.page" => {
                let pages = page_count(self.catalog.results.len(), 5);
                if page_delta(value) > 0 && self.catalog.search_page >= pages && self.catalog.search_has_more { self.load_more_search(); }
                else { self.catalog.search_page = change_page(self.catalog.search_page, value, pages); }
            }
            "detail.back" => self.go_back()?,
            "detail.favorite" => self.change_catalog_favorite(value)?,
            "detail.download" => match value {
                "cancel" => { self.catalog.selecting = false; self.catalog.selected.clear(); }
                "confirm" => {
                    if self.catalog.selected.is_empty() || !self.require_session() || !self.require_network("catalog", action, value) { return Ok(()); }
                    let selected: Vec<_> = self.catalog.selected.iter().cloned().collect();
                    let available: BTreeSet<_> = self.catalog.detail.as_ref().map(|detail| detail.episodes.iter().map(|episode| episode.id.clone()).collect()).unwrap_or_default();
                    for id in selected {
                        if !available.contains(&id) { self.toast("cat_selection_changed"); return Ok(()); }
                        if !self.try_enqueue_episode(&id, true)? { return Ok(()); }
                        self.catalog.selected.remove(&id);
                    }
                    if self.catalog.selected.is_empty() { self.catalog.selecting = false; self.navigate("downloads"); }
                }
                _ => {
                    if self.require_network("catalog", action, value) && self.catalog.detail.is_some() {
                        self.catalog.selecting = true; self.catalog.selected.clear();
                    }
                }
            },
            "detail.select-page" => {
                if let Some(detail) = &self.catalog.detail {
                    let rows = self.filtered_episodes(detail)?;
                    for episode in page_slice(&rows, self.catalog.detail_page, if self.is_scribe() { 9 } else { 7 }) {
                        if episode.readable_at(now()) { self.catalog.selected.insert(episode.id); }
                    }
                }
            }
            "detail.locate" => self.locate_catalog_episode()?,
            "episode.filter" => {
                let filter = if value == "dl" { "downloaded" } else { value };
                if ["all", "unread", "readable", "downloaded"].contains(&filter) { self.catalog.detail_filter = filter.into(); self.catalog.detail_page = 1; }
            }
            "episode.sort" => { self.catalog.detail_descending = !self.catalog.detail_descending; self.catalog.detail_page = 1; }
            "episode.page" => {
                if let Some(detail) = &self.catalog.detail {
                    let pages = page_count(self.filtered_episodes(detail)?.len(), if self.is_scribe() { 9 } else { 7 });
                    self.catalog.detail_page = change_page(self.catalog.detail_page, value, pages);
                }
            }
            "episode.select" => {
                if self.catalog.detail.as_ref().and_then(|detail| detail.episodes.iter().find(|episode| episode.id == value)).is_some_and(|episode| episode.readable_at(now())) {
                    if !self.catalog.selected.remove(value) { self.catalog.selected.insert(value.into()); }
                }
            }
            "episode.open" => {
                if let Some(episode) = self.catalog.detail.as_ref().and_then(|detail| detail.episodes.iter().find(|episode| episode.id == value)).cloned() {
                    if episode.readable_at(now()) { self.open_episode(&episode.comic_id, &episode.id)?; }
                    else if matches!(episode.access, EpisodeAccess::Locked | EpisodeAccess::Temporary) {
                        if self.require_network("catalog", action, value) && self.require_session() { self.open_purchase(&episode.id)?; }
                    } else { self.toast("cat_access_unknown"); }
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn sync_shelf(&mut self) {
        if self.demo.is_some() { self.catalog.last_sync = now(); self.toast("cat_shelf_synced"); return; }
        if self.catalog.shelf_syncing || !self.require_network("catalog", "shelf.sync", "") || !self.require_session() { return; }
        self.catalog.shelf_syncing = true;
        self.catalog.status.clear();
        let client = self.client.clone();
        self.source_job("catalog:shelf", async move {
            let favorites = client.list_favorites(1, SHELF_BATCH).await?;
            let history = client.list_history(1, 100).await;
            Ok((favorites, history))
        }, |app, result| {
            app.catalog.shelf_syncing = false;
            match result {
                Ok((favorites, history)) => {
                    app.catalog.shelf_has_more = favorites.len() == SHELF_BATCH as usize;
                    app.catalog.shelf_server_page = 1;
                    app.catalog.shelf_ids = favorites.iter().map(|comic| comic.id.clone()).collect();
                    if let Ok(mut known) = app.store.load_comics(&app.account) {
                        for comic in &mut known { comic.favorite = app.catalog.shelf_ids.contains(&comic.id); }
                        if app.store.save_comics(&app.account, &known).is_err() { app.toast("common_storage_error"); return; }
                    }
                    let stored = app.store.save_catalog(&app.account, "shelf", &favorites);
                    if stored.is_err() { app.toast("common_storage_error"); return; }
                    if app.store.set_preference(&app.account, "catalog_shelf_initialized", &true).is_err() { app.toast("common_storage_error"); }
                    match history {
                        Ok(mut history) => {
                            for comic in &mut history { comic.favorite = app.catalog.shelf_ids.contains(&comic.id); }
                            if app.store.save_catalog(&app.account, "history", &history).is_err() { app.toast("common_storage_error"); }
                            app.catalog.history_comics = history;
                        }
                        Err(error) => app.source_failure(&error),
                    }
                    match app.store.load_comics(&app.account) { Ok(comics) => app.catalog.comics = comics, Err(_) => app.toast("common_storage_error") }
                    app.catalog.last_sync = now();
                    if app.store.set_preference(&app.account, "catalog_last_sync", &app.catalog.last_sync).is_err() { app.toast("common_storage_error"); }
                    app.trim_catalog_cache();
                }
                Err(error) => { app.catalog.status = i18n::text("cat_sync_failed_cached"); app.source_failure(&error); }
            }
        });
    }

    pub(super) fn load_store(&mut self) {
        if let Some(demo) = &self.demo {
            let selected = self.catalog.saved.category.clone();
            let genre = self.catalog.categories.iter().find(|category| category.id == selected).map(|category| category.name.clone());
            let mut comics: Vec<_> = demo.comics.iter().filter(|comic| comic.extra["store_order"].as_u64().is_some()).cloned().collect();
            comics.sort_by_key(|comic| comic.extra["store_order"].as_u64().unwrap_or(u64::MAX));
            comics.truncate(if self.is_scribe() { 20 } else { 18 });
            if selected != "all" { comics.retain(|comic| genre.as_ref().is_some_and(|genre| comic.genre.contains(genre))); }
            self.catalog.store_comics = comics; self.catalog.store_has_more = false;
            return;
        }
        if self.options.offline || self.catalog.store_loading { return; }
        if self.catalog.categories.is_empty() && !self.catalog.category_loading {
            self.catalog.category_loading = true;
            let client = self.client.clone();
            self.source_job("catalog:categories", async move { client.categories().await }, |app, result| {
                app.catalog.category_loading = false;
                match result {
                    Ok(CategoryMetadata { styles, .. }) => {
                        app.catalog.categories = styles;
                        if app.store.set_preference(&app.account, "catalog_categories", &app.catalog.categories).is_err() { app.toast("common_storage_error"); }
                    }
                    Err(error) => app.source_failure(&error),
                }
            });
        }
        self.catalog.store_loading = true;
        let client = self.client.clone();
        let category = self.catalog.saved.category.clone();
        let requested = category.clone();
        self.source_job("catalog:store", async move {
            if requested == "all" { client.recommendations().await.map(|comics| (comics, false)) }
            else { client.category_page(&requested, 0, 1).await.map(|page| (page.comics, page.has_more)) }
        }, move |app, result| {
            app.catalog.store_loading = false;
            if app.catalog.saved.category != category { return; }
            match result {
                Ok((mut comics, has_more)) => {
                    for comic in &mut comics { comic.favorite = app.catalog.shelf_ids.contains(&comic.id); }
                    if app.store.save_catalog(&app.account, &store_scope(&category), &comics).is_err() { app.toast("common_storage_error"); return; }
                    merge_comics(&mut app.catalog.comics, &comics);
                    app.catalog.store_comics = comics; app.catalog.store_has_more = has_more; app.catalog.store_server_page = 1;
                    app.trim_catalog_cache();
                }
                Err(error) => { app.catalog.status = i18n::text("cat_store_failed_cached"); app.source_failure(&error); }
            }
        });
    }

    pub(super) fn open_detail(&mut self, value: &str) -> Result<()> {
        let Some(id) = normalize_comic_id(value) else { self.toast("cat_invalid_comic_id"); return Ok(()); };
        if self.catalog.auth_detail_pending.as_ref().is_some_and(|intent| intent.comic_id != id) {
            self.catalog.auth_detail_pending = None;
        }
        self.catalog.selecting = false; self.catalog.selected.clear(); self.catalog.detail_filter = "all".into(); self.catalog.detail_descending = false;
        self.catalog.status.clear();
        let cached = self.demo.as_ref().and_then(|demo| demo.details.iter().find(|detail| detail.comic.id == id).cloned())
            .or(self.store.load_detail(&self.account, &id)?);
        self.catalog.detail = cached.or_else(|| self.catalog.comics.iter().find(|comic| comic.id == id).cloned().map(|comic| ComicDetail { comic, ..Default::default() }));
        self.catalog.detail_page = 1;
        self.locate_catalog_episode()?;
        self.navigate("detail");
        if self.demo.is_some() { return Ok(()); }
        if self.options.offline && self.catalog.detail.as_ref().is_some_and(|detail| !detail.episodes.is_empty()) { return Ok(()); }
        if !self.require_network("catalog", "comic.open", &id) { return Ok(()); }
        self.catalog.detail_loading = true;
        let client = self.client.clone();
        let requested = id.clone();
        self.source_job("catalog:detail", async move { client.comic_detail(&requested).await }, move |app, result| {
            app.catalog.detail_loading = false;
            match result {
                Ok(mut detail) => {
                    detail.comic.favorite = app.catalog.shelf_ids.contains(&detail.comic.id);
                    if app.store.save_detail(&app.account, &detail).is_err() {
                        app.catalog.status = i18n::text("cat_detail_failed_cached");
                        app.toast("common_storage_error"); return;
                    }
                    merge_comics(&mut app.catalog.comics, std::slice::from_ref(&detail.comic));
                    if app.catalog.detail.as_ref().is_none_or(|current| current.comic.id == id) {
                        app.catalog.detail = Some(detail);
                        if app.locate_catalog_episode().is_err() { app.toast("common_storage_error"); }
                        app.catalog.status.clear();
                        app.apply_catalog_auth_return();
                    }
                    app.request_catalog_local_refresh();
                    app.trim_catalog_cache();
                }
                Err(error) => { app.catalog.status = i18n::text("cat_detail_failed_cached"); app.source_failure(&error); }
            }
        });
        Ok(())
    }

    fn run_catalog_search(&mut self) -> Result<()> {
        let query = self.catalog.input.query.trim().to_owned();
        if query.is_empty() { return Ok(()); }
        if self.catalog.id_input {
            let Some(id) = normalize_comic_id(&query) else { self.toast("cat_invalid_comic_id"); return Ok(()); };
            self.catalog.id_input = false;
            return self.open_detail(&id);
        }
        if is_comic_id_query(&query) { return self.open_detail(&query); }
        if !self.require_network("catalog", "search.key", "enter") { return Ok(()); }
        if query.len() > 300 { self.toast("cat_search_too_long"); return Ok(()); }
        self.catalog.keyboard = false; self.catalog.search_page = 1; self.catalog.submitted_query = query.clone(); self.catalog.status.clear();
        self.catalog.history.retain(|entry| entry.query != query);
        self.catalog.history.insert(0, HistoryEntry { query: query.clone(), count: None });
        self.catalog.history.truncate(20); self.persist_search_history()?;
        if let Some(demo) = &self.demo {
            let normalized = query.to_lowercase();
            let mut matches: Vec<_> = demo.comics.iter().filter(|comic| matches_query(comic, &normalized)).cloned().collect();
            matches.sort_by_key(|comic| comic.extra["search_order"].as_u64().or(comic.extra["store_order"].as_u64()).unwrap_or(u64::MAX));
            self.catalog.results = matches; self.catalog.search_has_more = false; self.finish_search_history()?;
            return Ok(());
        }
        self.catalog.results = self.store.load_catalog(&self.account, &search_scope(&query))?;
        self.catalog.search_loading = true;
        let client = self.client.clone();
        let requested = query.clone();
        self.source_job("catalog:search", async move { client.search(&requested, 1, SEARCH_BATCH).await }, move |app, result| {
            app.catalog.search_loading = false;
            if app.catalog.submitted_query != query { return; }
            match result {
                Ok(mut comics) => {
                    for comic in &mut comics { comic.favorite = app.catalog.shelf_ids.contains(&comic.id); }
                    if app.store.save_catalog(&app.account, &search_scope(&query), &comics).is_err() { app.toast("common_storage_error"); return; }
                    app.catalog.search_has_more = comics.len() == SEARCH_BATCH as usize;
                    app.catalog.search_server_page = 1;
                    merge_comics(&mut app.catalog.comics, &comics);
                    app.catalog.results = comics;
                    if app.finish_search_history().is_err() { app.toast("common_storage_error"); }
                    app.trim_catalog_cache();
                }
                Err(error) => { app.catalog.status = i18n::text("cat_search_failed_cached"); app.source_failure(&error); }
            }
        });
        Ok(())
    }

    fn load_more_shelf(&mut self) {
        if self.catalog.shelf_syncing || self.catalog.shelf_server_page >= MAX_SHELF_BATCHES || !self.require_network("catalog", "shelf.page", "1") { return; }
        self.catalog.shelf_syncing = true;
        let page = self.catalog.shelf_server_page + 1;
        let origin = self.catalog.saved.shelf_page;
        let client = self.client.clone();
        self.source_job("catalog:shelf", async move { client.list_favorites(page, SHELF_BATCH).await }, move |app, result| {
            app.catalog.shelf_syncing = false;
            match result {
                Ok(comics) => {
                    let mut shelf = match app.store.load_catalog(&app.account, "shelf") {
                        Ok(shelf) => shelf,
                        Err(_) => { app.toast("common_storage_error"); return; }
                    };
                    merge_comics(&mut shelf, &comics);
                    if app.store.save_catalog(&app.account, "shelf", &shelf).is_err() { app.toast("common_storage_error"); return; }
                    app.catalog.shelf_has_more = comics.len() == SHELF_BATCH as usize && page < MAX_SHELF_BATCHES;
                    app.catalog.shelf_server_page = page;
                    app.catalog.shelf_ids = shelf.iter().map(|comic| comic.id.clone()).collect();
                    merge_comics(&mut app.catalog.comics, &comics);
                    if app.catalog.saved.shelf_page == origin { app.catalog.saved.shelf_page = origin + 1; }
                    if app.persist_catalog_view().is_err() { app.toast("common_storage_error"); }
                    app.trim_catalog_cache();
                }
                Err(error) => app.source_failure(&error),
            }
        });
    }

    fn load_more_category(&mut self) {
        if self.catalog.store_loading || self.catalog.saved.category == "all" || self.catalog.store_server_page >= 5 || !self.require_network("catalog", "store.page", "1") { return; }
        self.catalog.store_loading = true;
        let page = self.catalog.store_server_page + 1;
        let origin = self.catalog.saved.store_page;
        let category = self.catalog.saved.category.clone();
        let request_category = category.clone();
        let client = self.client.clone();
        self.source_job("catalog:store", async move { client.category_page(&request_category, 0, page).await }, move |app, result| {
            app.catalog.store_loading = false;
            if app.catalog.saved.category != category { return; }
            match result {
                Ok(result) => {
                    app.catalog.store_has_more = result.has_more && page < 5; app.catalog.store_server_page = page;
                    merge_comics(&mut app.catalog.store_comics, &result.comics); merge_comics(&mut app.catalog.comics, &result.comics);
                    if app.store.save_catalog(&app.account, &store_scope(&category), &app.catalog.store_comics).is_err() { app.toast("common_storage_error"); return; }
                    if app.catalog.saved.store_page == origin { app.catalog.saved.store_page = origin + 1; }
                    if app.persist_catalog_view().is_err() { app.toast("common_storage_error"); }
                    app.trim_catalog_cache();
                }
                Err(error) => app.source_failure(&error),
            }
        });
    }

    fn load_more_search(&mut self) {
        if self.catalog.search_loading || !self.require_network("catalog", "search.page", "1") { return; }
        self.catalog.search_loading = true;
        let page = self.catalog.search_server_page + 1;
        let origin = self.catalog.search_page;
        let query = self.catalog.submitted_query.clone();
        let requested = query.clone();
        let client = self.client.clone();
        self.source_job("catalog:search", async move { client.search(&requested, page, SEARCH_BATCH).await }, move |app, result| {
            app.catalog.search_loading = false;
            if app.catalog.submitted_query != query { return; }
            match result {
                Ok(comics) => {
                    app.catalog.search_has_more = comics.len() == SEARCH_BATCH as usize; app.catalog.search_server_page = page;
                    merge_comics(&mut app.catalog.results, &comics); merge_comics(&mut app.catalog.comics, &comics);
                    if app.store.save_catalog(&app.account, &search_scope(&query), &app.catalog.results).is_err() { app.toast("common_storage_error"); return; }
                    if app.catalog.search_page == origin { app.catalog.search_page = origin + 1; }
                    if app.finish_search_history().is_err() { app.toast("common_storage_error"); }
                    app.trim_catalog_cache();
                }
                Err(error) => app.source_failure(&error),
            }
        });
    }

    fn change_catalog_favorite(&mut self, id: &str) -> Result<()> {
        let id = if id.is_empty() { self.catalog.detail.as_ref().map(|detail| detail.comic.id.as_str()).unwrap_or_default() } else { id }.to_owned();
        if id.is_empty() || self.catalog.favorite_pending.contains(&id) { return Ok(()); }
        let favorite = !self.catalog.shelf_ids.contains(&id);
        if self.demo.is_some() { return self.apply_catalog_favorite(&id, favorite); }
        if !self.require_network("catalog", "detail.favorite", &id) || !self.require_session() { return Ok(()); }
        let client = self.client.bound_to_account(&self.account)?;
        self.catalog.favorite_pending.insert(id.clone());
        let requested = id.clone();
        self.source_job(&format!("catalog:favorite:{id}"), async move { client.set_favorite(&requested, favorite).await }, move |app, result| {
            app.catalog.favorite_pending.remove(&id);
            match result {
                Ok(()) => if app.apply_catalog_favorite(&id, favorite).is_err() { app.toast("common_storage_error"); },
                Err(error) => app.source_failure(&error),
            }
        });
        Ok(())
    }

    fn apply_catalog_favorite(&mut self, id: &str, favorite: bool) -> Result<()> {
        if favorite { self.catalog.shelf_ids.insert(id.into()); } else { self.catalog.shelf_ids.remove(id); }
        for comic in self.catalog.comics.iter_mut().chain(self.catalog.store_comics.iter_mut()).chain(self.catalog.results.iter_mut()) {
            if comic.id == id { comic.favorite = favorite; }
        }
        if let Some(detail) = &mut self.catalog.detail {
            if detail.comic.id == id { detail.comic.favorite = favorite; self.store.save_detail(&self.account, detail)?; }
        }
        let shelf: Vec<_> = self.catalog.comics.iter().filter(|comic| self.catalog.shelf_ids.contains(&comic.id)).cloned().collect();
        if let Some(comic) = self.catalog.comics.iter().find(|comic| comic.id == id) { self.store.save_comic(&self.account, comic)?; }
        self.store.save_catalog(&self.account, "shelf", &shelf)?;
        self.store.set_preference(&self.account, "catalog_shelf_initialized", &true)?;
        self.toast(if favorite { "cat_follow_success" } else { "cat_unfollow_success" });
        self.trim_catalog_cache();
        Ok(())
    }

    fn shelf_per_page(&self) -> usize {
        match (self.is_scribe(), self.catalog.saved.view.as_str()) { (true, "list") => 6, (true, _) => 10, (false, "list") => 7, _ => 6 }
    }

    fn visible_shelf_count(&self) -> Result<usize> {
        let resume = self.catalog_resume_id()?;
        Ok(self.catalog.comics.iter().filter(|comic| self.catalog.shelf_ids.contains(&comic.id))
            .filter(|comic| self.demo.is_none() || self.is_scribe() || comic.extra["shelf_order"].as_u64().is_some_and(|order| order < 12))
            .filter(|comic| !self.is_scribe() || Some(&comic.id) != resume.as_ref())
            .filter(|comic| match self.catalog.saved.filter.as_str() {
            "updated" => comic.extra["updated"].as_bool().unwrap_or(false),
            "downloaded" => self.comic_has_download(comic), "finished" => comic.finished,
            _ => Some(&comic.id) != resume.as_ref(),
        }).count())
    }

    fn catalog_resume_id(&self) -> Result<Option<String>> {
        Ok(self.store.latest_anchor(&self.account)?.map(|(comic, _)| comic)
            .or_else(|| self.demo.as_ref().and_then(|demo| demo.fixture["continue_reading"]["comic_id"].as_u64().map(|id| id.to_string())))
            .or_else(|| self.catalog.history_comics.iter().find(|comic| comic.last_episode_id.is_some()).map(|comic| comic.id.clone())))
    }

    fn shelf_comic_view(&mut self, comic: &Comic) -> ComicItem {
        let mut view = self.comic_view(comic);
        view.meta = self.reading_label(comic, self.catalog.saved.view == "list").into();
        view.in_shelf = self.catalog.shelf_ids.contains(&comic.id);
        view.downloaded = self.comic_has_download(comic);
        if !self.is_scribe() && self.options.offline && !self.online {
            let count = self.catalog.local_snapshot.episode_comics.iter().filter(|(episode, owner)| *owner == &comic.id
                && self.catalog.local_snapshot.episodes.get(*episode).is_some_and(|local| local.retained)).count();
            view.meta = if !self.catalog.local_snapshot.checked_all { i18n::text("cat_local_download_checking") }
                else if count > 0 { i18n::format("cat_offline_chapters", &[("count", count.to_string())]) }
                else if view.downloaded { i18n::text("cat_offline_available") } else { i18n::text("cat_network_required") }.into();
            if self.demo.is_some() && comic.extra["offline_cover_cached_pw"].as_bool() == Some(false) { view.cover = slint::Image::default(); }
        }
        let order = self.store.load_anchor(&self.account, &comic.id).ok().flatten()
            .and_then(|anchor| self.store.load_detail(&self.account, &comic.id).ok().flatten()
                .and_then(|detail| detail.episodes.iter().find(|episode| episode.id == anchor.episode_id).map(|episode| episode.order)))
            .or_else(|| comic.extra["read_episode"].as_f64());
        if let Some(order) = order.filter(|_| comic.chapter_count > 0) { view.progress = (order / f64::from(comic.chapter_count)).clamp(0.0, 1.0) as f32; }
        view
    }

    fn resume_comic_view(&mut self, comic: &Comic) -> Result<ComicItem> {
        let mut view = self.comic_view(comic);
        view.read_label = i18n::text(if self.demo.is_some() { "cat_demo_resume_time" } else { "cat_continue_reading" }).into();
        view.meta = self.reading_label(comic, false).into();
        view.subtitle = self.reading_label(comic, false).into();
        if let Some(anchor) = self.store.load_anchor(&self.account, &comic.id)? {
            let detail = self.store.load_detail(&self.account, &comic.id)?;
            let episode = detail.as_ref().and_then(|detail| detail.episodes.iter().find(|episode| episode.id == anchor.episode_id));
            if let Some(episode) = episode {
                view.subtitle = i18n::format("cat_chapter_title", &[("episode", display_order(episode.order)), ("title", episode.title.clone())]).into();
                let count = self.store.load_index(&self.account, &episode.id, Some(&anchor.revision))?.map(|index| index.images.len());
                if self.is_scribe() {
                    view.meta = i18n::format("cat_page_count", &[("page", (anchor.page_index + 1).to_string()), ("total", count.map_or_else(|| "?".into(), |count| count.to_string()))]).into();
                }
            }
        }
        if self.demo.is_some() {
            if let Some(raw) = self.demo.as_ref().map(|demo| &demo.fixture["continue_reading"]) {
                if raw["comic_id"].as_u64().map(|id| id.to_string()) == Some(comic.id.clone()) {
                    view.description = raw["summary"].as_str().unwrap_or_default().into();
                }
            }
        }
        Ok(view)
    }

    fn reading_label(&self, comic: &Comic, list: bool) -> String {
        if self.demo.is_some() {
            let unchanged = self.store.load_anchor(&self.account, &comic.id).ok().flatten().is_none_or(|anchor| {
                comic.extra["read_page"].as_u64() == Some(anchor.page_index as u64 + 1)
                    && comic.last_episode_id.as_ref().is_some_and(|id| *id == anchor.episode_id)
            });
            if unchanged {
                if let Some(value) = comic.extra[if list { "list_read_subtitle" } else { "read_subtitle" }].as_str() { return deduplicate_chapter_total(value); }
            }
        }
        if let Ok(Some(anchor)) = self.store.load_anchor(&self.account, &comic.id) {
            let order = self.store.load_detail(&self.account, &comic.id).ok().flatten()
                .and_then(|detail| detail.episodes.iter().find(|episode| episode.id == anchor.episode_id).map(|episode| episode.order));
            let total = self.store.load_index(&self.account, &anchor.episode_id, Some(&anchor.revision)).ok().flatten().map(|index| index.images.len());
            let label = i18n::format("cat_read_position", &[("episode", order.map(display_order).unwrap_or_else(|| "?".into())),
                ("page", (anchor.page_index + 1).to_string()), ("total", total.map_or_else(|| "?".into(), |count| count.to_string()))]);
            return if list && comic.chapter_count > 0 { format!("{label} · {}", i18n::format("cat_total_chapters", &[("count", comic.chapter_count.to_string())])) } else { label };
        }
        i18n::text("common_no_reading")
    }

    fn continue_label(&self, comic: &Comic) -> String {
        let episode = self.store.load_anchor(&self.account, &comic.id).ok().flatten().map(|anchor| anchor.episode_id)
            .or_else(|| comic.last_episode_id.clone());
        let order = episode.and_then(|id| self.store.load_detail(&self.account, &comic.id).ok().flatten()
            .and_then(|detail| detail.episodes.iter().find(|episode| episode.id == id).map(|episode| episode.order)))
            .or_else(|| comic.extra["read_episode"].as_f64()).filter(|order| *order > 0.0);
        order.map(|order| i18n::format(if self.is_scribe() { "cat_continue_episode" } else { "cat_continue_episode_pw" }, &[("episode", display_order(order))]))
            .unwrap_or_else(|| i18n::text("cat_read_first"))
    }

    fn status_only(&self, comic: &Comic) -> String {
        if self.demo.is_some() && !self.is_scribe() && comic.finished && comic.chapter_count > 0 {
            return i18n::format("cat_completed_total", &[("count", comic.chapter_count.to_string())]);
        }
        if self.demo.is_some() { if let Some(status) = comic.extra["status"].as_str().filter(|status| !status.is_empty()) { return status.into(); } }
        if comic.finished { i18n::text("cat_finished") }
        else if comic.chapter_count > 0 { i18n::format("cat_serial_to", &[("count", comic.chapter_count.to_string())]) }
        else if !comic.latest_episode_title.is_empty() { comic.latest_episode_title.clone() }
        else { i18n::text("cat_unknown_update") }
    }

    fn catalog_status(&self, comic: &Comic) -> String {
        let genre = comic.genre.split(['/', '·']).map(str::trim).find(|genre| !genre.is_empty()).unwrap_or_default();
        let status = if self.is_scribe() && !comic.finished && comic.chapter_count > 0 {
            i18n::format("cat_store_serial", &[("count", comic.chapter_count.to_string())])
        } else { self.status_only(comic) };
        join_nonempty(&[genre, &status])
    }

    fn catalog_tile_network_required(&self, comic_id: &str) -> bool {
        if !self.options.offline || self.online { return false; }
        // An unfinished inventory cannot prove that cached content is missing.
        // Let the existing local detail/reader path validate the selected item.
        self.route == "store" || self.route == "shelf" && self.catalog.local_snapshot.checked_all
            && self.catalog.comics.iter()
            .find(|comic| comic.id == comic_id).is_none_or(|comic| !self.comic_has_download(comic))
    }

    fn filtered_episodes(&self, detail: &ComicDetail) -> Result<Vec<Episode>> {
        let anchor = self.store.load_anchor(&self.account, &detail.comic.id)?;
        let mut episodes: Vec<_> = detail.episodes.iter().filter(|episode| match self.catalog.detail_filter.as_str() {
            "unread" => !episode.read && anchor.as_ref().is_none_or(|anchor| anchor.episode_id != episode.id),
            "readable" => episode.readable_at(now()),
            "downloaded" => self.local_episode(episode).is_ok_and(|status| status.retained),
            _ => true,
        }).cloned().collect();
        if self.catalog.detail_descending { episodes.reverse(); }
        Ok(episodes)
    }

    fn locate_catalog_episode(&mut self) -> Result<()> {
        let Some(detail) = &self.catalog.detail else { return Ok(()); };
        let current = self.store.load_anchor(&self.account, &detail.comic.id)?.map(|anchor| anchor.episode_id)
            .or_else(|| detail.comic.last_episode_id.clone());
        self.catalog.detail_filter = "all".into(); self.catalog.detail_descending = false;
        self.catalog.detail_page = current.and_then(|id| detail.episodes.iter().position(|episode| episode.id == id))
            .map_or(1, |position| position / if self.is_scribe() { 9 } else { 7 } + 1);
        Ok(())
    }

    pub(super) fn episode_view(&self, detail: &ComicDetail, episode: &Episode) -> Result<EpisodeItem> {
        let anchor = self.store.load_anchor(&self.account, &detail.comic.id)?;
        let current = anchor.as_ref().map_or_else(|| self.demo.is_some() && episode.extra["current"].as_bool().unwrap_or(false), |anchor| anchor.episode_id == episode.id);
        let local = self.local_episode(episode)?;
        let changed_demo_position = self.demo.is_some() && current && anchor.as_ref().is_some_and(|anchor|
            episode.extra["current"].as_bool() != Some(true) || episode.extra["page_count"].as_u64().is_some()
                && detail.comic.extra["read_page"].as_u64() != Some(anchor.page_index as u64 + 1));
        let reading = if self.demo.is_some() && !changed_demo_position {
            episode.extra[if self.is_scribe() { "read_label_sc" } else { "read_label_pw" }].as_str().unwrap_or_default().to_owned()
        } else if let Some(anchor) = anchor.filter(|anchor| anchor.episode_id == episode.id) {
            i18n::format("cat_episode_read_position", &[("page", (anchor.page_index + 1).to_string()), ("total", if local.total > 0 { local.total.to_string() } else { "?".into() })])
        } else { i18n::text(if episode.read { "cat_read" } else { "cat_unread" }) };
        let access = if self.demo.is_some() { episode.extra["access_label"].as_str().map(str::to_owned) } else { None }.unwrap_or_else(|| match episode.access {
            EpisodeAccess::Free => i18n::text("cat_free"), EpisodeAccess::Owned => i18n::text("cat_owned"),
            EpisodeAccess::Temporary => episode.expires_at.and_then(|timestamp| chrono::DateTime::from_timestamp(timestamp, 0))
                .map(|date| i18n::format("cat_free_until", &[("date", (date + chrono::Duration::hours(8)).format("%m.%d").to_string())]))
                .unwrap_or_else(|| i18n::text("cat_temporary")),
            EpisodeAccess::Locked => episode.pay_gold.map(|price| i18n::format("cat_coin_price", &[("amount", display_order(price))])).unwrap_or_else(|| i18n::text("common_locked")),
            EpisodeAccess::Unavailable => i18n::text("cat_unavailable"), EpisodeAccess::Unknown => i18n::text("common_unknown"),
        });
        let storage = if self.demo.is_some() {
            episode.extra[if self.is_scribe() { "download_label_sc" } else { "download_label_pw" }].as_str().unwrap_or_default().to_owned()
        } else if !local.known { i18n::text("common_unknown") }
        else if local.retained { i18n::text("cat_downloaded") }
        else if local.present > 0 { i18n::format("cat_cached_pages", &[("count", local.present.to_string()), ("total", local.total.to_string())]) }
        else if self.is_scribe() { "—".into() } else { i18n::text("cat_not_downloaded") };
        Ok(EpisodeItem { id: episode.id.clone().into(), order: display_order(episode.order).into(), title: episode.title.clone().into(),
            reading: reading.into(), access: access.into(), storage: storage.trim_start_matches(['\u{2713}', ' ']).into(), readable: episode.readable_at(now()),
            selected: self.catalog.selected.contains(&episode.id), current, downloaded: local.retained,
            temporary: episode.access == EpisodeAccess::Temporary, locked: episode.access == EpisodeAccess::Locked })
    }

    fn local_episode(&self, episode: &Episode) -> Result<LocalEpisode> {
        Ok(self.catalog.local_snapshot.episodes.get(&episode.id).cloned().unwrap_or_default())
    }

    pub(super) fn reader_episode_downloaded(&self, episode: &Episode) -> bool {
        self.local_episode(episode).is_ok_and(|status| status.retained)
    }

    pub(super) fn comic_has_download(&self, comic: &Comic) -> bool {
        if self.demo.is_some() && !self.is_scribe() {
            if let Some(downloaded) = comic.extra["downloaded_pw"].as_bool() { return downloaded; }
        }
        self.catalog.local_snapshot.downloaded_comics.contains(&comic.id)
    }

    pub(super) fn reader_local_inventory_for_revision(&self, episode_id: &str, revision: &str) -> Option<(u32, u32, bool)> {
        self.catalog.local_snapshot.inventory_for_revision(episode_id, revision)
    }

    pub(super) fn invalidate_catalog_local_episode(&mut self, comic_id: &str, episode_id: &str) {
        if self.demo.is_some() { return; }
        self.catalog.local_snapshot.invalidate_episode(comic_id, episode_id);
        self.catalog.local_generation = self.catalog.local_generation.wrapping_add(1);
        self.catalog.local_scan = None;
        self.catalog.local_dirty = true;
        self.dirty = true;
    }

    pub(super) fn invalidate_catalog_local_state(&mut self) {
        if self.demo.is_some() { return; }
        self.catalog.local_snapshot = LocalSnapshot::default();
        self.catalog.local_full_checked = None;
        self.catalog.local_generation = self.catalog.local_generation.wrapping_add(1);
        self.catalog.local_scan = None;
        self.catalog.local_dirty = true;
        self.dirty = true;
    }

    fn request_catalog_local_refresh(&mut self) {
        if self.demo.is_some() { return; }
        self.catalog.local_snapshot.checked_all = false;
        self.catalog.local_generation = self.catalog.local_generation.wrapping_add(1);
        self.catalog.local_scan = None;
        self.catalog.local_dirty = true;
        self.dirty = true;
    }

    pub(super) fn refresh_catalog_local_snapshot(&mut self) {
        if self.catalog.local_loading && !self.pending.contains_key("catalog:inventory-step") {
            // Logout can retire every job while keeping the guest's catalog.
            // A retired continuation has no callback that could clear loading.
            self.catalog.local_loading = false;
            self.catalog.local_scan = None;
            self.catalog.local_generation = self.catalog.local_generation.wrapping_add(1);
            self.catalog.local_dirty = true;
        }
        let full_recheck_due = self.catalog.local_full_checked
            .is_none_or(|checked| checked.elapsed() >= LOCAL_SCAN_INTERVAL);
        if self.demo.is_some() || self.is_suspended() || self.catalog.local_loading
            || !self.catalog.local_dirty && self.catalog.local_scan.is_none()
                && self.catalog.local_checked.elapsed() < LOCAL_SCAN_INTERVAL
                && (self.catalog.local_failed || !full_recheck_due)
        { return; }
        let scan = self.catalog.local_scan.take().unwrap_or_else(|| {
            // Scoped invalidations keep unrelated verified chapters. A periodic
            // recheck keeps its own deadline, even when incremental work or a
            // generation change interrupts it before all chapters complete.
            self.catalog.local_scan_full = full_recheck_due;
            let skip = if self.catalog.local_dirty && !full_recheck_due {
                self.catalog.local_snapshot.episodes.iter().filter_map(|(id, local)|
                    local.known.then(|| id.clone())).collect()
            } else { BTreeSet::new() };
            let priority = self.active_reader_identity().map(|(comic, episode)| (comic.to_owned(), episode.to_owned()));
            InventoryScan::new(self.catalog.comics.iter().map(|comic| comic.id.clone()).collect(), skip, priority)
        });
        self.catalog.local_loading = true;
        let delay = LOCAL_SCAN_YIELD.saturating_sub(self.catalog.local_attempted.elapsed());
        self.catalog.local_attempted = Instant::now();
        let generation = self.catalog.local_generation;
        let full_scan = self.catalog.local_scan_full;
        let database = self.store.path().to_owned();
        let account = self.account.clone();
        self.background_after("catalog:inventory-step", delay, move || {
            scan.run(database, &account)
        }, move |app, result| {
            app.catalog.local_loading = false;
            if app.catalog.local_generation != generation {
                app.catalog.local_dirty = true;
                app.refresh_catalog_local_snapshot();
                return;
            }
            match result {
                Ok(batch) => {
                    let visible_change = !batch.completed.is_empty() || batch.complete_targets.is_some();
                    for completed in batch.completed {
                        app.catalog.local_snapshot.install_episode(completed.comic_id, completed.episode_id, completed.local);
                    }
                    if let Some(live) = batch.complete_targets {
                        app.catalog.local_snapshot.retain_episodes(&live);
                        app.catalog.local_snapshot.checked_all = true;
                    }
                    app.catalog.local_scan = batch.continuation;
                    app.catalog.local_dirty = app.catalog.local_scan.is_some();
                    app.catalog.local_failed = false;
                    if visible_change {
                        app.view_dirty.mark(View::Catalog);
                        app.view_dirty.mark(View::Reader);
                    }
                    if app.catalog.local_scan.is_none() {
                        let checked = Instant::now();
                        app.catalog.local_checked = checked;
                        if full_scan { app.catalog.local_full_checked = Some(checked); }
                        app.catalog.local_scan_full = false;
                    }
                    else { app.refresh_catalog_local_snapshot(); }
                }
                Err(_) => {
                    app.catalog.local_snapshot = LocalSnapshot::default();
                    app.catalog.local_scan = None;
                    app.catalog.local_scan_full = false;
                    app.catalog.local_full_checked = None;
                    app.catalog.local_dirty = false;
                    app.catalog.local_checked = Instant::now();
                    if !app.catalog.local_failed { app.toast("common_storage_error"); }
                    app.catalog.local_failed = true;
                    app.view_dirty.mark(View::Catalog);
                    app.view_dirty.mark(View::Reader);
                }
            }
        });
    }

    fn selection_size_label(&self) -> Result<String> {
        if self.catalog.selected.is_empty() { return Ok(i18n::text(if self.is_scribe() { "cat_select_chapters_hint" } else { "cat_cross_page_selection" })); }
        if self.demo.is_some() { return Ok(i18n::format("cat_estimated_size", &[("size", format!("{:.0}", self.catalog.selected.len() as f64 * 15.4))])); }
        let mut known_bytes = 0u64;
        let mut known_episodes = 0usize;
        if let Some(detail) = &self.catalog.detail {
            for episode in &detail.episodes {
                let local = self.local_episode(episode)?;
                if local.retained { known_bytes = known_bytes.saturating_add(local.bytes); known_episodes += 1; }
            }
        }
        if known_episodes > 0 {
            let estimate = known_bytes as f64 / known_episodes as f64 * self.catalog.selected.len() as f64 / 1024.0 / 1024.0;
            Ok(i18n::format("cat_estimated_size", &[("size", format!("{estimate:.0}"))]))
        } else { Ok(i18n::text("cat_size_unknown")) }
    }

    fn persist_catalog_view(&self) -> Result<()> { self.store.set_preference(&self.account, "catalog_view", &self.catalog.saved) }
    fn persist_search_history(&self) -> Result<()> { self.store.set_preference(&self.account, "search_history", &self.catalog.history) }
    fn finish_search_history(&mut self) -> Result<()> {
        if let Some(entry) = self.catalog.history.iter_mut().find(|entry| entry.query == self.catalog.submitted_query) {
            entry.count = if self.catalog.search_has_more { None } else { Some(self.catalog.results.len()) };
        }
        self.persist_search_history()
    }
}

fn inspect_local_page(store: &Store, account: &str, pages: &Path, page: &PageDescriptor,
    receipts: &BTreeMap<String, PageReceipt>) -> Result<Option<PageReceipt>> {
    let candidate = receipts.get(&page.id)
        .filter(|receipt| receipt.page_index == page.index && receipt.width == page.width && receipt.height == page.height)
        .filter(|receipt| !receipt.file_name.is_empty() && !receipt.file_name.contains(['/', '\\']) && !receipt.file_name.contains(".."));
    match candidate {
        Some(receipt) => match crate::downloads::revalidate_receipt(store, account, pages, receipt) {
            Ok(receipt) => Ok(receipt),
            Err(error) if crate::downloads::is_validation_unavailable(&error) => Ok(None),
            Err(error) => Err(error),
        },
        None => Ok(None),
    }
}

fn normalize_comic_id(value: &str) -> Option<String> {
    let value = value.trim();
    let value = if value.get(..2).is_some_and(|prefix| prefix.eq_ignore_ascii_case("mc")) { &value[2..] } else { value };
    if value.is_empty() || value.len() > 18 || !value.bytes().all(|byte| byte.is_ascii_digit()) || value.bytes().all(|byte| byte == b'0') { return None; }
    Some(value.trim_start_matches('0').to_owned())
}

fn is_comic_id_query(value: &str) -> bool {
    value.get(..2).is_some_and(|prefix| prefix.eq_ignore_ascii_case("mc")) && normalize_comic_id(value).is_some()
}

fn deduplicate_chapter_total(value: &str) -> String {
    let mut parts = value.split(" · ");
    let mut label = parts.next().unwrap_or_default().to_owned();
    for part in parts {
        if part.starts_with('\u{5171}') && label.contains(part) { continue; }
        label.push_str(" · "); label.push_str(part);
    }
    label
}

fn matches_query(comic: &Comic, normalized: &str) -> bool {
    comic.title.to_lowercase().contains(normalized) || comic.author.to_lowercase().contains(normalized)
}

fn store_scope(category: &str) -> String { format!("store:{}", hex_digest(category.as_bytes())) }
fn search_scope(query: &str) -> String { format!("search:{}", hex_digest(query.as_bytes())) }
fn page_count(length: usize, per_page: usize) -> usize { length.div_ceil(per_page.max(1)).max(1) }
fn page_slice<T: Clone>(items: &[T], page: usize, per_page: usize) -> Vec<T> { items.iter().skip(page.saturating_sub(1).saturating_mul(per_page)).take(per_page).cloned().collect() }
fn page_delta(value: &str) -> i32 { match value { "next" => 1, "previous" | "prev" => -1, _ => value.parse().unwrap_or(0) } }
fn change_page(page: usize, delta: &str, maximum: usize) -> usize { (page as i64 + i64::from(page_delta(delta))).clamp(1, maximum.max(1) as i64) as usize }
fn merge_comics(destination: &mut Vec<Comic>, incoming: &[Comic]) {
    for comic in incoming {
        if let Some(existing) = destination.iter_mut().find(|existing| existing.id == comic.id) { *existing = comic.clone(); }
        else { destination.push(comic.clone()); }
    }
}
fn join_nonempty(values: &[&str]) -> String { values.iter().map(|value| value.trim()).filter(|value| !value.is_empty()).collect::<Vec<_>>().join(" · ") }

#[cfg(test)]
#[path = "catalog_integrity_tests.rs"]
mod integrity_tests;

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::{
        domain::{ImageIndex, PageDescriptor},
        store::{DownloadRecord, DownloadStatus, PageReceipt},
    };

    fn scanned_state(store: &Store, account: &str) -> Result<CatalogState> {
        let mut state = CatalogState::prepare(store, account, None)?;
        let mut pending = Some(InventoryScan::new(state.comics.iter().map(|comic| comic.id.clone()).collect(),
            BTreeSet::new(), None));
        while let Some(scan) = pending.take() {
            let batch = scan.run(store.path().to_owned(), account)?;
            assert!(batch.inspected_pages <= 4, "Each worker batch must bound complete source-file reads");
            for completed in batch.completed {
                state.local_snapshot.install_episode(completed.comic_id, completed.episode_id, completed.local);
            }
            if let Some(live) = batch.complete_targets {
                state.local_snapshot.retain_episodes(&live);
                state.local_snapshot.checked_all = true;
            }
            pending = batch.continuation;
        }
        state.local_dirty = false;
        Ok(state)
    }

    fn stored_pages(count: usize, published: usize) -> Result<(tempfile::TempDir, Store, Vec<std::path::PathBuf>)> {
        stored_pages_with_validation(count, published, true)
    }

    fn stored_pages_with_validation(count: usize, published: usize, validated: bool) -> Result<(tempfile::TempDir, Store, Vec<std::path::PathBuf>)> {
        let directory = tempfile::tempdir()?;
        let mut store = Store::open(directory.path().join("state.sqlite3"))?;
        store.save_detail("account", &ComicDetail {
            comic: Comic { id: "comic".into(), favorite: true, ..Default::default() },
            episodes: vec![Episode { id: "episode".into(), comic_id: "comic".into(), ..Default::default() }],
            ..Default::default()
        })?;
        let index = ImageIndex {
            episode_id: "episode".into(), revision: "revision".into(),
            images: (0..count).map(|position| PageDescriptor { id: format!("page-{position}"),
                index: position, width: 2, height: 3 }).collect(), ..Default::default()
        };
        store.save_index("account", &index)?;
        store.save_download("account", &DownloadRecord { id: "task".into(), comic_id: "comic".into(),
            episode_id: "episode".into(), revision: "revision".into(), total_pages: count as u32,
            generation: 1, retained: true, status: DownloadStatus::Running, ..Default::default() })?;
        let pages = directory.path().join("accounts").join(hex_digest(b"account")).join("pages");
        std::fs::create_dir_all(&pages)?;
        let mut files = Vec::new();
        for position in 0..published {
            let mut encoded = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(2, 3, image::Luma([position as u8])))
                .write_to(&mut encoded, image::ImageFormat::Png)?;
            let bytes = encoded.into_inner();
            let digest = hex_digest(&bytes);
            let file = pages.join(format!("{digest}.png"));
            std::fs::write(&file, &bytes)?;
            let receipt = PageReceipt { episode_id: "episode".into(), revision: "revision".into(),
                page_id: format!("page-{position}"), page_index: position,
                file_name: format!("{digest}.png"), sha256: digest, bytes: bytes.len() as u64,
                width: 2, height: 3, retained: true, verified_at: 20, ..Default::default() };
            store.begin_page_commit("account", "task", &receipt)?;
            store.commit_page("account", "task", &receipt)?;
            if validated {
                let proof = crate::image_integrity::validate_image(&bytes, None)?;
                store.promote_page_validation("account", &receipt, &proof, 20)?
                    .context("Bind the real fixture decode to its stored receipt")?;
            }
            files.push(file);
        }
        Ok((directory, store, files))
    }

    #[test]
    fn prepared_local_snapshot_does_not_promote_partial_receipts_to_offline_completion() -> Result<()> {
        let (_directory, store, _files) = stored_pages(2, 1)?;
        let state = scanned_state(&store, "account")?;
        let episode = state.local_snapshot.episodes.get("episode").context("Prepared episode status")?;
        assert!(episode.known);
        assert_eq!((episode.present, episode.total), (1, 2));
        assert!(!episode.retained);
        assert!(!state.local_snapshot.downloaded_comics.contains("comic"));
        Ok(())
    }

    #[test]
    fn page_change_invalidates_completion_before_a_background_rescan() -> Result<()> {
        let (_directory, store, files) = stored_pages(1, 1)?;
        let mut state = scanned_state(&store, "account")?;
        assert!(state.local_snapshot.downloaded_comics.contains("comic"));
        std::fs::remove_file(&files[0])?;
        state.local_snapshot.invalidate_episode("comic", "episode");
        assert!(!state.local_snapshot.downloaded_comics.contains("comic"));
        assert!(!state.local_snapshot.episodes.get("episode").is_some_and(|episode| episode.retained));
        let refreshed = scanned_state(&store, "account")?;
        assert_eq!(refreshed.local_snapshot.episodes["episode"].present, 0);
        assert!(!refreshed.local_snapshot.episodes["episode"].retained);
        assert!(scanned_state(&store, "another-account")?.local_snapshot.downloaded_comics.is_empty());
        Ok(())
    }

    #[test]
    fn equal_length_file_corruption_cannot_keep_a_comic_available_offline() -> Result<()> {
        let (_directory, store, files) = stored_pages(1, 1)?;
        let before = scanned_state(&store, "account")?;
        assert!(before.local_snapshot.downloaded_comics.contains("comic"));
        let mut bytes = std::fs::read(&files[0])?;
        bytes[8] ^= 0x5a;
        std::fs::write(&files[0], &bytes)?;
        let after = scanned_state(&store, "account")?;
        assert_eq!(after.local_snapshot.episodes["episode"].present, 0);
        assert!(!after.local_snapshot.episodes["episode"].retained);
        assert!(!after.local_snapshot.downloaded_comics.contains("comic"));
        Ok(())
    }

    #[test]
    fn offline_catalog_upgrades_legacy_receipts_after_a_complete_decode() -> Result<()> {
        let (_directory, store, _files) = stored_pages_with_validation(2, 2, false)?;
        assert!(store.page_receipts("account", "episode", "revision")?.iter()
            .all(|receipt| receipt.decode_validation_version == 0));
        let state = scanned_state(&store, "account")?;
        assert!(state.local_snapshot.downloaded_comics.contains("comic"));
        assert_eq!(state.local_snapshot.episodes["episode"].present, 2);
        assert!(store.page_receipts("account", "episode", "revision")?.iter()
            .all(|receipt| receipt.decode_validation_version == crate::image_integrity::VALIDATION_VERSION));
        Ok(())
    }

    #[test]
    fn unsupported_legacy_animation_preserves_originals_without_blocking_other_catalog_entries() -> Result<()> {
        use image::AnimationDecoder as _;

        let (directory, mut store, _files) = stored_pages(1, 1)?;
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            let frames = [17, 91].map(|tone| image::Frame::new(image::RgbaImage::from_pixel(
                2, 3, image::Rgba([tone, tone, tone, 255]))));
            encoder.encode_frames(frames.into_iter())?;
        }
        let decoded = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&bytes))?
            .into_frames().collect_frames()?;
        assert_eq!(decoded.len(), 2, "The legacy fixture must be a real complete animation");
        assert!(decoded.iter().all(|frame| frame.buffer().dimensions() == (2, 3)));
        let failure = crate::image_integrity::validate_image(&bytes, None)
            .err().context("The complete animation must be outside the supported image subset")?;
        assert_eq!(failure.kind, crate::image_integrity::IntegrityFailureKind::Unsupported);
        assert!(failure.encoding_evidence().is_none());

        store.save_detail("account", &ComicDetail {
            comic: Comic { id: "a-unsupported-comic".into(), favorite: true, chapter_count: 1, ..Default::default() },
            episodes: vec![Episode { id: "a-unsupported-episode".into(), comic_id: "a-unsupported-comic".into(),
                access: EpisodeAccess::Owned, ..Default::default() }], ..Default::default()
        })?;
        let index = ImageIndex { episode_id: "a-unsupported-episode".into(), revision: "legacy-animation-revision".into(),
            images: vec![PageDescriptor { id: "legacy-animation-page".into(), index: 0, width: 2, height: 3 }],
            ..Default::default() };
        store.save_index("account", &index)?;
        store.save_download("account", &DownloadRecord { id: "legacy-animation-task".into(),
            comic_id: "a-unsupported-comic".into(), episode_id: index.episode_id.clone(), revision: index.revision.clone(),
            total_pages: 1, generation: 1, retained: true, status: DownloadStatus::Running, ..Default::default() })?;
        let digest = hex_digest(&bytes);
        let file_name = format!("{digest}.gif");
        let file = directory.path().join("accounts").join(hex_digest(b"account")).join("pages").join(&file_name);
        std::fs::write(&file, &bytes)?;
        let receipt = PageReceipt { episode_id: index.episode_id.clone(), revision: index.revision.clone(),
            page_id: index.images[0].id.clone(), page_index: 0, file_name, sha256: digest,
            bytes: bytes.len() as u64, width: 2, height: 3, retained: true, verified_at: 30, ..Default::default() };
        store.begin_page_commit("account", "legacy-animation-task", &receipt)?;
        store.commit_page("account", "legacy-animation-task", &receipt)?;
        // A crash journal may still reference a published legacy object. The
        // unsupported verdict must preserve both its committed and pending state.
        store.begin_page_commit("account", "legacy-animation-task", &receipt)?;
        let journal_before = store.pending_page_commits("account")?;
        assert_eq!(journal_before.len(), 1);
        let history = || -> Result<(String, i64, i64)> {
            let connection = rusqlite::Connection::open_with_flags(store.path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            Ok(connection.query_row(
                "SELECT sha256,bytes,decode_validation_version FROM page_identity_history
                 WHERE account_key=?1 AND episode_id=?2 AND revision=?3 AND page_id=?4",
                rusqlite::params!["account", receipt.episode_id, receipt.revision, receipt.page_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?)
        };
        let history_before = history()?;
        assert_eq!(history_before.2, 0);

        let state = scanned_state(&store, "account")?;
        let unavailable = &state.local_snapshot.episodes["a-unsupported-episode"];
        assert!(unavailable.known);
        assert_eq!((unavailable.present, unavailable.total), (0, 1));
        assert!(!unavailable.retained);
        assert!(!state.local_snapshot.downloaded_comics.contains("a-unsupported-comic"));
        assert!(state.local_snapshot.downloaded_comics.contains("comic"),
            "An unavailable first entry must not prevent a later supported chapter from being scanned");
        assert_eq!(std::fs::read(&file)?, bytes);
        assert_eq!(store.load_index("account", &index.episode_id, Some(&index.revision))?, Some(index.clone()));
        assert_eq!(store.page_receipt("account", &receipt.episode_id, &receipt.revision, &receipt.page_id)?, Some(receipt.clone()));
        assert_eq!(store.pending_page_commits("account")?, journal_before);
        assert_eq!(history()?, history_before);
        assert!(store.page_object_referenced("account", &receipt.file_name)?);
        let task = store.load_download("account", "legacy-animation-task")?.context("Preserve the failed legacy task")?;
        assert_eq!(task.status, DownloadStatus::Failed);
        assert_eq!(task.error_code.as_deref(), Some(failure.code));
        Ok(())
    }

    #[test]
    fn reading_inventory_is_bound_to_the_actual_scanned_revision() -> Result<()> {
        let (_directory, store, _files) = stored_pages(1, 1)?;
        let state = scanned_state(&store, "account")?;
        assert_eq!(state.local_snapshot.inventory_for_revision("episode", "revision"), Some((1, 1, true)));
        assert_eq!(state.local_snapshot.inventory_for_revision("episode", "another-revision"), None);
        Ok(())
    }

    #[test]
    fn startup_attaches_unknown_inventory_without_reading_or_upgrading_cached_pages() -> Result<()> {
        let (_directory, store, files) = stored_pages_with_validation(2, 2, false)?;
        let mut bytes = std::fs::read(&files[1])?;
        bytes[8] ^= 0x5a;
        std::fs::write(&files[1], &bytes)?;
        let state = CatalogState::prepare(&store, "account", None)?;
        assert!(state.local_dirty);
        assert!(state.local_snapshot.episodes.is_empty());
        assert!(state.local_snapshot.downloaded_comics.is_empty());
        assert!(store.page_receipts("account", "episode", "revision")?.iter()
            .all(|receipt| receipt.decode_validation_version == 0));
        let scanned = scanned_state(&store, "account")?;
        assert_eq!(scanned.local_snapshot.episodes["episode"].present, 1);
        assert!(!scanned.local_snapshot.episodes["episode"].retained);
        Ok(())
    }

    #[test]
    fn incremental_inventory_publishes_only_a_completely_checked_chapter() -> Result<()> {
        let (_directory, store, files) = stored_pages(7, 7)?;
        std::fs::remove_file(&files[6])?;
        let mut pending = Some(InventoryScan::new(vec!["comic".into()], BTreeSet::new(), None));
        let mut inspected = 0;
        let mut publications = Vec::new();
        while let Some(scan) = pending.take() {
            let batch = scan.run(store.path().to_owned(), "account")?;
            assert!(batch.inspected_pages <= 4);
            inspected += batch.inspected_pages;
            if inspected < 7 { assert!(batch.completed.is_empty()); }
            publications.extend(batch.completed);
            pending = batch.continuation;
        }
        assert_eq!(inspected, 7);
        assert_eq!(publications.len(), 1);
        assert_eq!((publications[0].local.present, publications[0].local.total), (6, 7));
        assert!(!publications[0].local.retained);
        Ok(())
    }

    #[test]
    fn invalidated_completed_inventory_cannot_publish_stale_offline_availability() -> Result<()> {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
        let (directory, store, files) = stored_pages(1, 1)?;
        let client = crate::source::BiliClient::offline(directory.path())?;
        let mut app = super::super::test_support::app(directory.path(), crate::platform::DeviceProfile::Paperwhite5,
            client, runtime.handle().clone())?;
        app.account = "account".into();
        app.catalog = CatalogState::prepare(&store, "account", None)?;
        app.refresh_catalog_local_snapshot();
        let old = super::super::test_support::next_job(&app, Duration::from_secs(5))?;
        std::fs::remove_file(&files[0])?;
        app.invalidate_catalog_local_episode("comic", "episode");
        (old.apply)(&mut app);
        assert!(!app.catalog.local_snapshot.downloaded_comics.contains("comic"));
        for _ in 0..10 {
            if !app.catalog.local_loading { break; }
            let next = super::super::test_support::next_job(&app, Duration::from_secs(5))?;
            (next.apply)(&mut app);
        }
        assert!(!app.catalog.local_loading);
        assert_eq!(app.catalog.local_snapshot.episodes["episode"].present, 0);
        assert!(!app.catalog.local_snapshot.downloaded_comics.contains("comic"));
        Ok(())
    }

    #[test]
    fn retired_inventory_job_does_not_leave_the_retained_catalog_permanently_loading() -> Result<()> {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
        let (directory, store, _files) = stored_pages(1, 1)?;
        let client = crate::source::BiliClient::offline(directory.path())?;
        let mut app = super::super::test_support::app(directory.path(), crate::platform::DeviceProfile::Paperwhite5,
            client, runtime.handle().clone())?;
        app.account = "account".into();
        app.catalog = CatalogState::prepare(&store, "account", None)?;
        app.refresh_catalog_local_snapshot();
        let retired = super::super::test_support::next_job(&app, Duration::from_secs(5))?;
        app.jobs.clear();
        app.pending.clear();
        drop(retired);
        assert!(app.catalog.local_loading);
        app.refresh_catalog_local_snapshot();
        for _ in 0..10 {
            if !app.catalog.local_loading { break; }
            let next = super::super::test_support::next_job(&app, Duration::from_secs(5))?;
            (next.apply)(&mut app);
        }
        assert!(!app.catalog.local_loading);
        assert_eq!(app.catalog.local_snapshot.episodes["episode"].present, 1);
        assert!(app.catalog.local_snapshot.downloaded_comics.contains("comic"));
        Ok(())
    }

    #[test]
    fn comic_id_shortcuts_do_not_capture_ordinary_mc_prefixed_search_terms() {
        assert!(is_comic_id_query("mc28601"));
        assert!(is_comic_id_query("MC28601"));
        assert!(!is_comic_id_query("mcloud"));
        assert!(!is_comic_id_query("mc0"));
        assert!(!is_comic_id_query("\u{6f2b}\u{753b}"));
        assert_eq!(deduplicate_chapter_total("\u{5df2}\u{8bfb}\u{5b8c} · \u{5171} 60 \u{8bdd} · \u{5171} 60 \u{8bdd}"),
            "\u{5df2}\u{8bfb}\u{5b8c} · \u{5171} 60 \u{8bdd}");
    }

    #[test]
    fn auth_return_restores_only_selected_ids_readable_in_the_new_detail() {
        let intent = CatalogAuthReturn {
            comic_id: "1".into(), selected: ["11", "12", "13", "14", "15"].into_iter().map(str::to_owned).collect(),
            selecting: true, destination_account: "new-account".into(), captured: Instant::now(),
        };
        let detail = ComicDetail {
            comic: Comic { id: "1".into(), ..Default::default() },
            episodes: vec![
                Episode { id: "11".into(), comic_id: "1".into(), access: EpisodeAccess::Locked, ..Default::default() },
                Episode { id: "12".into(), comic_id: "1".into(), access: EpisodeAccess::Free, ..Default::default() },
                Episode { id: "13".into(), comic_id: "1".into(), access: EpisodeAccess::Owned, ..Default::default() },
                Episode { id: "14".into(), comic_id: "1".into(), access: EpisodeAccess::Temporary, expires_at: Some(50), ..Default::default() },
                Episode { id: "15".into(), comic_id: "another-comic".into(), access: EpisodeAccess::Owned, ..Default::default() },
            ], ..Default::default()
        };
        assert_eq!(intent.selection_from(&detail, 100), ["12", "13"].into_iter().map(str::to_owned).collect());
        let other = ComicDetail { comic: Comic { id: "another-comic".into(), ..Default::default() }, ..detail };
        assert!(intent.selection_from(&other, 100).is_empty());
    }
}

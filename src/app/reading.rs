use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    rc::Rc,
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use slint::{ComponentHandle, Model, ModelRc, SharedPixelBuffer, Rgba8Pixel, VecModel};

use super::{App, Preferences, display_order, hex_digest, now, tr};
use crate::{
    demo::DemoData,
    domain::{ComicDetail, Episode, EpisodeAccess, ImageIndex, PageDescriptor, ReadingAnchor},
    downloads::{DownloadEvent, DownloadEventKind, DownloadRequest, LocalPageResolver},
    i18n,
    performance::{Operation, Span},
    platform::{RefreshKind, RefreshPlan, RefreshReason, RefreshRect},
    reader::{DisplayMode, FitMode, NormalizedRect, PageDirection, ReaderCore, ReaderMove, ReaderSettings, RenderedPage, Viewport},
    source::ErrorKind,
    store::{DownloadRecord, DownloadStatus},
    ui::{AppWindow, ReaderChapter, ReaderModel, ReaderThumbnail, ShellModel},
};

const MAX_QUEUED_INTENTS: usize = 32;
const NEXT_FRAME_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct SavedReaderPreferences {
    mode: String,
    fit: String,
    direction: String,
    every: String,
    landscape: String,
    cover_alone: bool,
    dither: bool,
    footer: bool,
}

impl Default for SavedReaderPreferences {
    fn default() -> Self { Self::from(&Preferences::default()) }
}

impl From<&Preferences> for SavedReaderPreferences {
    fn from(value: &Preferences) -> Self {
        Self { mode: value.mode.clone(), fit: value.fit.clone(), direction: value.direction.clone(),
            every: value.every.clone(), landscape: value.landscape.clone(), cover_alone: value.cover_alone,
            dither: value.dither, footer: value.footer }
    }
}

pub(super) struct ReadingState {
    comic_id: String,
    episode_id: String,
    detail: Option<ComicDetail>,
    original_index: Option<ImageIndex>,
    pending_index: Option<ImageIndex>,
    core: Option<ReaderCore>,
    presented_core: Option<ReaderCore>,
    presented_strip: bool,
    saved: SavedReaderPreferences,
    generation: u64,
    version: u64,
    rendering: bool,
    rendering_version: Option<u64>,
    render_again: bool,
    render_after_sleep: bool,
    render_started: Option<Instant>,
    missing_pages: Vec<usize>,
    cancelled: Arc<AtomicBool>,
    render_cancelled: Arc<AtomicBool>,
    next_frame: NextFrameSlot,
    next_frame_retry_pending: bool,
    queued: VecDeque<(String, String, Option<Operation>)>,
    performance_request: Option<Operation>,
    performance_presented: Option<Operation>,
    performance_model_id: Option<u64>,
    page: slint::Image,
    second_page: slint::Image,
    overview: slint::Image,
    metadata: FrameMetadata,
    loading: bool,
    error: Option<String>,
    menu: bool,
    sheet: String,
    guidance: bool,
    paged_anchor: Option<ReadingAnchor>,
    demo_strip: bool,
    history_sent: bool,
    last_dark_fraction: Option<f32>,
    brightness: Option<u8>,
    warmth: Option<u8>,
    light_busy: bool,
    light_version: u64,
    page_resolver: Option<LocalPageResolver>,
    acquisition: Option<ReaderAcquisition>,
    prefetch_retry: bool,
    prefetch_held_after_sleep: bool,
    grid_page: usize,
    thumbnails: Vec<ReaderThumbnail>,
    thumbnail_plan: Option<ThumbnailPlan>,
    thumbnail_proofs: BTreeMap<usize, crate::image_integrity::ValidatedImage>,
    thumbnail_cancelled: Arc<AtomicBool>,
    thumbnail_model: Rc<VecModel<ReaderThumbnail>>,
    chapter_model: Rc<VecModel<ReaderChapter>>,
    thumbnails_pending: bool,
    thumbnail_version: u64,
    thumbnails_rendering: bool,
    thumbnails_again: bool,
}

impl Drop for ReadingState {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.render_cancelled.store(true, Ordering::Release);
        self.next_frame.cancel();
        self.thumbnail_cancelled.store(true, Ordering::Release);
    }
}

impl ReadingState {
    fn pending(comic_id: String, episode_id: String, generation: u64, saved: SavedReaderPreferences, guidance: bool, demo: bool) -> Self {
        Self { comic_id, episode_id, detail: None, original_index: None, pending_index: None,
            core: None, presented_core: None, presented_strip: false,
            saved, generation, version: 1, rendering: false, rendering_version: None,
            render_again: false, render_after_sleep: false,
            render_started: None, missing_pages: Vec::new(),
            cancelled: Arc::new(AtomicBool::new(false)), render_cancelled: Arc::new(AtomicBool::new(false)),
            next_frame: NextFrameSlot::Empty, next_frame_retry_pending: false,
            queued: VecDeque::new(), performance_request: None,
            performance_presented: None, performance_model_id: None,
            page: slint::Image::default(), second_page: slint::Image::default(), overview: slint::Image::default(),
            metadata: FrameMetadata::default(), loading: true, error: None, menu: false, sheet: String::new(),
            guidance, paged_anchor: None, demo_strip: false, history_sent: false, last_dark_fraction: None,
            brightness: demo.then_some(12), warmth: demo.then_some(8), light_busy: false, light_version: 0,
            page_resolver: None, acquisition: None, prefetch_retry: false, prefetch_held_after_sleep: false, grid_page: 1,
            thumbnails: Vec::new(), thumbnail_plan: None, thumbnail_proofs: BTreeMap::new(),
            thumbnail_cancelled: Arc::new(AtomicBool::new(false)),
            thumbnail_model: Rc::new(VecModel::from(Vec::new())),
            chapter_model: Rc::new(VecModel::from(Vec::new())), thumbnails_pending: false,
            thumbnail_version: 0, thumbnails_rendering: false, thumbnails_again: false }
    }

    fn episode(&self) -> Option<&Episode> {
        self.detail.as_ref()?.episodes.iter().find(|episode| episode.id == self.episode_id)
    }

    fn next_episode(&self) -> Option<&Episode> {
        let detail = self.detail.as_ref()?;
        let position = detail.episodes.iter().position(|episode| episode.id == self.episode_id)?;
        detail.episodes.get(position + 1)
    }

    fn visible_core(&self) -> Option<&ReaderCore> {
        self.presented_core.as_ref().or(self.core.as_ref())
    }

    fn displayed_anchor(&self, timestamp: i64) -> Option<(ReadingAnchor, bool)> {
        if let Some(core) = &self.presented_core {
            Some((core.anchor(timestamp), self.presented_strip))
        } else {
            self.paged_anchor.clone().map(|mut anchor| {
                anchor.updated_at = timestamp;
                (anchor, false)
            })
        }
    }

    fn retain_frame_during_preparation(&self) -> bool {
        self.presented_core.as_ref().is_some_and(|core| !core.is_transition())
            && self.page.size().width > 0 && self.missing_pages.is_empty()
            && self.error.is_none()
    }

    fn preparation_feedback_due(&self) -> bool {
        self.loading && self.retain_frame_during_preparation()
            && self.render_started.is_some_and(|started| started.elapsed() >= Duration::from_millis(600))
    }

    /// Cancel only the target actually owned by this render job. Notifications
    /// for the same version still coalesce without revoking useful work.
    fn cancel_obsolete_render(&self) -> bool {
        self.rendering && self.rendering_version.is_some_and(|version| version != self.version)
            && !self.render_cancelled.swap(true, Ordering::AcqRel)
    }

    fn depends_on_download(&self, event: &DownloadEvent) -> bool {
        let Some(core) = self.core.as_ref() else { return false; };
        let visible = reader_visible_pages(core);
        match &event.kind {
            DownloadEventKind::PageReady { episode_id, revision, page_id, page_index, .. } => {
                episode_id == &self.episode_id && revision == &core.index().revision
                    && visible.contains(page_index)
                    && core.index().images.get(*page_index).is_some_and(|page| &page.id == page_id)
            },
            DownloadEventKind::IndexReady { index } => index.episode_id == self.episode_id
                && index.revision == core.index().revision,
            DownloadEventKind::PrefetchReady | DownloadEventKind::Completed | DownloadEventKind::Failed { .. } => {
                self.acquisition.as_ref().is_some_and(|request| request.task_id == event.task_id
                    && request.task_generation == event.generation && request.reader_generation == self.generation
                    && request.revision == core.index().revision && request.visible == visible)
            },
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct FrameMetadata {
    overlap: bool,
    ratio: f32,
    focus_x: f32,
    focus_y: f32,
    focus_width: f32,
    focus_height: f32,
    left_page: i32,
    right_page: i32,
}

#[derive(Clone)]
struct Raster { rgba: Vec<u8>, width: u32, height: u32 }

pub(super) struct ReaderSleepLease {
    pub(super) generation: u64,
    pub(super) episode_id: String,
    pub(super) revision: String,
    pub(super) task_id: Option<String>,
    pub(super) task_generation: Option<u64>,
}

#[derive(Clone, Debug)]
struct ReaderAcquisition {
    task_id: String,
    task_generation: u64,
    reader_generation: u64,
    revision: String,
    visible: Vec<usize>,
    requested: Vec<usize>,
}

impl ReaderAcquisition {
    fn failed_for(&self, record: &DownloadRecord, reader_generation: u64, episode_id: &str,
        revision: &str, visible: &[usize], missing: &[usize], require_all: bool) -> bool {
        !missing.is_empty()
            && self.reader_generation == reader_generation
            && self.revision == revision
            && self.visible == visible
            && record.id == self.task_id
            && record.generation == self.task_generation
            && record.episode_id == episode_id
            && record.revision == revision
            && !record.retained
            && record.status == DownloadStatus::Failed
            && if require_all { missing.iter().all(|page| self.requested.contains(page)) }
                else { missing.iter().any(|page| self.requested.contains(page)) }
    }
}

struct PreparedFrame {
    page: Raster,
    second: Option<Raster>,
    overview: Option<Raster>,
    metadata: FrameMetadata,
    dark_fraction: f32,
}

enum PreparedOutcome { Ready(PreparedFrame), Missing(Vec<usize>), Cancelled, Failed }
struct PreparedReader { core: ReaderCore, outcome: PreparedOutcome }

/// A pre-render input, distinct from the focus written back by rendering.
/// In particular, rotated pages can acquire different source coordinates.
#[derive(Clone, PartialEq)]
struct FrameInput {
    anchor: ReadingAnchor,
    crop_index: usize,
    crop_count: usize,
    settings: ReaderSettings,
}

impl FrameInput {
    fn new(core: &ReaderCore) -> Self {
        Self { anchor: core.anchor(0), crop_index: core.crop_index(),
            crop_count: core.crop_count(), settings: core.settings().clone() }
    }

    fn matches(&self, core: &ReaderCore) -> bool {
        self.anchor == core.anchor(0) && self.crop_index == core.crop_index()
            && self.crop_count == core.crop_count() && self.settings == *core.settings()
    }

    fn allocated_bytes(&self) -> u64 {
        (self.anchor.episode_id.capacity() + self.anchor.revision.capacity()
            + self.anchor.page_id.capacity()) as u64
    }
}

#[derive(Clone, PartialEq)]
struct NextFrameKey {
    epoch: u64,
    generation: u64,
    origin_version: u64,
    origin: FrameInput,
    target: FrameInput,
    viewport: Viewport,
    landscape: bool,
}

impl NextFrameKey {
    fn origin_is_current(&self, state: &ReadingState, epoch: u64,
        viewport: Viewport, landscape: bool,
    ) -> bool {
        self.epoch == epoch && self.generation == state.generation
            && self.origin_version == state.version && self.viewport == viewport
            && self.landscape == landscape && !state.demo_strip
            && !state.loading && !state.rendering && state.queued.is_empty()
            && state.error.is_none() && !state.cancelled.load(Ordering::Acquire)
            && state.presented_core.as_ref().is_some_and(|core| self.origin.matches(core))
            && state.core.as_ref().is_some_and(|core| self.origin.matches(core))
    }

    fn target_is_current(&self, state: &ReadingState, epoch: u64,
        viewport: Viewport, landscape: bool,
    ) -> bool {
        self.epoch == epoch && self.generation == state.generation
            && state.version == self.origin_version.wrapping_add(1)
            && self.viewport == viewport && self.landscape == landscape
            && !state.demo_strip && !state.cancelled.load(Ordering::Acquire)
            && state.presented_core.as_ref().is_some_and(|core| self.origin.matches(core))
            && state.core.as_ref().is_some_and(|core| self.target.matches(core))
    }

    fn allocated_bytes(&self) -> u64 {
        self.origin.allocated_bytes().saturating_add(self.target.allocated_bytes())
    }

    fn source_notified(&self, event: &DownloadEvent) -> bool {
        match &event.kind {
            DownloadEventKind::PageReady { episode_id, revision, page_id, page_index, .. } =>
                episode_id == &self.target.anchor.episode_id && revision == &self.target.anchor.revision
                    && page_id == &self.target.anchor.page_id && *page_index == self.target.anchor.page_index,
            DownloadEventKind::IndexReady { index } => index.episode_id == self.target.anchor.episode_id,
            _ => false,
        }
    }
}

struct NextFrame {
    key: NextFrameKey,
    core: ReaderCore,
    frame: PreparedFrame,
    proof: crate::image_integrity::ValidatedImage,
}

impl NextFrame {
    fn retained_bytes(&self) -> u64 {
        let pixels = self.frame.page.rgba.capacity()
            .saturating_add(self.frame.second.as_ref().map_or(0, |frame| frame.rgba.capacity()))
            .saturating_add(self.frame.overview.as_ref().map_or(0, |frame| frame.rgba.capacity()));
        (std::mem::size_of::<Self>() as u64).saturating_add(pixels as u64)
            .saturating_add(self.key.allocated_bytes())
            .saturating_add(self.core.retained_preparation_metadata_bytes())
            .saturating_add(self.proof.sha256.capacity() as u64)
    }
}

enum NextFrameSlot {
    Empty,
    Preparing { key: NextFrameKey, cancelled: Arc<AtomicBool> },
    Ready(Box<NextFrame>),
}

impl NextFrameSlot {
    fn cancel(&mut self) {
        match self {
            Self::Preparing { cancelled, .. } => {
                // This marker remains until the actual worker reports exit.
                cancelled.store(true, Ordering::Release);
            }
            Self::Ready(_) => *self = Self::Empty,
            Self::Empty => {}
        }
    }

    fn take_matching(&mut self, matches: impl FnOnce(&NextFrameKey) -> bool) -> Option<NextFrame> {
        if matches!(self, Self::Preparing { .. }) {
            self.cancel();
            return None;
        }
        match std::mem::replace(self, Self::Empty) {
            Self::Ready(frame) if matches(&frame.key) => Some(*frame),
            _ => None,
        }
    }

    fn retained_bytes(&self) -> u64 {
        match self { Self::Ready(frame) => frame.retained_bytes(), _ => 0 }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Preparing { cancelled, .. } if cancelled.load(Ordering::Acquire) => "cancelling",
            Self::Preparing { .. } => "preparing",
            Self::Ready(_) => "ready",
        }
    }
}

#[derive(Clone)]
enum ReaderFiles {
    Demo { paths: Vec<PathBuf>, strip: Option<PathBuf>, strip_active: bool },
    Account(LocalPageResolver),
}

impl ReaderFiles {
    fn local_page(&self, index: &ImageIndex, position: usize) -> Result<Option<PathBuf>> {
        let Some(page) = index.images.get(position) else { return Ok(None); };
        match self {
            Self::Demo { paths, strip, strip_active } => {
                let path = if *strip_active { strip.clone() } else { paths.get(position).cloned() };
                Ok(path.filter(|path| path.is_file()))
            }
            Self::Account(resolver) => resolver.local_page(&index.episode_id, &index.revision, &page.id),
        }
    }

    fn source(&self, index: &ImageIndex, position: usize,
        previous: Option<&crate::image_integrity::ValidatedImage>, cancelled: &AtomicBool,
    ) -> Result<Option<Option<crate::reader::ValidatedGraySource>>> {
        self.source_profiled(index, position, previous, cancelled, None)
    }

    fn source_profiled(&self, index: &ImageIndex, position: usize,
        previous: Option<&crate::image_integrity::ValidatedImage>, cancelled: &AtomicBool,
        profile: Option<&mut crate::reader::ReaderPreparationProfile>,
    ) -> Result<Option<Option<crate::reader::ValidatedGraySource>>> {
        let Some(page) = index.images.get(position) else { return Ok(None); };
        match (self, profile) {
            (Self::Demo { .. }, Some(profile)) => self.local_page(index, position)?
                .map(|path| crate::reader::load_changed_source_profiled(&path, previous, profile).map_err(anyhow::Error::new))
                .transpose(),
            (Self::Demo { .. }, None) => self.local_page(index, position)?
                .map(|path| crate::reader::load_changed_source(&path, previous).map_err(anyhow::Error::new))
                .transpose(),
            (Self::Account(resolver), Some(profile)) => resolver.with_verified_page_profiled(
                &index.episode_id, &index.revision, &page.id, cancelled, profile,
                |profile| &mut profile.stages,
                |input, profile| crate::reader::load_hashed_source_profiled(input, previous, cancelled, profile)),
            (Self::Account(resolver), None) => resolver.with_verified_page(&index.episode_id, &index.revision,
                &page.id, cancelled, |input| crate::reader::load_hashed_source(input, previous, cancelled)),
        }
    }
}

struct PreparedThumbnail {
    page: usize,
    raster: Option<Arc<Raster>>,
    proof: Option<crate::image_integrity::ValidatedImage>,
    unchanged: bool,
    failed: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct ThumbnailPlan {
    revision: String,
    start: usize,
    end: usize,
    size: (u32, u32),
    dither: bool,
}

impl App {
    pub(super) fn active_reader_identity(&self) -> Option<(&str, &str)> {
        self.reading.as_ref().map(|state| (state.comic_id.as_str(), state.episode_id.as_str()))
    }

    pub(super) fn initialize_demo_anchor(&mut self) -> Result<()> {
        let Some(demo) = self.demo.as_ref() else { return Ok(()); };
        let resume_comic = demo.fixture["continue_reading"]["comic_id"].as_u64().map(|id| id.to_string());
        for comic in demo.comics.iter().filter(|comic| resume_comic.as_deref() == Some(comic.id.as_str())) {
            if self.store.load_anchor(&self.account, &comic.id)?.is_some() { continue; }
            let Some(episode_id) = comic.last_episode_id.as_ref() else { continue; };
            let Some(index) = demo.indices.iter().find(|index| &index.episode_id == episode_id) else { continue; };
            if index.images.is_empty() { continue; }
            let page = comic.extra["read_page"].as_u64().unwrap_or(1).saturating_sub(1) as usize;
            let mut core = ReaderCore::new(index.clone(), ReaderSettings::default())?;
            core.set_page(page.min(index.images.len() - 1))?;
            self.store.save_anchor(&self.account, &comic.id, &core.anchor(now()))?;
        }
        Ok(())
    }

    pub(super) fn open_reader_for_comic(&mut self, comic_id: &str) -> Result<()> {
        let detail = self.reader_cached_detail(comic_id)?;
        let Some(detail) = detail else { return self.request_reader_detail(comic_id, None); };
        let anchor = self.store.load_anchor(&self.account, comic_id)?;
        let selected = anchor.as_ref().and_then(|anchor| detail.episodes.iter().find(|episode| episode.id == anchor.episode_id))
            .or_else(|| detail.comic.last_episode_id.as_ref().and_then(|id| detail.episodes.iter().find(|episode| &episode.id == id)))
            .or_else(|| detail.episodes.iter().find(|episode| episode.readable_at(now())))
            .or_else(|| detail.episodes.first()).map(|episode| episode.id.clone());
        if let Some(episode_id) = selected { self.open_episode(comic_id, &episode_id)?; }
        else { self.toast("common_empty"); }
        Ok(())
    }

    pub(super) fn open_episode(&mut self, comic_id: &str, episode_id: &str) -> Result<()> {
        let Some(detail) = self.reader_cached_detail(comic_id)? else {
            return self.request_reader_detail(comic_id, Some(episode_id));
        };
        let Some(episode) = detail.episodes.iter().find(|episode| episode.id == episode_id).cloned() else {
            return self.request_reader_detail(comic_id, Some(episode_id));
        };
        if !episode.readable_at(now()) {
            if episode.access == EpisodeAccess::Locked { self.catalog.detail = Some(detail); self.open_purchase(episode_id)?; }
            else { self.toast(reader_blocked_message(&episode, now())); }
            return Ok(());
        }
        let mut anchor = self.store.load_anchor(&self.account, comic_id)?.filter(|anchor| anchor.episode_id == episode_id);
        let cached = if let Some(anchor) = anchor.as_ref() {
            self.store.load_index(&self.account, episode_id, Some(&anchor.revision))?
                .or(self.store.load_index(&self.account, episode_id, None)?)
        } else { self.store.load_index(&self.account, episode_id, None)? };
        if anchor.is_none() && self.demo.is_some() && detail.comic.last_episode_id.as_deref() == Some(episode_id) {
            if let Some(index) = cached.as_ref() {
                if let Some(page) = detail.comic.extra["read_page"].as_u64().filter(|page| *page > 0) {
                    let mut core = ReaderCore::new(index.clone(), ReaderSettings::default())?;
                    core.set_page((page.saturating_sub(1) as usize).min(core.page_count() - 1))?;
                    anchor = Some(core.anchor(now()));
                }
            }
        }
        if cached.is_none() && self.demo.is_none()
            && (!self.require_network("reader", "open-episode", &format!("{comic_id}:{episode_id}")) || !self.require_session()) {
            return Ok(());
        }
        let generation = self.begin_reader(comic_id, episode_id)?;
        if let Some(state) = self.reading.as_mut() { state.detail = Some(detail.clone()); }
        self.catalog.detail = Some(detail);
        if let Some(index) = cached {
            self.install_reader_index(generation, index, anchor)?;
        } else if let Some(demo) = self.demo.as_ref() {
            if let Some(index) = demo.indices.iter().find(|index| index.episode_id == episode_id).cloned() {
                self.install_reader_index(generation, index, anchor)?;
            } else { self.fail_reader("common_no_download"); }
        } else {
            let client = self.client.clone();
            let id = episode_id.to_owned();
            self.source_job("reader-index", async move { client.image_index(&id).await }, move |app, result| {
                if !app.reader_is_current(generation) { return; }
                match result {
                    Ok(index) => {
                        if let Err(error) = app.store.save_index(&app.account, &index)
                            .and_then(|_| app.install_reader_index(generation, index, anchor)) {
                            eprintln!("bilicomics: reader index persistence failed: {error:#}");
                            app.fail_reader("common_storage_error");
                        }
                    }
                    Err(error) => { app.source_failure(&error); app.fail_reader(reader_source_message(error.kind)); }
                }
            });
        }
        Ok(())
    }

    fn reader_cached_detail(&self, comic_id: &str) -> Result<Option<ComicDetail>> {
        Ok(self.catalog.detail.as_ref().filter(|detail| detail.comic.id == comic_id).cloned()
            .or(self.store.load_detail(&self.account, comic_id)?))
    }

    fn request_reader_detail(&mut self, comic_id: &str, episode_id: Option<&str>) -> Result<()> {
        let value = episode_id.map_or_else(|| comic_id.to_owned(), |id| format!("{comic_id}:{id}"));
        if !self.require_network("reader", if episode_id.is_some() { "open-episode" } else { "open-comic" }, &value)
            || !self.require_session() { return Ok(()); }
        let generation = self.begin_reader(comic_id, episode_id.unwrap_or_default())?;
        let id = comic_id.to_owned();
        let target = episode_id.map(str::to_owned);
        let client = self.client.clone();
        self.source_job("reader-detail", async move { client.comic_detail(&id).await }, move |app, result| {
            if !app.reader_is_current(generation) || app.route != "reader" { return; }
            match result {
                Ok(detail) => {
                    if target.as_ref().is_some_and(|id| !detail.episodes.iter().any(|episode| &episode.id == id)) {
                        app.fail_reader("common_source_error");
                        return;
                    }
                    let comic_id = detail.comic.id.clone();
                    if let Err(error) = app.store.save_detail(&app.account, &detail) {
                        eprintln!("bilicomics: reader catalog persistence failed: {error:#}");
                        app.fail_reader("common_storage_error");
                        return;
                    }
                    app.catalog.detail = Some(detail);
                    let opened = if let Some(episode_id) = target { app.open_episode(&comic_id, &episode_id) }
                        else { app.open_reader_for_comic(&comic_id) };
                    if let Err(error) = opened {
                        eprintln!("bilicomics: reader catalog application failed: {error:#}");
                        app.fail_reader("common_source_error");
                    }
                }
                Err(error) => { app.source_failure(&error); app.fail_reader(reader_source_message(error.kind)); }
            }
        });
        Ok(())
    }

    fn begin_reader(&mut self, comic_id: &str, episode_id: &str) -> Result<u64> {
        self.save_reading_position()?;
        if let Err(error) = self.release_reader() {
            eprintln!("bilicomics: previous reader retirement warning: {error:#}");
            self.toast("common_storage_error");
        }
        self.reading = None;
        self.sequence = self.sequence.checked_add(1).context("Reader generation sequence is exhausted")?;
        let generation = self.sequence;
        let saved = self.store.preference(&self.account, &format!("reader_preferences_{comic_id}"))?
            .unwrap_or_else(|| SavedReaderPreferences::from(&self.preferences));
        self.reading = Some(ReadingState::pending(comic_id.to_owned(), episode_id.to_owned(), generation, saved,
            !self.preferences.reader_guidance_seen, self.demo.is_some()));
        self.navigate("reader");
        Ok(generation)
    }

    fn release_reader(&mut self) -> Result<()> {
        self.platform.set_reader_performance_operation(None);
        let mut released = Ok(());
        if let Some(state) = self.reading.as_mut() {
            state.cancelled.store(true, Ordering::Release);
            state.render_cancelled.store(true, Ordering::Release);
            state.thumbnail_cancelled.store(true, Ordering::Release);
            state.next_frame.cancel();
            if let Some(manager) = self.downloads.as_mut() { released = manager.cancel_reader(state.generation); }
        }
        for key in ["reader-render", "reader-prepare-next", "reader-thumbnails", "reader-index", "reader-detail", "reader-light-read", "reader-light-write"] { self.invalidate(key); }
        released
    }

    pub(super) fn close_reader(&mut self) -> Result<()> {
        let saved = self.save_reading_position();
        let released = self.release_reader();
        self.reading = None;
        if let Some(ui) = self.ui.upgrade() {
            let view = ui.global::<ReaderModel>();
            view.set_page(slint::Image::default());
            view.set_second_page(slint::Image::default());
            view.set_overview_image(slint::Image::default());
            view.set_thumbnails(ModelRc::default());
            view.set_chapters(ModelRc::default());
            view.set_thumbnails_loading(false);
            view.set_spread_active(false);
            view.set_retain_page(false);
            view.set_preparing_feedback(false);
            view.set_loading(false);
        }
        if let Err(error) = saved.and(released) {
            // The checkpoint writer retains only the dirty anchor for retry.
            // A failed disk write must not keep an invisible live reader alive.
            eprintln!("bilicomics: reader close checkpoint warning: {error:#}");
            self.toast("common_storage_error");
        }
        Ok(())
    }

    pub(super) fn sync_reader_lease(&mut self) -> Result<()> {
        let current = self.reading.as_ref().filter(|state| self.route == "reader" && !state.cancelled.load(Ordering::Acquire))
            .and_then(|state| state.original_index.as_ref().map(|index|
                (state.generation, state.episode_id.clone(), index.revision.clone())));
        let resolver = self.downloads.as_ref().map(|manager| manager.page_resolver());
        if let Some(manager) = self.downloads.as_mut() { manager.synchronize_reader_owner(current.clone())?; }
        if let Some(state) = self.reading.as_mut() {
            if resolver.is_some() { state.page_resolver = resolver; }
        }
        if current.is_some() && self.downloads.is_some() && !self.is_suspended() && !self.sleep_download_resume_pending() {
            self.enqueue_reader_prefetch(false)?;
            self.reader_page_ready()?;
        }
        Ok(())
    }

    fn reader_is_current(&self, generation: u64) -> bool {
        self.reading.as_ref().is_some_and(|state| state.generation == generation && !state.cancelled.load(Ordering::Acquire))
    }

    fn install_reader_index(&mut self, generation: u64, index: ImageIndex, anchor: Option<ReadingAnchor>) -> Result<()> {
        if !self.reader_is_current(generation) { return Ok(()); }
        if let Some(state) = self.reading.as_mut() { state.next_frame.cancel(); }
        if let Some(previous) = anchor.as_ref().filter(|anchor| anchor.revision != index.revision) {
            if let Some(state) = self.reading.as_mut() {
                state.pending_index = Some(index);
                state.paged_anchor = Some(previous.clone());
                state.loading = false;
                state.error = Some(i18n::text("read_version_changed"));
            }
            return Ok(());
        }
        let state = self.reading.as_mut().context("Reader state disappeared")?;
        let mut core = ReaderCore::new(index.clone(), reader_settings(&state.saved, self.options.landscape, self.platform.logical_viewport_size()))?;
        if let Some(anchor) = anchor.as_ref() {
            if core.restore(anchor).is_err() {
                state.pending_index = Some(index);
                state.paged_anchor = Some(anchor.clone());
                state.loading = false;
                state.error = Some(i18n::text("read_version_changed"));
                return Ok(());
            }
        }
        core.update_settings(reader_settings(&state.saved, self.options.landscape, self.platform.logical_viewport_size()))?;
        core.set_landscape(self.options.landscape);
        attach_demo_panels(&mut core, self.demo.as_deref())?;
        state.paged_anchor = anchor;
        state.original_index = Some(index.clone());
        state.page_resolver = self.downloads.as_ref().map(|manager| manager.page_resolver());
        state.pending_index = None;
        state.core = Some(core);
        state.loading = true;
        state.error = None;
        if state.saved.mode == "strip" && self.demo.as_ref().is_some_and(|demo| demo.strip.is_some()) {
            self.switch_demo_strip(true)?;
        }
        if let Some(manager) = self.downloads.as_ref() { manager.pin_reader(generation, &index.episode_id, &index.revision)?; }
        self.configure_reader_refresh()?;
        self.request_reader_render()?;
        Ok(())
    }

    fn fail_reader(&mut self, key: &str) {
        self.platform.set_reader_performance_operation(None);
        if let Some(state) = self.reading.as_mut() {
            state.next_frame.cancel(); state.loading = false; state.error = Some(i18n::text(key));
        }
        self.dirty = true;
    }

    pub(super) fn reader_action(&mut self, action: &str, value: &str) -> Result<()> {
        let trace = matches!(action, "turn" | "page" | "source-page" | "seek" | "beginning" | "last-view")
            .then(|| Operation::new("reader")).flatten();
        if let Some(trace) = trace {
            // Only the allowlisted action category is recorded, never its value.
            trace.record("navigation_request", &serde_json::json!({"action": action}));
        }
        self.reader_action_traced(action, value, trace)
    }

    fn reader_action_traced(&mut self, action: &str, value: &str, trace: Option<Operation>) -> Result<()> {
        if self.is_suspended() { return Ok(()); }
        if matches!(action, "turn" | "page" | "source-page" | "seek" | "beginning" | "last-view"
            | "retry" | "episode" | "next" | "purchase" | "open-comic" | "open-episode")
            && (action != "turn" || value != "next") {
            if let Some(state) = self.reading.as_mut() { state.next_frame.cancel(); }
        }
        if self.reading.as_ref().is_some_and(|state| state.sheet == "grid")
            && (matches!(action, "menu" | "refresh" | "back" | "shelf" | "turn" | "page" | "source-page"
                | "seek" | "beginning" | "last-view" | "retry" | "episode" | "next" | "purchase"
                | "open-comic" | "open-episode") || action == "sheet" && value != "grid") {
            self.cancel_reader_thumbnails();
        }
        if matches!(action, "turn" | "page" | "source-page" | "seek" | "retry" | "beginning" | "last-view") {
            if let Some(state) = self.reading.as_mut() { state.prefetch_held_after_sleep = false; }
        }
        if matches!(action, "turn" | "page" | "source-page" | "seek" | "beginning" | "last-view"
            | "retry" | "episode" | "next" | "purchase" | "open-comic" | "open-episode") {
            let closed = if let Some(state) = self.reading.as_mut() {
                let opened = state.menu || !state.sheet.is_empty();
                state.menu = false;
                state.sheet.clear();
                opened
            } else { false };
            if closed { self.reader_overlay_refresh(false); }
        }
        match action {
            "open-comic" => return self.open_reader_for_comic(value),
            "open-episode" => {
                if let Some((comic_id, episode_id)) = value.split_once(':') { return self.open_episode(comic_id, episode_id); }
                return Ok(());
            }
            "back" | "shelf" => {
                self.close_reader()?;
                if action == "shelf" { self.navigate("shelf"); } else { self.go_back()?; }
                return Ok(());
            }
            "menu" => {
                if let Some(state) = self.reading.as_mut() { state.menu = value == "open"; state.sheet.clear(); }
                self.reader_overlay_refresh(value == "open");
                return Ok(());
            }
            "sheet" => {
                if let Some(state) = self.reading.as_mut() {
                    state.menu = true;
                    state.sheet = value.to_owned();
                    if value == "grid" {
                        let page = state.core.as_ref().map_or(0, ReaderCore::page_index);
                        state.grid_page = page / if self.options.profile.is_scribe() { 15 } else { 12 } + 1;
                    }
                }
                if value == "grid" { self.request_reader_thumbnails()?; }
                if value == "light" { self.query_reader_lights(); }
                self.reader_overlay_refresh(true);
                return Ok(());
            }
            "guidance" => {
                if let Some(state) = self.reading.as_mut() { state.guidance = false; }
                self.preferences.reader_guidance_seen = true;
                self.save_preferences()?;
                self.reader_overlay_refresh(false);
                return Ok(());
            }
            "brightness" | "warmth" => { self.change_reader_light(action, value); return Ok(()); }
            "refresh" => {
                if let Some(state) = self.reading.as_mut() { state.menu = false; state.sheet.clear(); }
                if self.platform.capabilities().explicit_refresh {
                    if self.platform.submit_refresh(RefreshPlan { kind: RefreshKind::Full, rect: None }).is_err() { self.toast("common_not_supported"); }
                } else if !self.platform.is_kindle {
                    if let Some(ui) = self.ui.upgrade() { ui.window().request_redraw(); }
                } else { self.toast("common_not_supported"); }
                return Ok(());
            }
            "download" => {
                if let Some(id) = self.reading.as_ref().map(|state| state.episode_id.clone()) { self.enqueue_episode(&id, true)?; }
                return Ok(());
            }
            "purchase" | "next" => return self.open_reader_next(action == "purchase"),
            "episode" => {
                if let Some(comic) = self.reading.as_ref().map(|state| state.comic_id.clone()) { self.open_episode(&comic, value)?; }
                return Ok(());
            }
            "grid" => {
                let scribe = self.is_scribe();
                if let Some(state) = self.reading.as_mut() {
                    let count = state.original_index.as_ref().map_or(1, |index| index.images.len());
                    let pages = count.div_ceil(if scribe { 15 } else { 12 }).max(1);
                    state.grid_page = if value == "previous" { state.grid_page.saturating_sub(1).max(1) } else { (state.grid_page + 1).min(pages) };
                }
                self.request_reader_thumbnails()?;
                return Ok(());
            }
            "retry" => {
                if self.reading.as_ref().and_then(ReadingState::episode).is_some_and(|episode| !episode.readable_at(now())) {
                    if let Some((comic, episode)) = self.reading.as_ref().map(|state| (state.comic_id.clone(), state.episode_id.clone())) {
                        return self.request_reader_detail(&comic, Some(&episode));
                    }
                }
                let pending = self.reading.as_mut().and_then(|state| state.pending_index.take().map(|index| (state.generation, index, state.paged_anchor.clone(), state.comic_id.clone())));
                if let Some((generation, index, previous, comic_id)) = pending {
                    if let Some(previous) = previous { self.store.set_preference(&self.account, &format!("reader_previous_anchor_{comic_id}"), &previous)?; }
                    return self.install_reader_index(generation, index, None);
                }
                if self.reading.as_ref().is_some_and(|state| state.core.is_none()) {
                    if let Some((comic, episode)) = self.reading.as_ref().map(|state| (state.comic_id.clone(), state.episode_id.clone())) {
                        if episode.is_empty() { return self.open_reader_for_comic(&comic); }
                        return self.open_episode(&comic, &episode);
                    }
                }
                let needs_network = self.reading.as_ref().is_some_and(|state| state.error.as_deref() == Some(i18n::text("common_no_download").as_str()));
                if needs_network && !self.require_network("reader", "retry", "") && self.demo.is_none() { return Ok(()); }
                if let Some(state) = self.reading.as_mut() { state.error = None; state.version = state.version.wrapping_add(1); }
                self.enqueue_reader_prefetch(true)?;
                return self.request_reader_render();
            }
            _ => {}
        }
        if self.reading.is_none() { return Ok(()); }
        if action == "setting" { return self.change_reader_setting(value); }
        let queue = self.reading.as_ref().is_some_and(|state| state.core.is_none() || state.rendering
            && state.core.as_ref().is_some_and(|core| core.settings().mode != DisplayMode::Page || core.settings().fit != FitMode::Page));
        if queue {
            if let Some(state) = self.reading.as_mut() {
                if state.queued.len() < MAX_QUEUED_INTENTS {
                    state.queued.push_back((action.to_owned(), value.to_owned(), trace));
                    if let Some(trace) = trace { trace.mark("navigation_queued"); }
                } else if let Some(trace) = trace { trace.mark("navigation_queue_full"); }
            }
            return Ok(());
        }
        if action == "source-page" && self.reading.as_ref().is_some_and(|state| state.demo_strip) {
            self.change_reader_setting("mode:page")?;
        }
        let mut next = false;
        let mut changed = false;
        if let Some(state) = self.reading.as_mut() {
            let Some(core) = state.core.as_mut() else { return Ok(()); };
            match action {
                "turn" => {
                    let forward = value == "next";
                    if core.is_transition() && forward { next = true; }
                    else { changed = core.step(forward) != ReaderMove::AtStart; }
                }
                "page" | "source-page" => {
                    if let Ok(page) = value.parse::<usize>() {
                        core.set_page(page.saturating_sub(1).min(core.page_count() - 1))?;
                        changed = true;
                    }
                }
                "beginning" => { core.to_start()?; changed = true; }
                "last-view" => { core.back_to_last_view()?; changed = true; }
                "seek" => {
                    if let Ok(page) = value.parse::<usize>() {
                        if core.settings().mode == DisplayMode::Strip {
                            let target = page.saturating_sub(1).min(core.crop_count().saturating_sub(1));
                            while core.crop_index() < target { if core.step(true) != ReaderMove::Moved { break; } }
                            while core.crop_index() > target { if core.step(false) != ReaderMove::Moved { break; } }
                        } else { core.set_page(page.saturating_sub(1).min(core.page_count() - 1))?; }
                        changed = true;
                    }
                }
                _ => {}
            }
            if changed { state.version = state.version.wrapping_add(1); state.error = None; }
        }
        if next { return self.open_reader_next(false); }
        if changed {
            self.request_reader_render_traced(trace)?;
        } else if let Some(trace) = trace {
            trace.mark("navigation_unchanged");
        }
        Ok(())
    }

    fn open_reader_next(&mut self, purchase: bool) -> Result<()> {
        let next = self.reading.as_ref().and_then(|state| state.next_episode().cloned().map(|episode| (state.comic_id.clone(), episode)));
        let Some((comic, episode)) = next else { self.toast("read_latest_episode"); return Ok(()); };
        if purchase || !episode.readable_at(now()) {
            if purchase && episode.access == EpisodeAccess::Locked { self.save_reading_position()?; self.open_purchase(&episode.id)?; }
            else if purchase { self.toast("common_unavailable"); }
            return Ok(());
        }
        self.open_episode(&comic, &episode.id)
    }

    fn change_reader_setting(&mut self, setting: &str) -> Result<()> {
        let Some((key, value)) = setting.split_once(':') else { return Ok(()); };
        let Some(state) = self.reading.as_mut() else { return Ok(()); };
        match key {
            "mode" if ["page", "panel", "strip"].contains(&value) => { state.saved.mode = value.into(); self.preferences.mode = value.into(); }
            "fit" if ["page", "width"].contains(&value) => { state.saved.fit = value.into(); self.preferences.fit = value.into(); }
            "direction" if ["ltr", "rtl"].contains(&value) => { state.saved.direction = value.into(); self.preferences.direction = value.into(); }
            "refresh-policy" if ["smart", "0", "1", "6"].contains(&value) => { state.saved.every = value.into(); self.preferences.every = value.into(); }
            "landscape-layout" if ["spread", "single"].contains(&value) => { state.saved.landscape = value.into(); self.preferences.landscape = value.into(); }
            "cover-alone" if ["on", "off"].contains(&value) => { state.saved.cover_alone = value == "on"; self.preferences.cover_alone = value == "on"; }
            "dither" if ["on", "off"].contains(&value) => { state.saved.dither = value == "on"; self.preferences.dither = value == "on"; }
            "show-footer" if ["on", "off"].contains(&value) => { state.saved.footer = value == "on"; self.preferences.footer = value == "on"; }
            _ => return Ok(()),
        }
        state.next_frame.cancel();
        let saved = state.saved.clone();
        let comic = state.comic_id.clone();
        if key == "show-footer" {
            if let Some(core) = state.core.as_mut() {
                let mut settings = core.settings().clone();
                settings.footer = saved.footer;
                core.update_settings(settings)?;
            }
        } else {
            let strip = saved.mode == "strip" && self.demo.as_ref().is_some_and(|demo| demo.strip.is_some());
            if strip != state.demo_strip { self.switch_demo_strip(strip)?; }
            if let Some(state) = self.reading.as_mut() {
                if let Some(core) = state.core.as_mut() { core.update_settings(reader_settings(&saved, self.options.landscape, self.platform.logical_viewport_size()))?; core.set_landscape(self.options.landscape); }
                state.version = state.version.wrapping_add(1);
                state.error = None;
            }
        }
        let saved_comic = self.store.set_preference(&self.account, &format!("reader_preferences_{comic}"), &saved);
        let saved_defaults = self.save_preferences();
        if saved_comic.is_err() || saved_defaults.is_err() { self.toast("common_storage_error"); }
        if key == "show-footer" { return Ok(()); }
        self.configure_reader_refresh()?;
        if self.save_reading_position().is_err() { self.toast("common_storage_error"); }
        self.request_reader_render()?;
        Ok(())
    }

    fn switch_demo_strip(&mut self, enable: bool) -> Result<()> {
        if self.reading.as_ref().is_none_or(|state| state.demo_strip == enable) { return Ok(()); }
        self.save_reading_position()?;
        let Some(state) = self.reading.as_mut() else { return Ok(()); };
        if state.demo_strip == enable { return Ok(()); }
        state.next_frame.cancel();
        let Some(index) = state.original_index.clone() else { return Ok(()); };
        if let Some((anchor, false)) = state.displayed_anchor(now()) {
            state.paged_anchor = Some(anchor);
        }
        let settings = reader_settings(&state.saved, self.options.landscape, self.platform.logical_viewport_size());
        let mut core = if enable {
            let page_id = hex_digest(format!("design-strip:{}:{}", index.episode_id, index.revision).as_bytes());
            let strip_index = ImageIndex { episode_id: format!("{}:design-strip", index.episode_id),
                revision: format!("{}:strip", index.revision), images: vec![PageDescriptor { id: page_id, index: 0, width: 0, height: 0 }],
                ..Default::default() };
            let mut core = ReaderCore::new(strip_index, settings.clone())?;
            if let Some(anchor) = self.store.preference::<ReadingAnchor>(&self.account, &format!("reader_strip_anchor_{}", state.episode_id))? {
                if anchor.revision == core.index().revision { core.restore(&anchor)?; }
            }
            core
        } else {
            let mut core = ReaderCore::new(index, settings.clone())?;
            if let Some(anchor) = state.paged_anchor.as_ref() { core.restore(anchor)?; }
            attach_demo_panels(&mut core, self.demo.as_deref())?;
            core
        };
        core.update_settings(settings)?;
        core.set_landscape(self.options.landscape);
        state.core = Some(core);
        state.demo_strip = enable;
        state.version = state.version.wrapping_add(1);
        Ok(())
    }

    fn configure_reader_refresh(&self) -> Result<()> {
        let policy = self.reading.as_ref().map_or(self.preferences.every.as_str(), |state| state.saved.every.as_str());
        self.platform.set_reader_refresh_interval(if policy == "smart" { 0 } else { policy.parse().unwrap_or(6) })?;
        Ok(())
    }

    fn reader_files(&self, state: &ReadingState) -> Option<ReaderFiles> {
        if let Some(demo) = self.demo.as_ref() {
            Some(ReaderFiles::Demo { paths: demo.pages.get(&state.episode_id).cloned().unwrap_or_default(),
                strip: demo.strip.clone(), strip_active: state.demo_strip })
        } else { state.page_resolver.clone().or_else(|| self.downloads.as_ref().map(|manager| manager.page_resolver())).map(ReaderFiles::Account) }
    }

    fn retire_next_frame(&mut self) {
        if let Some(state) = self.reading.as_mut() {
            state.next_frame.cancel();
            state.next_frame = NextFrameSlot::Empty;
            state.next_frame_retry_pending = false;
        }
        self.invalidate("reader-prepare-next");
    }

    /// Consume one deferred opportunity after mailbox callbacks release their
    /// delivery permits. Busy or changed state does not schedule another retry.
    pub(super) fn retry_next_frame_after_delivery(&mut self) {
        let retry = self.reading.as_mut()
            .is_some_and(|state| std::mem::take(&mut state.next_frame_retry_pending));
        if retry { self.request_next_frame(); }
    }

    /// Honor the existing preload preference without moving the live reader.
    /// Re-enabling offers one immediate attempt; busy work remains a normal miss.
    pub(super) fn reader_preload_changed(&mut self) {
        if self.preferences.preload == 0 { self.retire_next_frame(); }
        else { self.request_next_frame(); }
    }

    fn request_next_frame(&mut self) {
        if self.preferences.preload == 0 { self.retire_next_frame(); return; }
        if self.is_suspended() || self.route != "reader" { return; }
        let Some(state) = self.reading.as_ref() else { return; };
        let Some(presented) = state.presented_core.as_ref() else { return; };
        let viewport = reader_viewport(self.platform.logical_viewport_size(), self.options.landscape,
            presented.settings(), self.platform.scale_factor());
        let valid_slot = match &state.next_frame {
            NextFrameSlot::Empty => true,
            NextFrameSlot::Preparing { key, .. } =>
                key.origin_is_current(state, self.epoch, viewport, self.options.landscape),
            NextFrameSlot::Ready(frame) =>
                frame.key.origin_is_current(state, self.epoch, viewport, self.options.landscape),
        };
        if !valid_slot {
            if let Some(state) = self.reading.as_mut() { state.next_frame.cancel(); }
        }
        let Some(state) = self.reading.as_ref() else { return; };
        if !matches!(state.next_frame, NextFrameSlot::Empty)
            || state.loading || state.rendering || !state.queued.is_empty() || state.error.is_some()
            || state.demo_strip || state.cancelled.load(Ordering::Acquire)
            || state.page.size().width == 0
            || !state.episode().is_some_and(|episode| episode.readable_at(now())) { return; }
        let Some(presented) = state.presented_core.as_ref() else { return; };
        if presented.is_transition() || presented.is_spread()
            || presented.settings().mode != DisplayMode::Page
            || presented.settings().fit != FitMode::Page { return; }
        let origin = FrameInput::new(presented);
        if !state.core.as_ref().is_some_and(|core| origin.matches(core)) { return; }
        let mut core = presented.clone();
        if core.step(true) != ReaderMove::Moved || core.is_transition() || core.is_spread()
            || core.page_index() == presented.page_index() { return; }
        let key = NextFrameKey { epoch: self.epoch, generation: state.generation,
            origin_version: state.version, origin, target: FrameInput::new(&core),
            viewport, landscape: self.options.landscape };
        // The exact allocation is checked again after rendering. This lower
        // bound rejects oversized canvases before pixel work or source I/O.
        let planned_bytes = u64::from(viewport.width).saturating_mul(u64::from(viewport.height))
            .saturating_mul(4).saturating_add(std::mem::size_of::<NextFrame>() as u64)
            .saturating_add(key.allocated_bytes())
            .saturating_add(core.retained_preparation_metadata_bytes());
        if planned_bytes > NEXT_FRAME_BYTES { return; }
        let Some(files) = self.reader_files(state) else { return; };
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let worker_key = key.clone();
        let apply_key = key.clone();
        let apply_cancelled = cancelled.clone();
        let trace = Operation::new("reader-prepare");
        if let Some(state) = self.reading.as_mut() {
            state.next_frame = NextFrameSlot::Preparing { key, cancelled: cancelled.clone() };
        }
        let admitted = self.background_speculative("reader-prepare-next", trace, cancelled,
            move || {
                let mut profile = trace.map(|_| crate::reader::ReaderPreparationProfile::default());
                let measured = trace.map(|_| Span::start());
                let result = prepare_next_frame(core, worker_key, &files, &worker_cancelled, profile.as_mut());
                if let Some(trace) = trace {
                    trace.record("next_frame_preparation", &serde_json::json!({
                        "ready": result.as_ref().is_ok_and(Option::is_some),
                        "cancelled": worker_cancelled.load(Ordering::Acquire),
                        "elapsed": measured.map(Span::finish), "profile": profile,
                        "stage_scope": crate::reader::ReaderPreparationProfile::STAGE_SCOPE,
                    }));
                }
                result
            }, move |app, result| app.apply_next_frame(apply_key, apply_cancelled, trace, result));
        if !admitted {
            if let Some(state) = self.reading.as_mut() { state.next_frame = NextFrameSlot::Empty; }
            if let Some(trace) = trace { trace.mark("next_frame_admission_skipped"); }
        }
    }

    fn apply_next_frame(&mut self, key: NextFrameKey, cancelled: Arc<AtomicBool>,
        trace: Option<Operation>, result: Result<Option<NextFrame>>,
    ) {
        let Some(state) = self.reading.as_ref() else { return; };
        if !matches!(&state.next_frame, NextFrameSlot::Preparing { key: active, cancelled: token }
            if active == &key && Arc::ptr_eq(token, &cancelled)) { return; }
        let current = state.presented_core.as_ref().is_some_and(|presented| {
            let viewport = reader_viewport(self.platform.logical_viewport_size(), self.options.landscape,
                presented.settings(), self.platform.scale_factor());
            key.origin_is_current(state, self.epoch, viewport, self.options.landscape)
        }) && !self.is_suspended() && self.route == "reader" && self.preferences.preload > 0
            && !cancelled.load(Ordering::Acquire)
            && state.episode().is_some_and(|episode| episode.readable_at(now()));
        let retry = cancelled.load(Ordering::Acquire) && self.epoch == key.epoch
            && state.generation == key.generation && state.version != key.origin_version;
        let Some(state) = self.reading.as_mut() else { return; };
        state.next_frame = NextFrameSlot::Empty;
        match result {
            Ok(Some(frame)) if current => {
                let retained_bytes = frame.retained_bytes();
                if frame.key == key && retained_bytes <= NEXT_FRAME_BYTES {
                    state.next_frame = NextFrameSlot::Ready(Box::new(frame));
                    if let Some(trace) = trace {
                        trace.record("next_frame_retained", &serde_json::json!({"retained_bytes": retained_bytes}));
                    }
                }
            }
            other => drop(other),
        }
        // An obsolete worker can finish after the replacement foreground frame.
        // Its callback still owns the sole speculative delivery permit here.
        // Offer the new origin once, after this callback returns to the UI tick.
        state.next_frame_retry_pending |= retry;
    }

    fn request_reader_render(&mut self) -> Result<()> {
        self.request_reader_render_traced(None)
    }

    fn request_reader_render_traced(&mut self, requested_trace: Option<Operation>) -> Result<()> {
        if self.preferences.preload == 0 { self.retire_next_frame(); }
        if self.is_suspended() || self.route != "reader" { return Ok(()); }
        if let Some(state) = self.reading.as_mut() {
            if matches!(state.next_frame, NextFrameSlot::Preparing { .. }) { state.next_frame.cancel(); }
        }
        let Some(state) = self.reading.as_ref() else { return Ok(()); };
        if state.cancel_obsolete_render() {
            if let Some(trace) = requested_trace {
                trace.record("obsolete_render_cancel_requested", &serde_json::json!({
                    "rendering_version": state.rendering_version, "target_version": state.version,
                }));
            }
        }
        if let Some(episode) = state.episode().filter(|episode| !episode.readable_at(now())) {
            self.fail_reader(reader_blocked_message(episode, now()));
            return Ok(());
        }
        if state.rendering {
            if let Some(state) = self.reading.as_mut() {
                state.render_again = true;
                if let Some(trace) = requested_trace {
                    if let Some(previous) = state.performance_request.replace(trace) {
                        previous.mark("request_superseded");
                    }
                    trace.mark("render_deferred");
                }
            }
            return Ok(());
        }
        let pending_trace = state.performance_request;
        let trace = requested_trace.or(pending_trace).or_else(|| Operation::new("reader"));
        if let (Some(previous), Some(requested)) = (pending_trace, requested_trace) {
            if previous.id() != requested.id() { previous.mark("request_superseded"); }
        }
        let first_request = requested_trace.is_some() || pending_trace.is_none();
        if first_request {
            if let Some(trace) = trace {
                trace.record("render_request", &serde_json::json!({
                    "epoch": self.epoch, "generation": state.generation, "version": state.version,
                    "source_kind": if self.demo.is_some() { "demo" } else { "receipt" },
                }));
            }
        }
        let Some(core) = state.core.clone() else { return Ok(()); };
        if core.is_transition() {
            self.platform.set_reader_performance_operation(None);
            if let Some(trace) = trace { trace.mark("reader_transition"); }
            if let Some(state) = self.reading.as_mut() {
                state.next_frame.cancel();
                state.loading = false; state.error = None; state.missing_pages.clear();
                state.presented_core = Some(core); state.presented_strip = state.demo_strip;
            }
            self.queue_reading_position()?;
            self.reader_turn_refresh(0.0, true);
            return Ok(());
        }
        let Some(files) = self.reader_files(state) else {
            // A manager temporarily belongs to a recovery/removal worker. Its return
            // synchronizes the current reader lease and resumes this local lookup.
            if self.demo.is_none() && self.downloads.is_none() {
                if let Some(state) = self.reading.as_mut() {
                    state.loading = true; state.error = None; state.performance_request = trace;
                }
                if first_request {
                    if let Some(trace) = trace { trace.mark("awaiting_resolver"); }
                }
            } else { self.fail_reader("common_no_download"); }
            return Ok(());
        };
        let generation = state.generation;
        let version = state.version;
        let cancelled = Arc::new(AtomicBool::new(false));
        let viewport = reader_viewport(self.platform.logical_viewport_size(), self.options.landscape, core.settings(), self.platform.scale_factor());
        let overview_size = if self.is_scribe() { (88, 125) } else { (58, 84) };
        let matches_next = self.preferences.preload > 0 && matches!(&state.next_frame, NextFrameSlot::Ready(frame)
            if frame.key.target_is_current(state, self.epoch, viewport, self.options.landscape));
        let next_frame = self.reading.as_mut().and_then(|state| state.next_frame.take_matching(|_| matches_next));
        if let Some(trace) = trace {
            trace.record("prepared_frame_lookup", &serde_json::json!({"candidate": next_frame.is_some()}));
        }
        if let Some(state) = self.reading.as_mut() {
            state.render_cancelled = cancelled.clone();
            state.rendering = true; state.rendering_version = Some(version);
            state.render_again = false; state.loading = true;
            state.render_started = Some(Instant::now()); state.error = None; state.missing_pages.clear();
            state.performance_request = None;
        }
        self.background_traced("reader-render", trace, move || {
            let mut core = core;
            let mut profile = trace.map(|_| crate::reader::ReaderPreparationProfile::default());
            let measured = trace.map(|_| Span::start());
            let prepared = if cancelled.load(Ordering::Acquire) { Ok(PreparedOutcome::Cancelled) }
                else if let Some(next_frame) = next_frame {
                    reuse_next_frame(&mut core, next_frame, &files, viewport, overview_size,
                        &cancelled, profile.as_mut(), trace)
                }
                else if let Some(profile) = profile.as_mut() {
                    prepare_reader_frame_profiled(&mut core, &files, viewport, overview_size, &cancelled, Some(profile))
                } else { prepare_reader_frame(&mut core, &files, viewport, overview_size, &cancelled) };
            let elapsed = measured.map(Span::finish);
            if let Some(trace) = trace {
                let outcome = match &prepared {
                    Ok(PreparedOutcome::Ready(_)) => "ready",
                    Ok(PreparedOutcome::Missing(_)) => "missing",
                    Ok(PreparedOutcome::Cancelled) => "cancelled",
                    Ok(PreparedOutcome::Failed) => "failed",
                    Err(_) => "failed",
                };
                trace.record("reader_preparation", &serde_json::json!({
                    "outcome": outcome, "elapsed": elapsed, "profile": profile,
                    "stage_scope": crate::reader::ReaderPreparationProfile::STAGE_SCOPE,
                }));
            }
            let outcome = prepared.unwrap_or(PreparedOutcome::Failed);
            Ok(PreparedReader { core, outcome })
        }, move |app, result| app.apply_prepared_reader(generation, version, trace, result));
        Ok(())
    }

    fn apply_prepared_reader(&mut self, generation: u64, version: u64, trace: Option<Operation>, result: Result<PreparedReader>) {
            let app = self;
            if !app.reader_is_current(generation) {
                if let Some(trace) = trace { trace.mark("reader_generation_retired"); }
                return;
            }
            if !app.reading.as_ref().is_some_and(|state| state.rendering_version == Some(version)) {
                if let Some(trace) = trace { trace.mark("reader_render_retired"); }
                return;
            }
            let current = app.reading.as_ref().is_some_and(|state| state.version == version);
            let retry_missing = app.reading.as_ref().is_some_and(|state| state.render_again);
            if let Some(state) = app.reading.as_mut() {
                state.rendering = false;
                state.rendering_version = None;
            }
            if !current {
                if let Some(trace) = trace { trace.mark("reader_result_superseded"); }
                let _ = app.request_reader_render();
                return;
            }
            if let Some(episode) = app.reading.as_ref().and_then(ReadingState::episode)
                .filter(|episode| !episode.readable_at(now())) {
                app.fail_reader(reader_blocked_message(episode, now()));
                return;
            }
            match result {
                Ok(prepared) => {
                    if !matches!(&prepared.outcome, PreparedOutcome::Cancelled) {
                        if let Some(state) = app.reading.as_mut() { state.core = Some(prepared.core); }
                    }
                    match prepared.outcome {
                        PreparedOutcome::Ready(frame) => {
                            let dark = frame.dark_fraction;
                            let copied_bytes = trace.map(|_| frame.page.rgba.len()
                                + frame.second.as_ref().map_or(0, |frame| frame.rgba.len())
                                + frame.overview.as_ref().map_or(0, |frame| frame.rgba.len()));
                            let measured = trace.map(|_| Span::start());
                            if let Some(state) = app.reading.as_mut() {
                                state.page = raster_image(frame.page);
                                state.second_page = frame.second.map(raster_image).unwrap_or_default();
                                state.overview = frame.overview.map(raster_image).unwrap_or_default();
                                state.metadata = frame.metadata;
                                state.presented_core = state.core.clone();
                                state.presented_strip = state.demo_strip;
                                state.missing_pages.clear();
                                state.render_again = false;
                                state.loading = false;
                                state.error = None;
                                state.performance_presented = trace;
                            }
                            if let (Some(trace), Some(measured)) = (trace, measured) {
                                trace.record("ui_image_import", &serde_json::json!({
                                    "elapsed": measured.finish(), "copied_bytes": copied_bytes,
                                }));
                            }
                            if app.queue_reading_position().is_err() { app.toast("common_storage_error"); }
                            app.reader_turn_refresh(dark, false);
                            app.report_reader_history();
                            if app.enqueue_reader_prefetch(false).is_err() { app.toast("common_source_error"); }
                        }
                        PreparedOutcome::Missing(missing) => {
                            if let Some(state) = app.reading.as_mut() { state.missing_pages = missing.clone(); }
                            if app.demo.is_some() { app.fail_reader("common_no_download"); }
                            else if app.options.offline || app.downloads.is_none() {
                                if app.options.offline {
                                    app.fail_reader("common_no_download");
                                    app.require_network("reader", "retry", "");
                                }
                            }
                            else if app.reader_acquisition_failed(&missing, false).unwrap_or(false) { app.fail_reader("common_source_error"); }
                            else if app.enqueue_reader_prefetch(false).is_err() { app.fail_reader("common_source_error"); }
                            if retry_missing { let _ = app.request_reader_render(); }
                        }
                        PreparedOutcome::Cancelled => {
                            // A cooperative cancellation is not a source failure.
                            // Any still-current target restarts only after this
                            // old worker and its actual compute work have ended.
                            if let Some(trace) = trace { trace.mark("reader_cancelled_result"); }
                            let _ = app.request_reader_render();
                            return;
                        },
                        PreparedOutcome::Failed => app.fail_reader("common_source_error"),
                    }
                }
                Err(_) => app.fail_reader("common_source_error"),
            }
            for _ in 0..MAX_QUEUED_INTENTS {
                let queued = app.reading.as_mut().filter(|state| !state.loading && !state.rendering && state.error.is_none()).and_then(|state| state.queued.pop_front());
                let Some((action, value, trace)) = queued else { break; };
                if let Some(trace) = trace { trace.mark("navigation_dequeued"); }
                if app.reader_action_traced(&action, &value, trace).is_err() { app.toast("common_error"); break; }
            }
            app.request_next_frame();
    }

    fn enqueue_reader_prefetch(&mut self, retry: bool) -> Result<()> {
        if self.is_suspended() || self.demo.is_some() || self.options.offline { return Ok(()); }
        if retry {
            if let Some(state) = self.reading.as_mut() { state.prefetch_retry = true; }
        }
        let Some(state) = self.reading.as_ref() else { return Ok(()); };
        if state.prefetch_held_after_sleep {
            if state.loading { self.fail_reader("dl_paused"); }
            return Ok(());
        }
        let retry = state.prefetch_retry;
        let Some(core) = state.core.as_ref() else { return Ok(()); };
        let Some(episode) = state.episode() else { return Ok(()); };
        if !episode.readable_at(now()) { self.fail_reader(reader_blocked_message(episode, now())); return Ok(()); }
        let visible = reader_visible_pages(core);
        let mut positions = visible.clone();
        let current = core.page_index();
        positions.extend(current.saturating_sub(1)..(current + self.preferences.preload.min(10) as usize + 1).min(core.page_count()));
        positions.sort_unstable(); positions.dedup();
        let mut missing = Vec::new();
        for position in positions {
            let page = &core.index().images[position];
            if self.store.page_receipt(&self.account, &state.episode_id, &core.index().revision, &page.id)?.is_none() { missing.push(position); }
        }
        if missing.is_empty() {
            if let Some(state) = self.reading.as_mut() { state.prefetch_retry = false; }
            return Ok(());
        }
        if !retry && self.reader_acquisition_failed(&missing, true)? { return Ok(()); }
        let generation = state.generation;
        let revision = core.index().revision.clone();
        let requested = missing.clone();
        let request = DownloadRequest { comic_id: state.comic_id.clone(), episode_id: state.episode_id.clone(),
            title: episode.title.clone(), comic_title: state.detail.as_ref().map_or_else(String::new, |detail| detail.comic.title.clone()),
            expected_revision: Some(core.index().revision.clone()), retain: false,
            reader_generation: Some(state.generation), page_indices: Some(missing) };
        let Some(manager) = self.downloads.as_mut() else { return Ok(()); };
        manager.set_concurrency(self.preferences.concurrency.clamp(1, 4) as usize)?;
        let record = manager.enqueue(request)?;
        if let Some(state) = self.reading.as_mut().filter(|state| state.generation == generation) {
            state.prefetch_retry = false;
            state.acquisition = Some(ReaderAcquisition { task_id: record.id, task_generation: record.generation,
                reader_generation: generation, revision, visible, requested });
        }
        Ok(())
    }

    fn reader_acquisition_failed(&self, missing: &[usize], require_all: bool) -> Result<bool> {
        let Some(state) = self.reading.as_ref() else { return Ok(false); };
        let (Some(core), Some(request)) = (state.core.as_ref(), state.acquisition.as_ref()) else { return Ok(false); };
        let Some(record) = self.store.load_download(&self.account, &request.task_id)? else { return Ok(false); };
        Ok(request.failed_for(&record, state.generation, &state.episode_id, &core.index().revision,
            &reader_visible_pages(core), missing, require_all))
    }

    pub(super) fn reader_page_ready(&mut self) -> Result<()> {
        if self.is_suspended() || self.route != "reader" { return Ok(()); }
        let waiting = self.reading.as_ref().is_some_and(|state| state.loading || state.error.is_some());
        if waiting {
            // The worker verifies visible bytes before a matching acquisition
            // failure is considered. Historical failures never hide a ready page.
            // A matching completion can race an earlier missing-file lookup.
            // Retry that missing result, but never discard an already valid frame.
            self.request_reader_render()?;
        }
        if self.reading.as_ref().is_some_and(|state| state.sheet == "grid") { self.request_reader_thumbnails()?; }
        Ok(())
    }

    pub(super) fn reader_download_dependency_changed(&mut self, event: &DownloadEvent) -> bool {
        if event.account_epoch != self.epoch { return false; }
        let Some(state) = self.reading.as_mut() else { return false; };
        let predicted_source = match &state.next_frame {
            NextFrameSlot::Preparing { key, .. } => key.source_notified(event),
            NextFrameSlot::Ready(frame) => frame.key.source_notified(event),
            NextFrameSlot::Empty => false,
        };
        if predicted_source { state.next_frame.cancel(); }
        state.depends_on_download(event)
    }

    fn cancel_reader_thumbnails(&mut self) {
        if let Some(state) = self.reading.as_mut() {
            state.thumbnail_cancelled.store(true, Ordering::Release);
            state.thumbnail_version = state.thumbnail_version.wrapping_add(1);
            state.thumbnails_rendering = false;
            state.thumbnails_pending = false;
            state.thumbnails_again = false;
        }
        self.invalidate("reader-thumbnails");
    }

    fn request_reader_thumbnails(&mut self) -> Result<()> {
        if self.is_suspended() { return Ok(()); }
        let per_page = if self.is_scribe() { 15 } else { 12 };
        let Some(state) = self.reading.as_ref() else { return Ok(()); };
        let Some(index) = state.original_index.clone() else { return Ok(()); };
        let generation = state.generation;
        let page = state.grid_page;
        let start = (page - 1) * per_page;
        let end = (start + per_page).min(index.images.len());
        let current = if state.demo_strip { state.paged_anchor.as_ref().map_or(0, |anchor| anchor.page_index) }
            else { state.core.as_ref().map_or(0, ReaderCore::page_index) };
        let dither = state.saved.dither;
        let demo_uncached = self.demo.is_some() && !self.options.offline;
        let columns = if self.is_scribe() { 5 } else { 4 };
        let mut logical = self.platform.logical_viewport_size();
        if self.options.landscape { logical = (logical.1, logical.0); }
        let scale = self.platform.scale_factor();
        let edge = if self.is_scribe() { 48 } else { 24 };
        let width = logical.0.saturating_sub(2 * edge + (columns - 1) * 14) / columns;
        let caption = if self.is_scribe() { 24 } else { 22 };
        let bar = if self.is_scribe() { 64 } else { 56 };
        let status = if self.is_scribe() { 32 } else { 28 };
        let thumb_height = (logical.1.saturating_sub(status + 2 * bar + 52) / 3).saturating_sub(caption + 4)
            .max(48).min(if self.is_scribe() { 180 } else { 118 });
        let size = (((width.saturating_sub(8)) as f32 * scale).round() as u32,
            (thumb_height.saturating_sub(8) as f32 * scale).round() as u32);
        let plan = ThumbnailPlan { revision: index.revision.clone(), start, end, size, dither };
        let replace = state.thumbnail_plan.as_ref() != Some(&plan);
        if replace {
            self.cancel_reader_thumbnails();
            if let Some(state) = self.reading.as_mut() {
                state.thumbnail_plan = Some(plan);
                state.thumbnail_proofs.clear();
                state.thumbnails = (start..end).map(|position| ReaderThumbnail { page: position as i32 + 1,
                    picture: slint::Image::default(), label: i18n::format("read_thumbnail_loading", &[("page", (position + 1).to_string())]).into(),
                    cached: false, current: position == current }).collect();
            }
        }
        if let Some(state) = self.reading.as_mut() {
            for item in &mut state.thumbnails {
                item.current = item.page == current as i32 + 1;
                if item.cached {
                    let label = if item.current { format!("{} · {}", item.page, i18n::text("read_current")) }
                        else { item.page.to_string() };
                    item.label = label.into();
                }
            }
            if state.thumbnails_rendering { state.thumbnails_again = true; return Ok(()); }
        }
        let Some(state) = self.reading.as_ref() else { return Ok(()); };
        let requested = state.thumbnails.iter()
            .map(|item| item.page.saturating_sub(1) as usize).collect::<Vec<_>>();
        if requested.is_empty() { return Ok(()); }
        let Some(mut files) = self.reader_files(state) else { return Ok(()); };
        if let ReaderFiles::Demo { strip_active, .. } = &mut files { *strip_active = false; }
        let version = state.thumbnail_version.wrapping_add(1);
        let proofs = state.thumbnail_proofs.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        if let Some(state) = self.reading.as_mut() {
            state.thumbnail_version = version;
            state.thumbnail_cancelled = cancelled.clone();
            state.thumbnails_rendering = true;
            state.thumbnails_pending = true;
            state.thumbnails_again = false;
        }
        self.background("reader-thumbnails", move || {
            let mut result = Vec::new();
            let mut prepared = BTreeMap::<String, Arc<Raster>>::new();
            if cancelled.load(Ordering::Acquire) { return Ok(result); }
            let mut core = ReaderCore::new(index.clone(), ReaderSettings { dither, landscape_spread: false, ..Default::default() })?;
            for position in requested {
                if cancelled.load(Ordering::Acquire) { break; }
                if demo_uncached && position >= 30 {
                    result.push(PreparedThumbnail { page: position, raster: None, proof: None, unchanged: false, failed: false });
                    continue;
                }
                let source = match files.source(&index, position, proofs.get(&position), &cancelled) {
                    Ok(Some(Some(source))) => source,
                    Ok(Some(None)) => {
                        result.push(PreparedThumbnail { page: position, raster: None,
                            proof: proofs.get(&position).cloned(), unchanged: true, failed: false });
                        continue;
                    },
                    outcome => {
                        result.push(PreparedThumbnail { page: position, raster: None, proof: None,
                            unchanged: false, failed: outcome.is_err() });
                        continue;
                    },
                };
                if cancelled.load(Ordering::Acquire) { break; }
                let proof = source.proof().clone();
                let raster = if let Some(raster) = prepared.get(proof.sha256()) { raster.clone() }
                    else {
                        core.set_page(position)?;
                        let Some(frame) = core.render_validated_source_cancellable(&source,
                            Viewport::new(size.0, size.1), &cancelled, None)? else { break; };
                        if cancelled.load(Ordering::Acquire) { break; }
                        let raster = Arc::new(Raster { rgba: frame.rgba, width: frame.width, height: frame.height });
                        prepared.insert(proof.sha256().to_owned(), raster.clone());
                        raster
                    };
                result.push(PreparedThumbnail { page: position, raster: Some(raster), proof: Some(proof), unchanged: false, failed: false });
            }
            if cancelled.load(Ordering::Acquire) { result.clear(); }
            Ok(result)
        }, move |app, result| {
            if !app.reader_is_current(generation) { return; }
            let same = app.reading.as_ref().is_some_and(|state| state.thumbnail_version == version && state.grid_page == page);
            if !same { return; }
            let repeat = app.reading.as_ref().is_some_and(|state| state.thumbnails_again);
            if let Some(state) = app.reading.as_mut() { state.thumbnails_rendering = false; }
                if let Ok(items) = result {
                    let failed = items.iter().any(|item| item.failed);
                    let mut pictures = BTreeMap::<usize, slint::Image>::new();
                    let thumbnails = items.into_iter().map(|item| {
                        if item.unchanged { return (item.page, item.proof, None); }
                        let cached = item.raster.is_some();
                        let label = if item.failed { format!("{} · {}", item.page + 1, i18n::text("read_unknown")) }
                            else if item.page == current { format!("{} · {}", item.page + 1, i18n::text("read_current")) }
                            else if !cached { format!("{} · {}", item.page + 1, i18n::text("read_not_cached")) } else { (item.page + 1).to_string() };
                        let picture = item.raster.map(|raster| {
                            let key = Arc::as_ptr(&raster) as usize;
                            pictures.entry(key).or_insert_with(|| raster_image_ref(&raster)).clone()
                        }).unwrap_or_default();
                        let thumbnail = ReaderThumbnail { page: (item.page + 1) as i32, picture,
                            label: label.into(), cached, current: item.page == current };
                        (item.page, item.proof, Some(thumbnail))
                    }).collect::<Vec<_>>();
                    if let Some(state) = app.reading.as_mut() {
                        for (page, proof, item) in thumbnails {
                            if let Some(proof) = proof { state.thumbnail_proofs.insert(page, proof); }
                            else { state.thumbnail_proofs.remove(&page); }
                            if let Some(item) = item {
                                if let Some(row) = state.thumbnails.iter_mut().find(|row| row.page == item.page) { *row = item; }
                            }
                        }
                        state.thumbnails_pending = false;
                    }
                    if failed { app.toast("read_overview_error"); }
                } else {
                    if let Some(state) = app.reading.as_mut() {
                        state.thumbnails_pending = false;
                        for item in &mut state.thumbnails {
                            if !item.cached { item.label = format!("{} · {}", item.page, i18n::text("read_unknown")).into(); }
                        }
                    }
                    app.toast("read_overview_error");
                }
            if repeat && app.reading.as_ref().is_some_and(|state| state.sheet == "grid") { let _ = app.request_reader_thumbnails(); }
        });
        Ok(())
    }

    fn current_reader_checkpoint(&self) -> Option<super::reader_checkpoint::ReaderCheckpoint> {
        let state = self.reading.as_ref()?;
        let (anchor, demo_strip) = state.displayed_anchor(now())?;
        Some(super::reader_checkpoint::ReaderCheckpoint { account: self.account.clone(), epoch: self.epoch,
            comic_id: state.comic_id.clone(), episode_id: state.episode_id.clone(), anchor, demo_strip })
    }

    fn queue_reading_position(&mut self) -> Result<()> {
        if let Some(checkpoint) = self.current_reader_checkpoint() {
            if !checkpoint.demo_strip {
                if let Some(state) = self.reading.as_mut() { state.paged_anchor = Some(checkpoint.anchor.clone()); }
            }
            self.lifecycle.reader_checkpoints.submit_latest(checkpoint)?;
        }
        self.lifecycle.reader_checkpoints.drive(&self.handle, self.store.path(), Instant::now())
    }

    pub(super) fn save_reading_position(&mut self) -> Result<()> {
        let current = self.current_reader_checkpoint();
        self.lifecycle.reader_checkpoints.flush(&self.store, current)
    }

    pub(super) fn flush_reader_checkpoints(&mut self) -> Result<()> { self.save_reading_position() }

    pub(super) fn retire_reader_checkpoints(&mut self) { self.lifecycle.reader_checkpoints.retire(); }

    pub(super) fn tick_reader_checkpoints(&mut self) {
        if self.is_suspended() || self.quitting { return; }
        if let Err(error) = self.lifecycle.reader_checkpoints.drive(&self.handle, self.store.path(), Instant::now()) {
            eprintln!("bilicomics: reader progress checkpoint warning: {error:#}");
            self.toast("common_storage_error");
        }
        if let (Some(state), Some(ui)) = (self.reading.as_ref(), self.ui.upgrade()) {
            let view = ui.global::<ReaderModel>();
            let feedback = state.preparation_feedback_due();
            if view.get_preparing_feedback() != feedback { view.set_preparing_feedback(feedback); }
        }
    }

    pub(super) fn reader_sleep_checkpoint(&self) -> Option<(String, String, ReadingAnchor, bool)> {
        let checkpoint = self.current_reader_checkpoint()
            .or_else(|| self.lifecycle.reader_checkpoints.pending_checkpoint().cloned())?;
        Some((checkpoint.comic_id, checkpoint.episode_id, checkpoint.anchor, checkpoint.demo_strip))
    }

    fn report_reader_history(&mut self) {
        if self.is_suspended() || self.demo.is_some() || self.options.offline { return; }
        let Some(state) = self.reading.as_mut() else { return; };
        if state.history_sent { return; }
        let Ok(client) = self.client.bound_to_account(&self.account) else { return; };
        state.history_sent = true;
        let comic = state.comic_id.clone();
        let episode = state.episode_id.clone();
        self.source_job("reader-history", async move { client.add_history(&comic, &episode).await }, |app, result| {
            if let Err(error) = result { app.source_failure(&error); }
        });
    }

    fn reader_turn_refresh(&mut self, dark_fraction: f32, transition: bool) {
        let Some(state) = self.reading.as_mut() else { return; };
        let spread = state.core.as_ref().is_some_and(ReaderCore::is_spread);
        let first = state.last_dark_fraction.is_none();
        // These conservative thresholds are image heuristics, not panel acceptance results.
        let smart_full = state.saved.every == "smart" && state.last_dark_fraction.is_some_and(|previous|
            dark_fraction > 0.30 && previous <= 0.30 || (dark_fraction - previous).abs() > 0.35);
        state.last_dark_fraction = Some(dark_fraction);
        if spread || first || smart_full || transition {
            let _ = self.platform.submit_refresh(RefreshPlan { kind: RefreshKind::Full, rect: None });
        } else { let _ = self.platform.request_refresh(RefreshReason::ReaderTurn); }
    }

    fn reader_overlay_refresh(&self, opened: bool) {
        let (mut width, mut height) = self.platform.logical_viewport_size();
        if self.options.landscape { std::mem::swap(&mut width, &mut height); }
        let rect = RefreshRect { x: 0, y: 0, width, height };
        let _ = self.platform.request_refresh(if opened { RefreshReason::OverlayOpened(rect) } else { RefreshReason::OverlayClosed(rect) });
    }

    fn query_reader_lights(&mut self) {
        if self.is_suspended() || self.demo.is_some() { return; }
        let Some((generation, version)) = self.reading.as_ref().filter(|state| !state.light_busy).map(|state| (state.generation, state.light_version)) else { return; };
        let platform = self.platform.clone();
        self.background("reader-light-read", move || {
            let capabilities = platform.capabilities();
            Ok((capabilities.brightness.then(|| platform.brightness().ok()).flatten(), capabilities.warmth.then(|| platform.warmth().ok()).flatten()))
        }, move |app, result| {
            if !app.reader_is_current(generation) { return; }
            if let (Some(state), Ok((brightness, warmth))) = (app.reading.as_mut(), result) {
                if state.light_version == version && !state.light_busy { state.brightness = brightness; state.warmth = warmth; }
            }
        });
    }

    fn change_reader_light(&mut self, kind: &str, value: &str) {
        if self.is_suspended() { return; }
        let delta = if value == "+1" { 1i16 } else if value == "-1" { -1 } else { return; };
        let Some(state) = self.reading.as_ref() else { return; };
        if state.light_busy { return; }
        let Some(current) = (if kind == "brightness" { state.brightness } else { state.warmth }) else { return; };
        let target = (i16::from(current) + delta).clamp(0, 24) as u8;
        let generation = state.generation;
        let brightness = kind == "brightness";
        if self.demo.is_some() {
            if let Some(state) = self.reading.as_mut() { if brightness { state.brightness = Some(target); } else { state.warmth = Some(target); } }
            return;
        }
        if let Some(state) = self.reading.as_mut() { state.light_busy = true; state.light_version = state.light_version.wrapping_add(1); }
        let platform = self.platform.clone();
        self.background("reader-light-write", move || {
            if brightness { platform.set_brightness(target)?; Ok(platform.brightness()?) }
            else { platform.set_warmth(target)?; Ok(platform.warmth()?) }
        }, move |app, result| {
            if !app.reader_is_current(generation) { return; }
            if let Some(state) = app.reading.as_mut() { state.light_busy = false; }
            match result {
                Ok(value) => { if let Some(state) = app.reading.as_mut() { if brightness { state.brightness = Some(value); } else { state.warmth = Some(value); } } }
                Err(_) => app.toast("common_not_supported"),
            }
        });
    }

    fn reader_cache_label(&self, state: &ReadingState) -> String {
        let current_ready = !state.loading && state.error.is_none() && state.metadata.focus_width > 0.0;
        if !current_ready { return i18n::text(if self.options.offline { "read_offline_cache_loading" } else { "read_cache_loading" }); }
        let Some(index) = state.original_index.as_ref() else { return i18n::text("read_local_page_cached"); };
        let total = index.images.len() as u32;
        let visible = state.core.as_ref().map_or(1, |core| reader_visible_pages(core).len() as u32).max(1).min(total);
        let inventory = if self.demo.is_some() {
            state.episode().map(|episode| {
                let complete = episode.extra["downloaded"].as_bool().unwrap_or(false);
                let present = if complete { total } else { episode.extra["downloaded_pages"].as_u64().unwrap_or(0).min(u64::from(total)) as u32 };
                (present, total, complete)
            })
        } else { self.reader_local_inventory_for_revision(&state.episode_id, &index.revision) }
            .filter(|(present, known_total, _)| *known_total == total && *present <= total);
        if self.options.offline {
            if inventory.is_some_and(|(present, known_total, complete)| complete && present == known_total) {
                return i18n::text("read_offline_reading");
            }
            return inventory.map(|(present, total, _)| i18n::format("read_offline_cache_count", &[
                ("present", present.max(visible).to_string()), ("total", total.to_string())]))
                .unwrap_or_else(|| i18n::text("read_offline_page_cached"));
        }
        if self.online {
            if self.demo.is_some() && !state.demo_strip {
                if let (Some((present, _, _)), Some(core)) = (inventory, state.core.as_ref()) {
                    let ahead = present.saturating_sub(core.page_index() as u32 + 1).min(self.preferences.preload.min(10));
                    if ahead > 0 { return i18n::format("read_online_ahead_cached", &[("count", ahead.to_string())]); }
                }
            }
            return inventory.map(|(present, total, _)| i18n::format("read_online_cache_count", &[
                ("present", present.max(visible).to_string()), ("total", total.to_string())]))
                .unwrap_or_else(|| i18n::text("read_online_page_cached"));
        }
        inventory.map(|(present, total, _)| i18n::format("read_local_cache_count", &[
            ("present", present.max(visible).to_string()), ("total", total.to_string())]))
            .unwrap_or_else(|| i18n::text("read_local_page_cached"))
    }

    pub(super) fn refresh_reader_view(&mut self, ui: &AppWindow) -> Result<()> {
        if self.is_suspended() { return Ok(()); }
        let Some(state) = self.reading.as_ref() else { return Ok(()); };
        let view = ui.global::<ReaderModel>();
        let core = if state.loading && !state.retain_frame_during_preparation() {
            state.core.as_ref()
        } else { state.visible_core() };
        let mode = core.map_or(state.saved.mode.as_str(), |core| match core.settings().mode { DisplayMode::Page => "page", DisplayMode::Panel => "panel", DisplayMode::Strip => "strip" });
        let page = core.map_or(1, |core| core.page_index() + 1);
        let count = core.map_or_else(|| state.pending_index.as_ref().map_or(1, |index| index.images.len()), ReaderCore::page_count);
        let transition = core.is_some_and(ReaderCore::is_transition);
        let expired = state.episode().is_some_and(|episode| !episode.readable_at(now()));
        let crop_index = core.map_or(0, ReaderCore::crop_index);
        let crop_count = core.map_or(1, ReaderCore::crop_count).max(1);
        let episode_title = state.episode().map(|episode| reader_episode_title(episode)).unwrap_or_default();
        let title = state.detail.as_ref().map_or_else(String::new, |detail| detail.comic.title.clone());
        let progress = if mode == "strip" { format!("{}{} / {}{}", i18n::text("read_page_prefix"), crop_index + 1, crop_count, i18n::text("read_screen_suffix")) }
            else if mode == "panel" { i18n::format("read_panel_progress", &[("page", page.to_string()), ("total", count.to_string()), ("panel", (crop_index + 1).to_string())]) }
            else { format!("{page} / {count}") };
        view.set_page(state.page.clone());
        view.set_second_page(state.second_page.clone());
        view.set_overview_image(state.overview.clone());
        view.set_page_index(page as i32);
        view.set_page_count(count as i32);
        view.set_seek_index(if mode == "strip" { crop_index + 1 } else { page } as i32);
        view.set_seek_count(if mode == "strip" { crop_count } else { count } as i32);
        view.set_left_page_index(state.metadata.left_page);
        view.set_right_page_index(state.metadata.right_page);
        let mut visible = [state.metadata.left_page, state.metadata.right_page].into_iter().filter(|page| *page >= 1 && (*page as usize) <= count).collect::<Vec<_>>();
        visible.sort_unstable();
        let pair = match visible.as_slice() { [first, last] => format!("{first}–{last}"), [first] => first.to_string(), _ => page.to_string() };
        view.set_spread_label(format!("{pair} / {count} · {}", i18n::text("read_pair_refresh")).into());
        view.set_comic_title(title.into());
        view.set_episode_title(episode_title.into());
        view.set_transition_episode_title(state.episode().map(reader_episode_summary).unwrap_or_default().into());
        view.set_progress_label(progress.into());
        view.set_cache_label(self.reader_cache_label(state).into());
        view.set_clock(ui.global::<ShellModel>().get_clock());
        let battery = ui.global::<ShellModel>().get_battery();
        view.set_battery_label(if battery.is_empty() || battery == "--%" { tr("read_unknown") } else { battery });
        view.set_network_label(if ui.global::<ShellModel>().get_network().is_empty() { tr("read_unknown") } else { ui.global::<ShellModel>().get_network() });
        view.set_connected(self.online && !self.options.offline);
        view.set_mode(mode.into());
        view.set_fit(core.map_or(state.saved.fit.as_str(), |core| if core.settings().fit == FitMode::Width { "width" } else { "page" }).into());
        view.set_direction(state.saved.direction.clone().into());
        view.set_refresh_policy(state.saved.every.clone().into());
        view.set_landscape_layout(state.saved.landscape.clone().into());
        view.set_landscape(self.options.landscape);
        view.set_spread_active(core.is_some_and(ReaderCore::is_spread));
        view.set_landscape_subtitle(i18n::format(if core.is_some_and(ReaderCore::is_spread) { "read_landscape_spread_description" } else { "read_landscape_single_description" }, &[
            ("direction", i18n::text(if state.saved.direction == "rtl" { "read_rtl" } else { "read_ltr" })),
            ("cover", i18n::text(if state.saved.cover_alone { "read_cover_alone" } else { "read_cover_with_first" })),
        ]).into());
        view.set_menu(state.menu);
        view.set_sheet(state.sheet.clone().into());
        view.set_guidance(state.guidance && !state.loading && state.error.is_none());
        view.set_transition(transition);
        view.set_loading(state.loading && !transition && !expired);
        view.set_retain_page(!expired && state.retain_frame_during_preparation());
        view.set_preparing_feedback(!expired && state.preparation_feedback_due());
        view.set_failed(expired || state.error.is_some() && !state.loading);
        view.set_error_message(if expired { state.episode().map(|episode| tr(reader_blocked_message(episode, now()))).unwrap_or_else(|| tr("common_unknown")) } else { state.error.clone().unwrap_or_default().into() });
        view.set_retry_label(if state.pending_index.is_some() { tr("read_open_updated_version") } else { "".into() });
        view.set_offline(self.options.offline);
        view.set_show_footer(state.saved.footer);
        view.set_dither(state.saved.dither);
        view.set_cover_alone(state.saved.cover_alone);
        view.set_zoom_available(false);
        view.set_dither_note(if state.saved.dither { tr("read_dither_software_note") } else { "".into() });
        view.set_panel_index((crop_index + 1) as i32);
        view.set_panel_count(crop_count as i32);
        view.set_strip_screen((crop_index + 1) as i32);
        view.set_strip_count(crop_count as i32);
        view.set_overlap(state.metadata.overlap);
        view.set_viewport_ratio(state.metadata.ratio.max(0.05));
        view.set_focus_x(state.metadata.focus_x);
        view.set_focus_y(state.metadata.focus_y);
        view.set_focus_width(state.metadata.focus_width);
        view.set_focus_height(state.metadata.focus_height);
        view.set_brightness(state.brightness.unwrap_or_default().into());
        view.set_warmth(state.warmth.unwrap_or_default().into());
        let capabilities = self.platform.capabilities();
        view.set_brightness_available(state.brightness.is_some() && (self.demo.is_some() || capabilities.brightness));
        view.set_warmth_available(state.warmth.is_some() && (self.demo.is_some() || capabilities.warmth));
        view.set_light_busy(state.light_busy);
        view.set_grid_page(state.grid_page as i32);
        view.set_grid_pages(state.original_index.as_ref().map_or(1, |index| index.images.len().div_ceil(if self.is_scribe() { 15 } else { 12 }).max(1)) as i32);
        sync_reader_rows(&state.thumbnail_model, &state.thumbnails, same_reader_thumbnail);
        let thumbnails: ModelRc<ReaderThumbnail> = state.thumbnail_model.clone().into();
        if view.get_thumbnails() != thumbnails { view.set_thumbnails(thumbnails); }
        view.set_thumbnails_loading(state.thumbnails_pending);
        let next = state.next_episode();
        view.set_next_available(next.is_some_and(|episode| episode.readable_at(now()) || episode.access == EpisodeAccess::Locked));
        view.set_next_locked(next.is_some_and(|episode| episode.access == EpisodeAccess::Locked));
        view.set_next_title(next.map(reader_episode_summary).unwrap_or_else(|| i18n::text("read_no_next_episode")).into());
        view.set_next_note(next.map(|episode| reader_next_note(episode, now(), self.demo.is_some())).unwrap_or_else(|| i18n::text("read_latest_episode")).into());
        view.set_continue_label(next.map(|episode| i18n::format("read_continue_episode_number", &[("order", display_order(episode.order))])).unwrap_or_else(|| i18n::text("read_no_next_episode")).into());
        view.set_next_price(next.and_then(|episode| episode.pay_gold).map(|amount| i18n::format("read_coin_price", &[("amount", display_order(amount))])).unwrap_or_else(|| i18n::text("read_unknown")).into());
        let (balance, coupons) = self.account_wallet_labels();
        view.set_wallet_label(if balance.replace(',', "").parse::<f64>().is_ok() { i18n::format("read_coin_price", &[("amount", balance)]).into() } else { balance.into() });
        view.set_coupon_label(if coupons.replace(',', "").parse::<f64>().is_ok() { i18n::format("read_coupon_quantity", &[("amount", coupons)]).into() } else { coupons.into() });
        let mut chapters = Vec::new();
        if let Some(detail) = state.detail.as_ref() {
            let current = detail.episodes.iter().position(|episode| episode.id == state.episode_id).unwrap_or(0);
            let start = current.saturating_sub(2).min(detail.episodes.len().saturating_sub(6));
            for episode in detail.episodes.iter().skip(start).take(6) {
                let tag = if episode.id == state.episode_id { i18n::text("read_reading") }
                    else if episode.read { i18n::text("common_read") }
                    else if self.demo.is_some() && episode.extra["access_label"].is_string() { episode.extra["access_label"].as_str().unwrap_or_default().to_owned() }
                    else { match episode.access { EpisodeAccess::Owned => i18n::text("common_owned"), EpisodeAccess::Free => i18n::text("common_free"), EpisodeAccess::Temporary => i18n::text("common_temporary"), EpisodeAccess::Locked => episode.pay_gold.map(|amount| i18n::format("read_coin_price", &[("amount", display_order(amount))])).unwrap_or_else(|| i18n::text("common_locked")), _ => i18n::text("common_unknown") } };
                chapters.push(ReaderChapter { id: episode.id.clone().into(), order: display_order(episode.order).into(), title: episode.title.clone().into(), tag: tag.into(), current: episode.id == state.episode_id, readable: episode.readable_at(now()), temporary: episode.access == EpisodeAccess::Temporary && episode.readable_at(now()) });
            }
            view.set_chapter_count(detail.comic.chapter_count as i32);
        }
        sync_reader_rows(&state.chapter_model, &chapters, PartialEq::eq);
        let chapters: ModelRc<ReaderChapter> = state.chapter_model.clone().into();
        if view.get_chapters() != chapters { view.set_chapters(chapters); }
        if let Some(trace) = state.performance_presented.filter(|trace| state.performance_model_id != Some(trace.id())) {
            self.platform.set_reader_performance_operation(Some(trace.id()));
            trace.mark("reader_model_published");
            if let Some(state) = self.reading.as_mut() { state.performance_model_id = Some(trace.id()); }
        }
        Ok(())
    }

    pub(super) fn reader_sleep_lease(&self) -> Option<ReaderSleepLease> {
        let state = self.reading.as_ref().filter(|state| self.route == "reader" && !state.cancelled.load(Ordering::Acquire))?;
        let index = state.original_index.as_ref()?;
        let request = state.acquisition.as_ref().filter(|request| request.reader_generation == state.generation && request.revision == index.revision);
        Some(ReaderSleepLease { generation: state.generation, episode_id: state.episode_id.clone(), revision: index.revision.clone(),
            task_id: request.map(|request| request.task_id.clone()), task_generation: request.map(|request| request.task_generation) })
    }

    pub(super) fn hold_reader_prefetch_after_sleep(&mut self, held: bool) {
        if let Some(state) = self.reading.as_mut() { state.prefetch_held_after_sleep = held; }
    }

    pub(super) fn reader_task_resumed(&mut self, record: &DownloadRecord) {
        if let Some(state) = self.reading.as_mut().filter(|state| state.episode_id == record.episode_id) {
            if let Some(request) = state.acquisition.as_mut().filter(|request| request.task_id == record.id && request.revision == record.revision) {
                request.task_generation = record.generation;
                state.prefetch_held_after_sleep = false;
            }
        }
    }

    /// Retire work for an undisplayed target without discarding the live frame.
    /// This gate performs no storage access and leaves initial loading intact.
    pub(super) fn retain_reader_frame_for_sleep(&mut self) {
        self.platform.set_reader_performance_operation(None);
        // Sleep does not drain the UI mailbox. Retire any completed speculative
        // pixels there while the actual worker retains its own compute lease.
        self.retire_next_frame();
        let Some(state) = self.reading.as_mut() else { return; };
        state.performance_model_id = None;
        let pending = state.rendering || state.loading || state.render_again || state.render_after_sleep
            || !state.queued.is_empty()
            || state.presented_core.as_ref().is_some_and(|presented| state.core.as_ref().is_some_and(|core|
                core.anchor(0) != presented.anchor(0) || core.settings() != presented.settings()))
            || state.demo_strip != state.presented_strip;
        for (_, _, trace) in state.queued.drain(..) {
            if let Some(trace) = trace { trace.mark("navigation_cancelled_for_sleep"); }
        }
        if !pending { return; }
        let Some(mut retained) = state.presented_core.clone() else { return; };
        let settings = state.core.as_ref().map(|core| core.settings().clone())
            .unwrap_or_else(|| retained.settings().clone());
        let reflow = settings != *retained.settings() || state.demo_strip != state.presented_strip;
        // Settings came from a valid live core. Keeping the old frame is safer
        // than replacing its source position if an internal settings invariant
        // cannot be preserved.
        if retained.update_settings(settings).is_err() { return; }
        retained.set_landscape(self.options.landscape);
        if let Some(trace) = state.performance_request.take() {
            trace.mark("request_cancelled_for_sleep");
        }
        state.render_cancelled.store(true, Ordering::Release);
        state.version = state.version.wrapping_add(1);
        state.core = Some(retained);
        state.demo_strip = state.presented_strip;
        state.rendering = false;
        state.rendering_version = None;
        state.render_again = false;
        state.render_after_sleep |= reflow;
        state.render_started = None;
        state.loading = false;
        state.error = None;
        state.missing_pages.clear();
        self.invalidate("reader-render");
    }

    pub(super) fn resume_reader_from_memory(&mut self) -> Result<()> {
        if self.is_suspended() { return Ok(()); }
        let reflow = self.reading.as_ref().filter(|state| state.render_after_sleep)
            .and_then(|state| state.core.as_ref().map(|core| core.settings().clone()));
        if let Some(settings) = reflow {
            if let Some(state) = self.reading.as_mut() { state.render_after_sleep = false; }
            if self.demo.is_some() {
                let strip = self.reading.as_ref().is_some_and(|state| state.saved.mode == "strip");
                self.switch_demo_strip(strip)?;
            }
            if let Some(core) = self.reading.as_mut().and_then(|state| state.core.as_mut()) {
                core.update_settings(settings)?;
                core.set_landscape(self.options.landscape);
            }
            self.request_reader_render()?;
        }
        if let Some(ui) = self.ui.upgrade() { self.refresh_reader_view(&ui)?; }
        self.request_next_frame();
        Ok(())
    }

    /// Inspect live UI references and cache accounting, including after reader close.
    pub fn reader_resource_snapshot(&self) -> serde_json::Value {
        let usage = crate::reader::ReaderResources::shared().cache_usage();
        let mut snapshot = serde_json::json!({
            "reader_active": self.reading.is_some(),
            "reader_generation": self.reading.as_ref().map(|state| state.generation),
            "page_size": null, "second_page_size": null, "overview_image_size": null,
            "thumbnails_count": null, "chapters_count": null,
            "source_cache_bytes": usage.retained_bytes, "source_cache_budget": usage.byte_budget,
            "source_cache_entries": usage.entries
        });
        snapshot["prepared_frame_state"] = serde_json::json!(self.reading.as_ref()
            .map_or("empty", |state| state.next_frame.label()));
        snapshot["prepared_frame_bytes"] = serde_json::json!(self.reading.as_ref()
            .map_or(0, |state| state.next_frame.retained_bytes()));
        snapshot["prepared_frame_budget"] = serde_json::json!(NEXT_FRAME_BYTES);
        snapshot["reader_checkpoint"] = serde_json::to_value(self.lifecycle.reader_checkpoints.diagnostics())
            .unwrap_or(serde_json::Value::Null);
        if let Some(ui) = self.ui.upgrade() {
            let view = ui.global::<ReaderModel>();
            let page = view.get_page().size();
            let second = view.get_second_page().size();
            let overview = view.get_overview_image().size();
            snapshot["page_size"] = serde_json::json!({ "width": page.width, "height": page.height });
            snapshot["second_page_size"] = serde_json::json!({ "width": second.width, "height": second.height });
            snapshot["overview_image_size"] = serde_json::json!({ "width": overview.width, "height": overview.height });
            snapshot["thumbnails_count"] = serde_json::json!(view.get_thumbnails().row_count());
            snapshot["chapters_count"] = serde_json::json!(view.get_chapters().row_count());
        }
        snapshot
    }

    /// A credential-free proof of the retained source position and reader identity.
    pub fn reader_lifecycle_snapshot(&self) -> Option<serde_json::Value> {
        let state = self.reading.as_ref()?;
        let core = state.visible_core()?;
        let anchor = core.anchor(0);
        let frame = state.page.size();
        let second = state.second_page.size();
        let viewport = reader_viewport(self.platform.logical_viewport_size(), self.options.landscape,
            core.settings(), self.platform.scale_factor());
        let mut snapshot = serde_json::json!({
            "comic_id": state.comic_id, "episode_id": state.episode_id,
            "reader_generation": state.generation, "route": self.route,
            "launch_preference": self.preferences.launch,
            "revision": anchor.revision, "page_id": anchor.page_id,
            "page_index": anchor.page_index, "page_number": anchor.page_index + 1,
            "crop_index": core.crop_index(), "crop_count": core.crop_count(),
            "source_x": anchor.source_x, "source_y": anchor.source_y,
            "zoom_ratio": anchor.zoom_ratio, "rotation": anchor.rotation,
            "mode": state.saved.mode, "core_mode": anchor.mode,
            "fit": state.saved.fit, "direction": state.saved.direction,
            "landscape_layout": state.saved.landscape, "cover_alone": state.saved.cover_alone,
            "dither": state.saved.dither, "footer": state.saved.footer
        });
        let presentation = serde_json::json!({
            "demo_strip": state.demo_strip, "transition": core.is_transition(),
            "menu": state.menu, "sheet": state.sheet,
            "render_version": state.version, "rendering": state.rendering,
            "effective_spread": core.is_spread(),
            "viewport_width": viewport.width, "viewport_height": viewport.height,
            "left_page_number": state.metadata.left_page, "right_page_number": state.metadata.right_page,
            "focus_x": state.metadata.focus_x, "focus_y": state.metadata.focus_y,
            "focus_width": state.metadata.focus_width, "focus_height": state.metadata.focus_height,
            "frame_width": frame.width, "frame_height": frame.height,
            "second_frame_width": second.width, "second_frame_height": second.height,
            "thumbnails_loading": state.thumbnails_pending, "thumbnails_count": state.thumbnails.len(),
            "thumbnails_loaded_count": state.thumbnails.iter().filter(|row| row.cached).count(),
            "thumbnail_version": state.thumbnail_version, "thumbnails_rendering": state.thumbnails_rendering,
            "chapters_count": state.chapter_model.row_count()
        });
        snapshot.as_object_mut()?.extend(presentation.as_object()?.clone());
        let usage = crate::reader::ReaderResources::shared().cache_usage();
        let resources = serde_json::json!({ "source_cache_bytes": usage.retained_bytes,
            "source_cache_budget": usage.byte_budget, "source_cache_entries": usage.entries });
        snapshot.as_object_mut()?.extend(resources.as_object()?.clone());
        Some(snapshot)
    }
}

fn same_reader_thumbnail(current: &ReaderThumbnail, next: &ReaderThumbnail) -> bool {
    current.page == next.page && current.label == next.label && current.cached == next.cached
        && current.current == next.current && (current.picture == next.picture || {
            // Slint 1.17.1 considers two empty images unequal; loaded buffers retain identity.
            let current_size = current.picture.size();
            let next_size = next.picture.size();
            current_size.width == 0 && current_size.height == 0
                && next_size.width == 0 && next_size.height == 0
        })
}

fn sync_reader_rows<T: Clone + 'static>(model: &VecModel<T>, rows: &[T], same: impl Fn(&T, &T) -> bool) {
    let previous_count = model.row_count();
    for (row, value) in rows.iter().take(previous_count).enumerate() {
        if model.row_data(row).as_ref().is_none_or(|current| !same(current, value)) {
            model.set_row_data(row, value.clone());
        }
    }
    // Keep existing repeater instances alive while images and labels become ready.
    for row in (rows.len()..previous_count).rev() { model.remove(row); }
    for value in rows.iter().skip(previous_count) { model.push(value.clone()); }
}

fn reader_settings(saved: &SavedReaderPreferences, landscape: bool, logical: (u32, u32)) -> ReaderSettings {
    ReaderSettings {
        mode: match saved.mode.as_str() { "panel" => DisplayMode::Panel, "strip" => DisplayMode::Strip, _ => DisplayMode::Page },
        fit: if saved.fit == "width" { FitMode::Width } else { FitMode::Page },
        direction: if saved.direction == "rtl" { PageDirection::RightToLeft } else { PageDirection::LeftToRight },
        landscape_spread: landscape && logical.1 >= 1100 && saved.landscape == "spread", cover_alone: saved.cover_alone,
        dither: saved.dither, footer: saved.footer, ..Default::default()
    }
}

fn reader_visible_pages(core: &ReaderCore) -> Vec<usize> {
    let mut visible = core.spread_indices().into_iter().flatten().collect::<Vec<_>>();
    visible.sort_unstable();
    visible.dedup();
    visible
}

fn reader_source_message(kind: ErrorKind) -> &'static str {
    match kind { ErrorKind::Authentication | ErrorKind::AccountMismatch => "common_session_expired", ErrorKind::Locked => "common_locked",
        ErrorKind::Network | ErrorKind::Http => "common_network_error", ErrorKind::Storage => "common_storage_error", _ => "common_source_error" }
}

fn reader_blocked_message(episode: &Episode, timestamp: i64) -> &'static str {
    if episode.access == EpisodeAccess::Unknown { "common_unknown" }
    else if episode.access == EpisodeAccess::Locked { "common_locked" }
    else if episode.expires_at.is_some_and(|expiry| expiry > 0 && expiry <= timestamp) { "read_access_expired" }
    else { "common_unavailable" }
}

fn reader_next_note(episode: &Episode, timestamp: i64, demo: bool) -> String {
    if !episode.readable_at(timestamp) {
        return i18n::text(if episode.access == EpisodeAccess::Locked { "read_next_locked_note" } else { reader_blocked_message(episode, timestamp) });
    }
    match episode.access {
        EpisodeAccess::Owned => i18n::text("read_next_ready_note"),
        EpisodeAccess::Free => i18n::text("read_next_free_note"),
        EpisodeAccess::Temporary => {
            let access = if demo { episode.extra["access_label"].as_str().map(str::to_owned) } else { None }
                .or_else(|| episode.expires_at.filter(|expiry| *expiry > 0 && *expiry > timestamp).and_then(|expiry| chrono::DateTime::from_timestamp(expiry, 0))
                    .map(|expiry| i18n::format("read_temporary_until", &[("time", (expiry + chrono::Duration::hours(8)).format("%Y-%m-%d %H:%M").to_string())])));
            access.map(|access| i18n::format("read_next_access_note", &[("access", access)]))
                .unwrap_or_else(|| i18n::text("read_next_temporary_note"))
        }
        _ => i18n::text("common_unknown"),
    }
}

fn reader_episode_title(episode: &Episode) -> String {
    i18n::format("read_episode_title", &[("order", display_order(episode.order)), ("title", episode.title.clone())])
}

fn reader_episode_summary(episode: &Episode) -> String {
    i18n::format("read_episode_summary", &[("order", display_order(episode.order)), ("title", episode.title.clone())])
}

fn attach_demo_panels(core: &mut ReaderCore, demo: Option<&DemoData>) -> Result<()> {
    let Some(demo) = demo else { return Ok(()); };
    for page in core.index().images.clone() {
        if let Some(panels) = demo.panels.get(&page.id) {
            core.set_panel_map(&page.id, panels.iter().map(|panel| NormalizedRect { x: panel.x, y: panel.y, width: panel.width, height: panel.height }).collect())?;
        }
    }
    Ok(())
}

fn reader_viewport(mut logical: (u32, u32), landscape: bool, settings: &ReaderSettings, scale: f32) -> Viewport {
    if landscape { logical = (logical.1, logical.0); }
    let columns = if landscape && settings.landscape_spread && settings.mode == DisplayMode::Page && settings.fit == FitMode::Page { 2.0 } else { 1.0 };
    Viewport { width: (logical.0.max(1) as f32 * scale / columns).round() as u32,
        height: (logical.1.max(1) as f32 * scale).round() as u32, scale_factor: scale }
}

fn prepare_reader_frame(core: &mut ReaderCore, files: &ReaderFiles, viewport: Viewport, overview_size: (u32, u32), cancelled: &AtomicBool) -> Result<PreparedOutcome> {
    prepare_reader_frame_profiled(core, files, viewport, overview_size, cancelled, None)
}

fn prepare_reader_frame_profiled(core: &mut ReaderCore, files: &ReaderFiles, viewport: Viewport,
    overview_size: (u32, u32), cancelled: &AtomicBool,
    mut profile: Option<&mut crate::reader::ReaderPreparationProfile>,
) -> Result<PreparedOutcome> {
    let index = core.index();
    let primary_index = core.page_index();
    let spread = core.is_spread();
    let pair = if spread { core.spread_indices() } else { [Some(primary_index), None] };
    let mut sources = Vec::new();
    let mut missing = Vec::new();
    for position in pair.into_iter().flatten() {
        if cancelled.load(Ordering::Acquire) { return Ok(PreparedOutcome::Cancelled); }
        let source = files.source_profiled(&index, position, None, cancelled, profile.as_deref_mut());
        if cancelled.load(Ordering::Acquire) { return Ok(PreparedOutcome::Cancelled); }
        if let Some(Some(source)) = source? { sources.push((position, source)); }
        else { missing.push(position); }
    }
    if cancelled.load(Ordering::Acquire) { return Ok(PreparedOutcome::Cancelled); }
    if !missing.is_empty() { return Ok(PreparedOutcome::Missing(missing)); }
    render_reader_sources(core, sources, viewport, overview_size, cancelled, profile)
}

fn render_reader_sources(core: &mut ReaderCore,
    sources: Vec<(usize, crate::reader::ValidatedGraySource)>, viewport: Viewport,
    overview_size: (u32, u32), cancelled: &AtomicBool,
    mut profile: Option<&mut crate::reader::ReaderPreparationProfile>,
) -> Result<PreparedOutcome> {
    let index = core.index().clone();
    let primary_index = core.page_index();
    let spread = core.is_spread();
    let pair = if spread { core.spread_indices() } else { [Some(primary_index), None] };
    let source = &sources.iter().find(|(position, _)| *position == primary_index).or_else(|| sources.first())
        .context("The visible page has no verified source")?.1;
    if cancelled.load(Ordering::Acquire) { return Ok(PreparedOutcome::Cancelled); }
    let Some(rendered) = core.render_validated_source_cancellable(&source, viewport, cancelled,
        profile.as_deref_mut())? else { return Ok(PreparedOutcome::Cancelled); };
    if cancelled.load(Ordering::Acquire) { return Ok(PreparedOutcome::Cancelled); }
    let mut metadata = FrameMetadata { overlap: rendered.overlap_from_previous, ratio: rendered.crop.width as f32 / rendered.crop.height.max(1) as f32,
        focus_x: rendered.crop.x as f32 / rendered.source_width.max(1) as f32,
        focus_y: rendered.crop.y as f32 / rendered.source_height.max(1) as f32,
        focus_width: rendered.crop.width as f32 / rendered.source_width.max(1) as f32,
        focus_height: rendered.crop.height as f32 / rendered.source_height.max(1) as f32, ..Default::default() };
    let dark_fraction = rendered.dark_fraction;
    let mut primary = Some(rendered_raster(rendered));
    let mut second = None;
    let mut page = None;
    if spread {
        for (slot, position) in pair.into_iter().enumerate() {
            let raster = if let Some(position) = position {
                if position == primary_index { primary.take() }
                else {
                    if cancelled.load(Ordering::Acquire) { return Ok(PreparedOutcome::Cancelled); }
                    let leaf_source = &sources.iter().find(|(index, _)| *index == position)
                        .context("A spread leaf is unavailable")?.1;
                    let mut leaf = ReaderCore::new(index.clone(), core.settings().clone())?;
                    leaf.set_landscape(false);
                    leaf.set_page(position)?;
                    if cancelled.load(Ordering::Acquire) { return Ok(PreparedOutcome::Cancelled); }
                    let Some(rendered) = leaf.render_validated_source_cancellable(leaf_source, viewport, cancelled,
                        profile.as_deref_mut())? else { return Ok(PreparedOutcome::Cancelled); };
                    Some(rendered_raster(rendered))
                }
            } else { None };
            let number = position.map_or_else(|| if primary_index == 0 { 0 } else { index.images.len() as i32 + 1 }, |position| position as i32 + 1);
            if slot == 0 { page = raster; metadata.left_page = number; }
            else { second = raster; metadata.right_page = number; }
        }
    } else { page = primary.take(); metadata.left_page = primary_index as i32 + 1; }
    let overview = if core.settings().mode == DisplayMode::Panel {
        if cancelled.load(Ordering::Acquire) { return Ok(PreparedOutcome::Cancelled); }
        let mut overview = ReaderCore::new(index, ReaderSettings { dither: core.settings().dither, ..Default::default() })?;
        overview.set_page(primary_index)?;
        let size = Viewport::new(overview_size.0, overview_size.1);
        let Some(rendered) = overview.render_validated_source_cancellable(&source, size, cancelled,
            profile.as_deref_mut())? else { return Ok(PreparedOutcome::Cancelled); };
        Some(rendered_raster(rendered))
    } else { None };
    if cancelled.load(Ordering::Acquire) { return Ok(PreparedOutcome::Cancelled); }
    // An empty physical spread leaf still occupies its own source-independent slot.
    let page = page.unwrap_or_else(|| Raster { rgba: vec![255; 4], width: 1, height: 1 });
    Ok(PreparedOutcome::Ready(PreparedFrame { page, second, overview, metadata, dark_fraction }))
}

fn prepare_next_frame(mut core: ReaderCore, key: NextFrameKey, files: &ReaderFiles,
    cancelled: &AtomicBool, mut profile: Option<&mut crate::reader::ReaderPreparationProfile>,
) -> Result<Option<NextFrame>> {
    if cancelled.load(Ordering::Acquire) { return Ok(None); }
    let position = core.page_index();
    // This local-only lookup is also the existence check. A receipt lookup can
    // perform integrity work and therefore never belongs on the UI thread.
    let source = files.source_profiled(core.index(), position, None, cancelled, profile.as_deref_mut())?;
    if cancelled.load(Ordering::Acquire) { return Ok(None); }
    let Some(Some(source)) = source else { return Ok(None); };
    let proof = source.proof().clone();
    let outcome = render_reader_sources(&mut core, vec![(position, source)], key.viewport,
        (1, 1), cancelled, profile)?;
    if cancelled.load(Ordering::Acquire) { return Ok(None); }
    let PreparedOutcome::Ready(frame) = outcome else { return Ok(None); };
    let candidate = NextFrame { key, core, frame, proof };
    if candidate.retained_bytes() > NEXT_FRAME_BYTES { return Ok(None); }
    Ok(Some(candidate))
}

fn reuse_next_frame(core: &mut ReaderCore, next: NextFrame, files: &ReaderFiles,
    viewport: Viewport, overview_size: (u32, u32), cancelled: &AtomicBool,
    mut profile: Option<&mut crate::reader::ReaderPreparationProfile>, trace: Option<Operation>,
) -> Result<PreparedOutcome> {
    if cancelled.load(Ordering::Acquire) { return Ok(PreparedOutcome::Cancelled); }
    let source = files.source_profiled(core.index(), core.page_index(), Some(&next.proof),
        cancelled, profile.as_deref_mut());
    if cancelled.load(Ordering::Acquire) { return Ok(PreparedOutcome::Cancelled); }
    match source? {
        Some(None) => {
            // Only a fresh read/hash and the resolver's current receipt/object
            // checks can authorize using the previously rendered pixels.
            if let Some(trace) = trace { trace.mark("prepared_frame_hit"); }
            *core = next.core;
            Ok(PreparedOutcome::Ready(next.frame))
        }
        Some(Some(source)) => {
            if let Some(trace) = trace { trace.mark("prepared_frame_source_changed"); }
            // Drop old pixels before the fallback allocates a replacement.
            drop(next);
            let position = core.page_index();
            render_reader_sources(core, vec![(position, source)], viewport,
                overview_size, cancelled, profile)
        }
        None => {
            if let Some(trace) = trace { trace.mark("prepared_frame_source_missing"); }
            Ok(PreparedOutcome::Missing(vec![core.page_index()]))
        }
    }
}

fn rendered_raster(rendered: RenderedPage) -> Raster {
    Raster { rgba: rendered.rgba, width: rendered.width, height: rendered.height }
}

fn raster_image(raster: Raster) -> slint::Image {
    raster_image_ref(&raster)
}

fn raster_image_ref(raster: &Raster) -> slint::Image {
    slint::Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&raster.rgba, raster.width, raster.height))
}

#[cfg(test)]
#[path = "immersive_reader_tests.rs"]
mod immersive_reader_tests;

#[cfg(test)]
#[path = "reader_performance_tests.rs"]
mod reader_performance_tests;

#[cfg(test)]
#[path = "prepared_reader_tests.rs"]
mod prepared_reader_tests;

#[cfg(test)]
#[path = "prepared_reader_source_tests.rs"]
mod prepared_reader_source_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn acquisition() -> ReaderAcquisition {
        ReaderAcquisition { task_id: "reader-task".into(), task_generation: 7, reader_generation: 41,
            revision: "current-revision".into(), visible: vec![17, 18], requested: vec![18, 19, 20] }
    }

    fn failed_record() -> DownloadRecord {
        DownloadRecord { id: "reader-task".into(), generation: 7, episode_id: "episode-12".into(),
            revision: "current-revision".into(), status: DownloadStatus::Failed, ..Default::default() }
    }

    #[test]
    fn current_missing_leaf_uses_only_its_owned_acquisition_failure() {
        let request = acquisition();
        let record = failed_record();
        assert!(request.failed_for(&record, 41, "episode-12", "current-revision", &[17, 18], &[18], false));
        // A ready visible page wins even when an adjacent prefetch failed.
        assert!(!request.failed_for(&record, 41, "episode-12", "current-revision", &[17, 18], &[], false));
        // This visible page was not part of the failed acquisition.
        assert!(!request.failed_for(&record, 41, "episode-12", "current-revision", &[17, 18], &[17], false));
    }

    #[test]
    fn historical_retained_reader_revision_and_task_generations_cannot_mask_new_pages() {
        let request = acquisition();
        let original = failed_record();
        let mut history = original.clone();
        history.retained = true;
        assert!(!request.failed_for(&history, 41, "episode-12", "current-revision", &[17, 18], &[18], false));
        history = original.clone(); history.generation = 6;
        assert!(!request.failed_for(&history, 41, "episode-12", "current-revision", &[17, 18], &[18], false));
        history = original.clone(); history.revision = "old-revision".into();
        assert!(!request.failed_for(&history, 41, "episode-12", "current-revision", &[17, 18], &[18], false));
        assert!(!request.failed_for(&original, 42, "episode-12", "current-revision", &[17, 18], &[18], false));
        assert!(!request.failed_for(&original, 41, "episode-12", "current-revision", &[19, 20], &[19], false));
    }

    #[test]
    fn newly_missing_pages_can_start_a_new_window_without_blindly_replaying_old_failure() {
        let request = acquisition();
        let record = failed_record();
        assert!(request.failed_for(&record, 41, "episode-12", "current-revision", &[17, 18], &[18, 19], true));
        // A newly invalidated current page was not in the old failed request.
        assert!(!request.failed_for(&record, 41, "episode-12", "current-revision", &[17, 18], &[17, 18], true));
    }

    #[test]
    fn spread_requires_an_eligible_large_landscape_canvas() {
        let saved = SavedReaderPreferences::default();
        assert!(!reader_settings(&saved, true, (618, 824)).landscape_spread);
        assert!(!reader_settings(&saved, true, (632, 840)).landscape_spread);
        assert!(!reader_settings(&saved, false, (930, 1240)).landscape_spread);
        assert!(reader_settings(&saved, true, (930, 1240)).landscape_spread);
        let mut single = saved;
        single.landscape = "single".into();
        let settings = reader_settings(&single, true, (930, 1240));
        assert!(!settings.landscape_spread);
        assert_eq!(settings.fit, FitMode::Page);
        single.fit = "width".into();
        assert_eq!(reader_settings(&single, true, (930, 1240)).fit, FitMode::Width);
    }

    #[test]
    fn readable_access_and_unknown_prices_do_not_fabricate_ownership() {
        let free = Episode { access: EpisodeAccess::Free, pay_gold: Some(32.0), ..Default::default() };
        assert_eq!(reader_next_note(&free, 100, false), i18n::text("read_next_free_note"));
        let temporary = Episode { access: EpisodeAccess::Temporary, expires_at: Some(200), ..Default::default() };
        let note = reader_next_note(&temporary, 100, false);
        assert!(!note.contains(&i18n::text("common_owned")));
        assert_eq!(reader_blocked_message(&temporary, 201), "read_access_expired");
        let no_expiry = Episode { access: EpisodeAccess::Temporary, expires_at: Some(0), ..Default::default() };
        assert_eq!(reader_next_note(&no_expiry, 100, false), i18n::text("read_next_temporary_note"));
        let unknown = Episode { access: EpisodeAccess::Unknown, pay_gold: Some(32.0), ..Default::default() };
        assert_eq!(reader_blocked_message(&unknown, 100), "common_unknown");
        assert_eq!(reader_next_note(&unknown, 100, false), i18n::text("common_unknown"));
    }
}

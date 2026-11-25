//! Prepare one account-scoped sleep cover outside the power-event critical path.

use super::*;
use crate::ui::SleepScreenModel;
use std::io::Cursor;
use tokio::sync::Notify;

const MAX_COVER_WIDTH: u32 = 1536;
const MAX_COVER_HEIGHT: u32 = 2048;
const MAX_ENCODED_BYTES: usize = 8 * 1024 * 1024;
const RETRY_DELAY: Duration = Duration::from_secs(30);
const CAPACITY_RETRY: Duration = Duration::from_secs(1);
const READER_IDLE_DELAY: Duration = Duration::from_secs(5);
const LOCAL_JOB: &str = "cover-sleep-local";
const SOURCE_JOB: &str = "cover-sleep-source";
const PREPARE_JOB: &str = "cover-sleep-prepare";

#[derive(Clone, Debug, Eq, PartialEq)]
struct CoverIdentity {
    account: String,
    epoch: u64,
    comic_id: String,
    file_name: String,
}

impl CoverIdentity {
    fn new(account: &str, epoch: u64, comic: &Comic) -> Option<Self> {
        if account.len() > 4096 || comic.id.len() > 4096 || comic.cover_url.len() > 8192 { return None; }
        // Include field lengths so distinct account/comic boundaries cannot alias.
        // Epoch remains an in-memory freshness fence, not a persistent filename.
        let source = format!("sleep-cover-v1:{}:{account}:{}:{}:{}", account.len(), comic.id.len(), comic.id, comic.cover_url);
        Some(Self { account: account.to_owned(), epoch, comic_id: comic.id.clone(),
            file_name: format!("{}.png", hex_digest(source.as_bytes())) })
    }
}

#[derive(Default)]
pub(super) struct SleepScreenState {
    pub(super) active: bool,
    identity: Option<CoverIdentity>,
    title: SharedString,
    author: SharedString,
    cover: Option<slint::Image>,
    full_resolution: bool,
    local_checked: bool,
    pending: bool,
    can_fetch: bool,
    retry_at: Option<Instant>,
    failures: u8,
    idle_not_before: Option<Instant>,
    generation: u64,
    preparation: Option<CoverPreparation>,
    candidate: Option<source::UntrustedCoverCandidate>,
}

#[cfg(test)]
impl SleepScreenState {
    pub(super) fn elapse_reader_idle_for_test(&mut self) { self.idle_not_before = Some(Instant::now()); }
}

struct PreparedCover {
    luma: Vec<u8>,
    pixels: slint::SharedPixelBuffer<slint::Rgb8Pixel>,
    width: u32,
    height: u32,
}

enum PreparedLocalCover {
    Full(PreparedCover),
    Thumbnail(PreparedCover),
}

#[derive(Clone)]
struct PreparationToken {
    generation: u64,
    cancelled: Arc<AtomicBool>,
    drained: Arc<AtomicBool>,
    wakeup: Arc<Notify>,
    trace: Option<crate::performance::Operation>,
}

impl PreparationToken {
    fn new(generation: u64) -> Self {
        Self { generation, cancelled: Arc::new(AtomicBool::new(false)),
            drained: Arc::new(AtomicBool::new(false)), wakeup: Arc::new(Notify::new()),
            trace: crate::performance::Operation::new("sleep_cover") }
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.wakeup.notify_waiters();
    }

    fn is_cancelled(&self) -> bool { self.cancelled.load(Ordering::Acquire) }

    fn check(&self) -> Result<()> {
        anyhow::ensure!(!self.is_cancelled(), "Sleep-cover preparation cancelled");
        Ok(())
    }

    async fn cancellation(&self) {
        loop {
            let notified = self.wakeup.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() { return; }
            notified.await;
        }
    }

    fn same_work(&self, other: &Self) -> bool { Arc::ptr_eq(&self.cancelled, &other.cancelled) }
}

struct CoverPreparation {
    key: &'static str,
    sequence: u64,
    token: PreparationToken,
    completion_received: bool,
}

fn bounded_text(text: &str, limit: usize) -> SharedString {
    text.chars().take(limit).collect::<String>().into()
}

fn sleep_comic_title(comic: &Comic) -> SharedString {
    let title = comic.title.trim();
    if title.is_empty() { tr("sleep_untitled_comic") } else { bounded_text(title, 256) }
}

#[cfg(test)]
fn decode_cover(encoded: Vec<u8>) -> Result<PreparedCover> {
    decode_cover_cancellable(encoded, &|| Ok(()), None)
}

fn decode_cover_cancellable(encoded: Vec<u8>, check: &dyn Fn() -> Result<()>,
    trace: Option<crate::performance::Operation>) -> Result<PreparedCover>
{
    check()?;
    anyhow::ensure!(encoded.len() <= MAX_ENCODED_BYTES, "Sleep cover exceeds its encoded byte limit");
    if let Some(trace) = trace { trace.record("decode_input", &serde_json::json!({"encoded_bytes": encoded.len()})); }
    let mut reader = image::ImageReader::new(Cursor::new(encoded)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let measured = trace.map(|_| crate::performance::Span::start());
    let mut image = reader.decode()?;
    if let (Some(trace), Some(measured)) = (trace, measured) { trace.record("decode", &measured.finish()); }
    check()?;
    if image.width() > MAX_COVER_WIDTH || image.height() > MAX_COVER_HEIGHT {
        let measured = trace.map(|_| crate::performance::Span::start());
        // Assignment drops the large decoded source before RGB construction.
        image = image.resize(MAX_COVER_WIDTH, MAX_COVER_HEIGHT, image::imageops::FilterType::Triangle);
        if let (Some(trace), Some(measured)) = (trace, measured) { trace.record("resize", &measured.finish()); }
    }
    check()?;
    let image = image.into_rgba8();
    let (width, height) = image.dimensions();
    anyhow::ensure!(width > 0 && height > 0 && width <= MAX_COVER_WIDTH && height <= MAX_COVER_HEIGHT,
        "Sleep cover exceeds its prepared pixel budget");
    let measured = trace.map(|_| crate::performance::Span::start());
    let mut luma = Vec::with_capacity((width * height) as usize);
    let mut pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(width, height);
    for (source, destination) in image.as_raw().chunks(4096 * 4)
        .zip(pixels.make_mut_slice().chunks_mut(4096))
    {
        check()?;
        for (pixel, destination) in source.chunks_exact(4).zip(destination) {
            let gray = (2126 * u32::from(pixel[0]) + 7152 * u32::from(pixel[1]) + 722 * u32::from(pixel[2]) + 5000) / 10000;
            let alpha = u32::from(pixel[3]);
            let opaque = (gray * alpha + 255 * (255 - alpha) + 127) / 255;
            let gray = (((opaque + 8) / 17).min(15) * 17) as u8;
            luma.push(gray);
            *destination = slint::Rgb8Pixel { r: gray, g: gray, b: gray };
        }
    }
    check()?;
    if let (Some(trace), Some(measured)) = (trace, measured) {
        trace.record("prepare_rgb", &serde_json::json!({"timing": measured.finish(),
            "width": width, "height": height, "luma_bytes": luma.len(), "rgb_bytes": pixels.as_bytes().len()}));
    }
    Ok(PreparedCover { luma, pixels, width, height })
}

fn cover_image(prepared: PreparedCover) -> slint::Image {
    slint::Image::from_rgb8(prepared.pixels)
}

impl App {
    pub fn sleep_cover_enabled(&self) -> bool {
        self.preferences.sleep_cover && !self.is_starting()
    }

    pub(super) fn clear_sleep_screen(&mut self) {
        let preparation = self.sleep_screen.preparation.take();
        if let Some(preparation) = &preparation { preparation.token.cancel(); }
        let generation = self.sleep_screen.generation.saturating_add(1);
        let next_idle = Instant::now() + READER_IDLE_DELAY;
        let idle_not_before = Some(self.sleep_screen.idle_not_before
            .map_or(next_idle, |at| at.max(next_idle)));
        for key in [LOCAL_JOB, SOURCE_JOB, PREPARE_JOB] { self.invalidate(key); }
        // An identity change drops its preview but cannot retire an actual
        // worker or its retained result before the drain guard is returned.
        self.sleep_screen = SleepScreenState { pending: preparation.is_some(), preparation,
            generation, idle_not_before, ..Default::default() };
        self.hide_sleep_screen();
    }

    /// Reader entry points call this for real activity, never from a UI tick.
    /// Keep a ready preview, the retry backoff, and the draining worker marker.
    pub(super) fn defer_sleep_cover_for_reader_activity(&mut self) {
        let now = Instant::now();
        self.sleep_screen.generation = self.sleep_screen.generation.saturating_add(1);
        self.defer_sleep_cover_idle_until(now + READER_IDLE_DELAY);
        if let Some(preparation) = &self.sleep_screen.preparation {
            preparation.token.cancel();
            if let Some(trace) = preparation.token.trace { trace.mark("reader_activity_cancel"); }
        }
        self.defer_ordinary_cover_work_for_reader_activity();
    }

    fn defer_sleep_cover_idle_until(&mut self, deadline: Instant) {
        self.sleep_screen.idle_not_before = Some(self.sleep_screen.idle_not_before
            .map_or(deadline, |at| at.max(deadline)));
    }

    fn reap_sleep_cover_preparation(&mut self) {
        let Some(preparation) = &self.sleep_screen.preparation else { return; };
        let invalidated = !self.jobs.is_current(self.epoch, preparation.sequence, preparation.key);
        if invalidated && !preparation.completion_received { preparation.token.cancel(); }
        if !preparation.token.drained.load(Ordering::Acquire)
            || source::work::speculative_in_flight_for(&preparation.token.cancelled)
            || !(preparation.completion_received || preparation.token.is_cancelled() || invalidated) { return; }
        let preparation = self.sleep_screen.preparation.take().expect("The observed preparation exists");
        if self.pending.get(preparation.key) == Some(&preparation.sequence) { self.invalidate(preparation.key); }
        self.sleep_screen.pending = false;
        if preparation.token.is_cancelled() {
            // A cancellation callback must not reset the deadline and restart
            // optional work in this same UI tick.
            self.defer_sleep_cover_idle_until(Instant::now() + READER_IDLE_DELAY);
        }
        if let Some(trace) = preparation.token.trace { trace.mark("drained"); }
        self.retry_reader_prefetch_after_optional_cover_drain();
    }

    fn track_sleep_cover_preparation(&mut self, key: &'static str, token: PreparationToken) {
        self.sleep_screen.pending = true;
        self.sleep_screen.preparation = Some(CoverPreparation {
            key, sequence: self.sequence, token, completion_received: false,
        });
    }

    fn complete_sleep_cover_preparation(&mut self, identity: &CoverIdentity,
        token: &PreparationToken) -> bool
    {
        let Some(preparation) = self.sleep_screen.preparation.as_mut() else { return false; };
        if !preparation.token.same_work(token) { return false; }
        preparation.completion_received = true;
        !token.is_cancelled() && token.generation == self.sleep_screen.generation
            && !self.is_suspended() && self.sleep_cover_is_current(identity)
    }

    /// Cover maintenance shares Reader idle admission even when the optional
    /// sleep presentation is disabled. Resets preserve this same deadline.
    pub(super) fn optional_cover_work_allowed(&self) -> bool {
        let now = Instant::now();
        !self.is_suspended() && !self.is_starting()
            && self.sleep_screen.idle_not_before.is_none_or(|at| now >= at)
            && !self.reader_blocks_optional_cover_work()
    }

    fn sleep_cover_can_start(&self) -> bool {
        self.optional_cover_work_allowed() && !self.sleep_screen.pending
            && self.sleep_screen.preparation.is_none()
            && self.sleep_screen.retry_at.is_none_or(|at| Instant::now() >= at)
    }

    fn sleep_cover_is_current(&self, identity: &CoverIdentity) -> bool {
        self.sleep_screen.identity.as_ref() == Some(identity)
            && self.current_sleep_comic().and_then(|comic| CoverIdentity::new(&self.account, self.epoch, &comic)).as_ref() == Some(identity)
    }

    /// Only ordinary awake work may read, acquire, or decode a candidate.
    pub(super) fn prepare_sleep_screen(&mut self) {
        self.reap_sleep_cover_preparation();
        if self.is_suspended() || self.is_starting()
            || (!self.platform.is_kindle && self.demo.is_none()) { return; }
        let comic = self.preferences.sleep_cover.then(|| self.current_sleep_comic()).flatten();
        let identity = comic.as_ref().and_then(|comic| CoverIdentity::new(&self.account, self.epoch, comic));
        if identity != self.sleep_screen.identity {
            self.clear_sleep_screen();
            self.sleep_screen.identity = identity.clone();
        }
        let (Some(comic), Some(identity)) = (comic, identity) else { return; };
        self.sleep_screen.title = sleep_comic_title(&comic);
        self.sleep_screen.author = bounded_text(&comic.author, 128);
        if self.sleep_screen.full_resolution { return; }
        let fixture_artwork = self.demo.as_ref().and_then(|_| crate::demo::sleep_artwork(&comic));
        if fixture_artwork.is_none() {
            if let Some(thumbnail) = self.cached_cover_preview(&comic) {
                self.sleep_screen.cover = Some(thumbnail);
            }
        }
        let can_fetch = self.demo.is_none() && !self.options.offline && !comic.cover_url.trim().is_empty();
        if can_fetch != self.sleep_screen.can_fetch {
            self.sleep_screen.can_fetch = can_fetch;
            self.sleep_screen.retry_at = None;
        }
        if !self.sleep_cover_can_start() { return; }
        if self.sleep_screen.candidate.is_some() {
            self.prepare_sleep_cover_candidate(identity);
            return;
        }
        if self.sleep_screen.local_checked {
            if can_fetch { self.request_sleep_cover(identity, comic.cover_url); }
            return;
        }
        let token = PreparationToken::new(self.sleep_screen.generation);
        let worker_token = token.clone();
        let callback_token = token.clone();
        let data_dir = self.data_dir.clone();
        let file_name = identity.file_name.clone();
        let thumbnail_name = (self.demo.is_none() && self.sleep_screen.cover.is_none()
            && !comic.cover_url.trim().is_empty())
            .then(|| format!("{}.png", hex_digest(comic.cover_url.as_bytes())));
        let admitted = self.background_optional_cover(LOCAL_JOB, token.trace,
            token.cancelled.clone(), token.drained.clone(), move || {
            worker_token.check()?;
            let measured = worker_token.trace.map(|_| crate::performance::Span::start());
            let encoded = match fixture_artwork {
                Some(bytes) => Some(bytes.to_vec()),
                None => covers::read_sleep_cover(&data_dir, &file_name)?,
            };
            if let (Some(trace), Some(measured)) = (worker_token.trace, measured) {
                trace.record("local_read", &serde_json::json!({"timing": measured.finish(),
                    "encoded_bytes": encoded.as_ref().map_or(0, Vec::len)}));
            }
            worker_token.check()?;
            if let Some(encoded) = encoded {
                return decode_cover_cancellable(encoded, &|| worker_token.check(), worker_token.trace)
                    .map(|prepared| Some(PreparedLocalCover::Full(prepared)));
            }
            // A cold offline resume may only have the ordinary URL-scoped
            // thumbnail. Read it here without restarting the regular cover
            // pipeline, and keep it distinct from a full sleep derivative.
            let Some(thumbnail_name) = thumbnail_name else { return Ok(None); };
            let measured = worker_token.trace.map(|_| crate::performance::Span::start());
            let encoded = covers::read_sleep_cover(&data_dir, &thumbnail_name)?;
            if let (Some(trace), Some(measured)) = (worker_token.trace, measured) {
                trace.record("thumbnail_read", &serde_json::json!({"timing": measured.finish(),
                    "encoded_bytes": encoded.as_ref().map_or(0, Vec::len)}));
            }
            worker_token.check()?;
            let Some(encoded) = encoded else { return Ok(None); };
            let measured = worker_token.trace.map(|_| crate::performance::Span::start());
            let checked = crate::image_integrity::validate_image_cancellable(&encoded, Some(480 * 640),
                &|| worker_token.is_cancelled())?.context("Sleep thumbnail validation cancelled")?;
            anyhow::ensure!(checked.width() <= 480 && checked.height() <= 640,
                "Cached sleep thumbnail exceeds its prepared pixel budget");
            if let (Some(trace), Some(measured)) = (worker_token.trace, measured) {
                trace.record("thumbnail_verify", &measured.finish());
            }
            worker_token.check()?;
            decode_cover_cancellable(encoded, &|| worker_token.check(), worker_token.trace)
                .map(|prepared| Some(PreparedLocalCover::Thumbnail(prepared)))
        }, move |app, result| {
            if !app.complete_sleep_cover_preparation(&identity, &callback_token) { return; }
            match result {
                Ok(Some(PreparedLocalCover::Full(prepared))) => app.install_sleep_cover(prepared),
                Ok(Some(PreparedLocalCover::Thumbnail(prepared))) => {
                    app.sleep_screen.local_checked = true;
                    if app.sleep_screen.cover.is_none() { app.sleep_screen.cover = Some(cover_image(prepared)); }
                }
                Ok(None) => app.sleep_screen.local_checked = true,
                Err(error) => {
                    let capacity = error.downcast_ref::<jobs::CapacityError>().is_some();
                    // A corrupt local derivative must not permanently suppress
                    // an awake refill from its current source.
                    if !capacity { app.sleep_screen.local_checked = true; }
                    app.defer_sleep_cover(capacity);
                }
            }
        });
        if admitted { self.track_sleep_cover_preparation(LOCAL_JOB, token); }
        else { self.defer_sleep_cover(true); }
    }

    fn request_sleep_cover(&mut self, identity: CoverIdentity, url: String) {
        if !self.sleep_cover_can_start() || !self.sleep_cover_is_current(&identity) || !self.sleep_screen.can_fetch { return; }
        let admission = match self.begin_job(SOURCE_JOB, false) {
            Ok(admission) => admission,
            Err(_) => { self.defer_sleep_cover(true); return; }
        };
        let token = PreparationToken::new(self.sleep_screen.generation);
        let worker_token = token.clone();
        let callback_token = token.clone();
        let drain = MarkJobDrainedOnDrop(token.drained.clone());
        let client = self.client.clone();
        self.track_sleep_cover_preparation(SOURCE_JOB, token);
        self.source_job_reserved(SOURCE_JOB, admission, async move {
            let _drain = drain;
            if let Some(trace) = worker_token.trace { trace.mark("fetch_begin"); }
            let measured = worker_token.trace.map(|_| crate::performance::Span::start());
            let result = tokio::select! {
                biased;
                _ = worker_token.cancellation() => Err(source::SourceError::new(ErrorKind::Cancelled, "sleep_cover_cancelled")),
                result = client.download_cover_candidate(&url) => result,
            };
            if let (Some(trace), Some(measured)) = (worker_token.trace, measured) {
                trace.record("fetch_end", &serde_json::json!({"timing": measured.finish(),
                    "ok": u8::from(result.is_ok()), "encoded_bytes": result.as_ref().map_or(0, |candidate| candidate.encoded_len())}));
            }
            result
        }, move |app, result| {
            if !app.complete_sleep_cover_preparation(&identity, &callback_token) { return; }
            match result {
                Ok(candidate) => app.sleep_screen.candidate = Some(candidate),
                Err(error) => {
                    if error.kind == ErrorKind::Cancelled {
                        app.defer_sleep_cover_idle_until(Instant::now() + READER_IDLE_DELAY);
                    } else { app.defer_sleep_cover(error.code == "application_job_capacity_reached"); }
                }
            }
        });
    }

    fn prepare_sleep_cover_candidate(&mut self, identity: CoverIdentity) {
        if !self.sleep_cover_can_start() || !self.sleep_cover_is_current(&identity) { return; }
        let Some(candidate) = self.sleep_screen.candidate.take() else { return; };
        // A failed nonqueued admission must not discard an acquired response
        // and make the next idle tick download the same source again.
        let candidate = Arc::new(std::sync::Mutex::new(Some(candidate)));
        let worker_candidate = candidate.clone();
        let token = PreparationToken::new(self.sleep_screen.generation);
        let worker_token = token.clone();
        let callback_token = token.clone();
        let data_dir = self.data_dir.clone();
        let file_name = identity.file_name.clone();
        let admitted = self.background_optional_cover(PREPARE_JOB, token.trace,
            token.cancelled.clone(), token.drained.clone(), move || {
            worker_token.check()?;
            let measured = worker_token.trace.map(|_| crate::performance::Span::start());
            let candidate = worker_candidate.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
                .take().context("The sleep-cover candidate was already consumed")?;
            let verified = candidate.verify_cancellable(&|| worker_token.is_cancelled())?;
            if let (Some(trace), Some(measured)) = (worker_token.trace, measured) { trace.record("verify", &measured.finish()); }
            worker_token.check()?;
            let prepared = decode_cover_cancellable(verified.bytes, &|| worker_token.check(), worker_token.trace)?;
            worker_token.check()?;
            let measured = worker_token.trace.map(|_| crate::performance::Span::start());
            covers::publish_sleep_cover(&data_dir, &file_name, &prepared.luma, prepared.width, prepared.height)?;
            if let (Some(trace), Some(measured)) = (worker_token.trace, measured) { trace.record("publish", &measured.finish()); }
            worker_token.check()?;
            Ok(prepared)
        }, move |app, result| {
            if !app.complete_sleep_cover_preparation(&identity, &callback_token) { return; }
            match result {
                Ok(prepared) => app.install_sleep_cover(prepared),
                Err(error) => app.defer_sleep_cover(error.downcast_ref::<jobs::CapacityError>().is_some()),
            }
        });
        if admitted { self.track_sleep_cover_preparation(PREPARE_JOB, token); }
        else {
            self.sleep_screen.candidate = candidate.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
            self.defer_sleep_cover(true);
        }
    }

    fn install_sleep_cover(&mut self, prepared: PreparedCover) {
        let trace = self.sleep_screen.preparation.as_ref().and_then(|work| work.token.trace);
        let measured = trace.map(|_| crate::performance::Span::start());
        self.sleep_screen.cover = Some(cover_image(prepared));
        if let (Some(trace), Some(measured)) = (trace, measured) { trace.record("install_memory", &measured.finish()); }
        self.sleep_screen.full_resolution = true;
        self.sleep_screen.pending = self.sleep_screen.preparation.is_some();
        self.sleep_screen.retry_at = None;
        self.sleep_screen.failures = 0;
    }

    fn defer_sleep_cover(&mut self, capacity: bool) {
        self.sleep_screen.pending = self.sleep_screen.preparation.is_some();
        let delay = if capacity { CAPACITY_RETRY } else {
            self.sleep_screen.failures = self.sleep_screen.failures.saturating_add(1);
            RETRY_DELAY.saturating_mul(1_u32 << self.sleep_screen.failures.saturating_sub(1).min(4))
                .min(Duration::from_secs(5 * 60))
        };
        self.sleep_screen.retry_at = Some(Instant::now() + delay);
    }

    /// Live values are absent from historical baselines that lack this schema.
    pub fn sleep_cover_resource_snapshot(&self) -> serde_json::Value {
        let now = Instant::now();
        let dimensions = self.sleep_screen.cover.as_ref().map(|cover| cover.size());
        let preparation = self.sleep_screen.preparation.as_ref();
        serde_json::json!({
            "ready": self.sleep_screen.cover.is_some(), "full": self.sleep_screen.full_resolution,
            "width": dimensions.as_ref().map(|size| size.width),
            "height": dimensions.as_ref().map(|size| size.height),
            "pending": self.sleep_screen.pending,
            "draining": preparation.is_some_and(|work| work.token.is_cancelled()
                && (!work.token.drained.load(Ordering::Acquire)
                    || source::work::speculative_in_flight_for(&work.token.cancelled))),
            "phase": preparation.map(|work| work.key),
            "idle_remaining_ms": self.sleep_screen.idle_not_before.map(|at| at.saturating_duration_since(now).as_millis() as u64),
            "retry_remaining_ms": self.sleep_screen.retry_at.map(|at| at.saturating_duration_since(now).as_millis() as u64),
            "generation": self.sleep_screen.generation,
            "candidate_bytes": self.sleep_screen.candidate.as_ref().map_or(0, |candidate| candidate.encoded_len()),
        })
    }

    /// Consume prepared memory only, even after the application intent gate closes.
    pub fn show_sleep_screen(&mut self) -> Result<()> {
        let ui = self.ui.upgrade().context("The sleep presentation window is unavailable")?;
        let comic = self.current_sleep_comic();
        let identity = comic.as_ref().and_then(|comic| CoverIdentity::new(&self.account, self.epoch, comic));
        let current = identity.is_some() && identity == self.sleep_screen.identity;
        let (title, author, cover) = match comic {
            Some(comic) if current => {
                let title = if self.sleep_screen.title.is_empty() {
                    sleep_comic_title(&comic)
                } else { self.sleep_screen.title.clone() };
                (title, self.sleep_screen.author.clone(), self.sleep_screen.cover.clone())
            }
            Some(comic) => (sleep_comic_title(&comic), bounded_text(&comic.author, 128), None),
            None => (tr("app_name"), tr("sleep_neutral"), None),
        };
        let model = ui.global::<SleepScreenModel>();
        model.set_has_cover(cover.is_some());
        model.set_cover(cover.unwrap_or_default());
        model.set_title(title);
        model.set_author(author);
        model.set_active(true);
        self.sleep_screen.active = true;
        self.sync_presented_reader_dithering();
        ui.window().request_redraw();
        Ok(())
    }

    /// Hide the temporary overlay before the native backend redraws the retained scene.
    pub fn hide_sleep_screen(&mut self) {
        self.sleep_screen.active = false;
        if let Some(ui) = self.ui.upgrade() {
            let model = ui.global::<SleepScreenModel>();
            model.set_active(false);
            model.set_has_cover(false);
            model.set_cover(slint::Image::default());
            model.set_title("".into());
            model.set_author("".into());
        }
        self.sync_presented_reader_dithering();
    }
}

#[cfg(test)]
mod tests;

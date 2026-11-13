//! Prepare one account-scoped sleep cover outside the power-event critical path.

use super::*;
use crate::ui::SleepScreenModel;
use std::io::Cursor;

const MAX_COVER_WIDTH: u32 = 1536;
const MAX_COVER_HEIGHT: u32 = 2048;
const MAX_ENCODED_BYTES: usize = 8 * 1024 * 1024;
const RETRY_DELAY: Duration = Duration::from_secs(30);
const CAPACITY_RETRY: Duration = Duration::from_secs(1);
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
}

struct PreparedCover {
    luma: Vec<u8>,
    width: u32,
    height: u32,
}

fn bounded_text(text: &str, limit: usize) -> SharedString {
    text.chars().take(limit).collect::<String>().into()
}

fn decode_cover(encoded: Vec<u8>) -> Result<PreparedCover> {
    anyhow::ensure!(encoded.len() <= MAX_ENCODED_BYTES, "Sleep cover exceeds its encoded byte limit");
    let mut reader = image::ImageReader::new(Cursor::new(encoded)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode()?;
    let image = if image.width() > MAX_COVER_WIDTH || image.height() > MAX_COVER_HEIGHT {
        image.resize(MAX_COVER_WIDTH, MAX_COVER_HEIGHT, image::imageops::FilterType::Triangle)
    } else { image };
    let image = image.into_rgba8();
    let (width, height) = image.dimensions();
    anyhow::ensure!(width > 0 && height > 0 && width <= MAX_COVER_WIDTH && height <= MAX_COVER_HEIGHT,
        "Sleep cover exceeds its prepared pixel budget");
    let luma = image.pixels().map(|pixel| {
        let gray = (2126 * u32::from(pixel[0]) + 7152 * u32::from(pixel[1]) + 722 * u32::from(pixel[2]) + 5000) / 10000;
        let alpha = u32::from(pixel[3]);
        let opaque = (gray * alpha + 255 * (255 - alpha) + 127) / 255;
        (((opaque + 8) / 17).min(15) * 17) as u8
    }).collect();
    Ok(PreparedCover { luma, width, height })
}

fn cover_image(prepared: PreparedCover) -> slint::Image {
    let mut pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(prepared.width, prepared.height);
    for (destination, gray) in pixels.make_mut_slice().iter_mut().zip(prepared.luma) {
        *destination = slint::Rgb8Pixel { r: gray, g: gray, b: gray };
    }
    slint::Image::from_rgb8(pixels)
}

impl App {
    pub fn sleep_cover_enabled(&self) -> bool {
        self.preferences.sleep_cover && !self.is_starting()
    }

    pub(super) fn clear_sleep_screen(&mut self) {
        for key in [LOCAL_JOB, SOURCE_JOB, PREPARE_JOB] { self.invalidate(key); }
        self.sleep_screen = SleepScreenState::default();
        self.hide_sleep_screen();
    }

    fn sleep_cover_is_current(&self, identity: &CoverIdentity) -> bool {
        self.sleep_screen.identity.as_ref() == Some(identity)
            && self.current_sleep_comic().and_then(|comic| CoverIdentity::new(&self.account, self.epoch, &comic)).as_ref() == Some(identity)
    }

    /// Only ordinary awake work may read, acquire, or decode a candidate.
    pub(super) fn prepare_sleep_screen(&mut self) {
        if self.is_suspended() || self.is_starting()
            || (!self.platform.is_kindle && self.demo.is_none()) { return; }
        let comic = self.preferences.sleep_cover.then(|| self.current_sleep_comic()).flatten();
        let identity = comic.as_ref().and_then(|comic| CoverIdentity::new(&self.account, self.epoch, comic));
        if identity != self.sleep_screen.identity {
            self.clear_sleep_screen();
            self.sleep_screen.identity = identity.clone();
        }
        let (Some(comic), Some(identity)) = (comic, identity) else { return; };
        self.sleep_screen.title = bounded_text(&comic.title, 256);
        self.sleep_screen.author = bounded_text(&comic.author, 128);
        if self.sleep_screen.full_resolution { return; }
        let thumbnail = self.cover_image(&comic);
        if thumbnail.size().width > 0 && thumbnail.size().height > 0 {
            self.sleep_screen.cover = Some(thumbnail);
        }
        let can_fetch = self.demo.is_none() && !self.options.offline && !comic.cover_url.trim().is_empty();
        if can_fetch != self.sleep_screen.can_fetch {
            self.sleep_screen.can_fetch = can_fetch;
            self.sleep_screen.retry_at = None;
        }
        if self.sleep_screen.pending || self.sleep_screen.retry_at.is_some_and(|at| Instant::now() < at) { return; }
        if self.sleep_screen.local_checked {
            if can_fetch { self.request_sleep_cover(identity, comic.cover_url); }
            return;
        }
        self.sleep_screen.pending = true;
        let data_dir = self.data_dir.clone();
        let file_name = identity.file_name.clone();
        self.cover_background(LOCAL_JOB, move || {
            covers::read_sleep_cover(&data_dir, &file_name)?.map(decode_cover).transpose()
        }, move |app, result| {
            if !app.sleep_cover_is_current(&identity) { return; }
            app.sleep_screen.pending = false;
            match result {
                Ok(Some(prepared)) => app.install_sleep_cover(prepared),
                Ok(None) => {
                    app.sleep_screen.local_checked = true;
                    if app.sleep_screen.can_fetch { app.request_sleep_cover(identity, comic.cover_url); }
                }
                Err(error) => {
                    let capacity = error.downcast_ref::<jobs::CapacityError>().is_some();
                    // A corrupt local derivative must not permanently suppress
                    // an awake refill from its current source.
                    if !capacity { app.sleep_screen.local_checked = true; }
                    app.defer_sleep_cover(capacity);
                }
            }
        });
    }

    fn request_sleep_cover(&mut self, identity: CoverIdentity, url: String) {
        if self.is_suspended() || !self.sleep_cover_is_current(&identity) || !self.sleep_screen.can_fetch { return; }
        self.sleep_screen.pending = true;
        let client = self.client.clone();
        self.source_job(SOURCE_JOB, async move { client.download_cover(&url).await }, move |app, result| {
            if !app.sleep_cover_is_current(&identity) { return; }
            match result {
                Ok(image) => {
                    let data_dir = app.data_dir.clone();
                    let file_name = identity.file_name.clone();
                    app.cover_background(PREPARE_JOB, move || {
                        let prepared = decode_cover(image.bytes)?;
                        covers::publish_sleep_cover(&data_dir, &file_name, &prepared.luma, prepared.width, prepared.height)?;
                        Ok(prepared)
                    }, move |app, result| {
                        if !app.sleep_cover_is_current(&identity) { return; }
                        app.sleep_screen.pending = false;
                        match result {
                            Ok(prepared) => { app.install_sleep_cover(prepared); app.schedule_cover_maintenance(false); }
                            Err(error) => app.defer_sleep_cover(error.downcast_ref::<jobs::CapacityError>().is_some()),
                        }
                    });
                }
                Err(error) => {
                    app.sleep_screen.pending = false;
                    app.defer_sleep_cover(error.code == "application_job_capacity_reached");
                }
            }
        });
    }

    fn install_sleep_cover(&mut self, prepared: PreparedCover) {
        self.sleep_screen.cover = Some(cover_image(prepared));
        self.sleep_screen.full_resolution = true;
        self.sleep_screen.pending = false;
        self.sleep_screen.retry_at = None;
        self.sleep_screen.failures = 0;
    }

    fn defer_sleep_cover(&mut self, capacity: bool) {
        self.sleep_screen.pending = false;
        let delay = if capacity { CAPACITY_RETRY } else {
            self.sleep_screen.failures = self.sleep_screen.failures.saturating_add(1);
            RETRY_DELAY.saturating_mul(1_u32 << self.sleep_screen.failures.saturating_sub(1).min(4))
                .min(Duration::from_secs(5 * 60))
        };
        self.sleep_screen.retry_at = Some(Instant::now() + delay);
    }

    /// Consume prepared memory only, even after the application intent gate closes.
    pub fn show_sleep_screen(&mut self) -> Result<()> {
        let ui = self.ui.upgrade().context("The sleep presentation window is unavailable")?;
        let comic = self.current_sleep_comic();
        let identity = comic.as_ref().and_then(|comic| CoverIdentity::new(&self.account, self.epoch, comic));
        let current = identity.is_some() && identity == self.sleep_screen.identity;
        let (title, author, cover) = if current {
            (self.sleep_screen.title.clone(), self.sleep_screen.author.clone(), self.sleep_screen.cover.clone())
        } else if let Some(comic) = comic {
            (bounded_text(&comic.title, 256), bounded_text(&comic.author, 128), None)
        } else { (tr("app_name"), tr("sleep_neutral"), None) };
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

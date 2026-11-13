//! Local-only comic positioning, bounded raster preparation, and viewport layout.
//!
//! White-gutter detection is a heuristic. A page without reliable gutters uses
//! deterministic viewport slices; it is never reported as a verified panel map.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::Path,
    sync::{Arc, atomic::{AtomicBool, Ordering}},
};

use image::{GenericImageView, GrayImage};
#[cfg(test)]
use std::io::Cursor;
#[cfg(test)]
use image::{ImageFormat, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::{domain::{ImageIndex, ReadingAnchor, ReadingMode}, image_integrity::{HashedImage, IntegrityError, ValidatedImage}};
use crate::performance::Span;
pub use crate::performance::{StageMap, StageTotals};

mod resources;
pub(crate) use resources::ReaderResources;

#[cfg(test)]
mod reuse_tests;
#[cfg(test)]
mod profile_tests;
#[cfg(test)]
mod preparation_profile_tests;
#[cfg(test)]
mod cancellation_tests;
#[cfg(test)]
mod hardware_dither_tests;
#[cfg(test)]
mod fit_tests;
mod triangle;

pub const MAX_ENCODED_BYTES: u64 = crate::image_integrity::MAX_ENCODED_BYTES;
pub const MAX_SOURCE_PIXELS: u64 = crate::image_integrity::MAX_SOURCE_PIXELS;
pub const MAX_DECODE_ALLOCATION: u64 = crate::image_integrity::MAX_DECODE_ALLOCATION;
pub const MAX_VIEWPORT_PIXELS: u64 = 8_000_000;
const MAX_LAYOUT_REGIONS: usize = 512;
const MAX_REMEMBERED_LAYOUTS: usize = 8;
const MAX_EXPLICIT_PANEL_PAGES: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum ReaderError {
    #[error("the episode descriptor is empty or invalid")]
    InvalidDescriptor,
    #[error("the reading position does not belong to this episode revision")]
    StaleAnchor,
    #[error("the reading position is invalid")]
    InvalidAnchor,
    #[error("the requested page is outside the episode")]
    PageOutOfRange,
    #[error("the image file exceeds the encoded byte limit")]
    EncodedImageTooLarge,
    #[error("the source image exceeds the bounded decode budget")]
    SourceImageTooLarge,
    #[error("the viewport is invalid or exceeds the output budget")]
    InvalidViewport,
    #[error("the image format is unsupported")]
    UnsupportedFormat,
    #[error("the encoded image is incomplete or corrupt")]
    CorruptImage,
    #[error("the image needs more viewport regions than the bounded layout allows")]
    LayoutTooComplex,
    #[error("the reading settings contain an invalid value")]
    InvalidSettings,
    #[error("image preparation failed: {0}")]
    Image(#[from] image::ImageError),
    #[error("local image access failed: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, ReaderError>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMode {
    #[default]
    Page,
    Panel,
    Strip,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    /// Keep small sources at native size and shrink larger ones to fit.
    Original,
    /// Stretch the selected source view across both viewport axes.
    #[default]
    Page,
    /// Use one height-derived scale and center any horizontal overflow.
    Height,
    /// Use one width-derived scale and center any vertical overflow.
    Width,
}

impl FitMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Page => "page",
            Self::Height => "height",
            Self::Width => "width",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "original" => Some(Self::Original),
            "page" => Some(Self::Page),
            "height" => Some(Self::Height),
            "width" => Some(Self::Width),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageDirection {
    #[default]
    LeftToRight,
    RightToLeft,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReaderSettings {
    pub mode: DisplayMode,
    pub fit: FitMode,
    pub direction: PageDirection,
    pub landscape_spread: bool,
    pub cover_alone: bool,
    pub dither: bool,
    /// Keep Gray8 pixels intact for the display backend's hardware Y4 path.
    pub hardware_dither: bool,
    pub footer: bool,
    pub rotation: u16,
    pub zoom_ratio: f64,
}

impl Default for ReaderSettings {
    fn default() -> Self {
        Self {
            mode: DisplayMode::Page,
            fit: FitMode::Page,
            direction: PageDirection::LeftToRight,
            landscape_spread: true,
            cover_alone: true,
            dither: true,
            hardware_dither: false,
            footer: true,
            rotation: 0,
            zoom_ratio: 1.0,
        }
    }
}

impl ReaderSettings {
    pub fn uses_hardware_dither(&self) -> bool {
        self.dither && self.hardware_dither
    }

    pub fn validate(&self) -> Result<()> {
        if !matches!(self.rotation, 0 | 90 | 180 | 270)
            || !self.zoom_ratio.is_finite()
            || !(1.0..=8.0).contains(&self.zoom_ratio)
        {
            return Err(ReaderError::InvalidSettings);
        }
        Ok(())
    }
}

/// Width and height are output pixels, not the device's logical design units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub scale_factor: f32,
}

impl Viewport {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            scale_factor: 1.0,
        }
    }

    fn validate(self) -> Result<()> {
        if self.width == 0
            || self.height == 0
            || u64::from(self.width) * u64::from(self.height) > MAX_VIEWPORT_PIXELS
            || !self.scale_factor.is_finite()
            || !(0.5..=4.0).contains(&self.scale_factor)
        {
            return Err(ReaderError::InvalidViewport);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A separately supplied panel map is distinct from heuristic gutter detection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NormalizedRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl NormalizedRect {
    fn validate(self) -> Result<()> {
        if ![self.x, self.y, self.width, self.height].iter().all(|value| value.is_finite())
            || self.x < 0.0 || self.y < 0.0 || self.width <= 0.0 || self.height <= 0.0
            || self.x + self.width > 1.0 || self.y + self.height > 1.0
        {
            return Err(ReaderError::InvalidDescriptor);
        }
        Ok(())
    }

    fn pixel_rect(self, width: u32, height: u32, rotation: u16) -> SourceRect {
        let rect = match rotation {
            90 => Self { x: 1.0 - self.y - self.height, y: self.x, width: self.height, height: self.width },
            180 => Self { x: 1.0 - self.x - self.width, y: 1.0 - self.y - self.height, ..self },
            270 => Self { x: self.y, y: 1.0 - self.x - self.width, width: self.height, height: self.width },
            _ => self,
        };
        let x = (rect.x * f64::from(width)).round().clamp(0.0, f64::from(width - 1)) as u32;
        let y = (rect.y * f64::from(height)).round().clamp(0.0, f64::from(height - 1)) as u32;
        let right = ((rect.x + rect.width) * f64::from(width)).round().clamp(f64::from(x + 1), f64::from(width)) as u32;
        let bottom = ((rect.y + rect.height) * f64::from(height)).round().clamp(f64::from(y + 1), f64::from(height)) as u32;
        SourceRect { x, y, width: right - x, height: bottom - y }
    }
}

impl SourceRect {
    fn whole(width: u32, height: u32) -> Self {
        Self { x: 0, y: 0, width, height }
    }

    fn contains(self, x: u32, y: u32) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(self.width)
            && y < self.y.saturating_add(self.height)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReaderMove {
    Moved,
    AtStart,
    EndOfEpisode,
}

pub struct RenderedPage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub crop: SourceRect,
    pub page_index: usize,
    pub crop_index: usize,
    pub crop_count: usize,
    pub overlap_from_previous: bool,
    pub heuristic_panels: bool,
    pub dark_fraction: f32,
}

/// Optional stage totals for worker-side image preparation, in microseconds.
///
/// The counters may cover several source loads and frames (for example a spread
/// and its overview). They contain no paths, content identities, or account data.
/// `source_read_us` is populated by the path API only; a resolver that supplies
/// an already hashed immutable input must measure its own read and hash stage.
/// Integrity checks and decoding share a stage because the integrity layer owns
/// their ordering. A reused source still performs that layer's required checks.
/// Durations describe completed stages; an early error or cancellation may leave
/// its active span unrecorded. Canvas time includes releasing rotated pixels.
///
/// `stages` adds explicit wall and same-thread CPU totals. Missing stages were
/// not attempted; unavailable CPU clocks remain None. Its leaf stages do not
/// overlap each other. `aggregate_*` entries are parent intervals and must not
/// be added to their children. Path-source hashing is inside
/// `aggregate_integrity`, while a receipt resolver hashes before that interval;
/// `aggregate_receipt_prepare` includes integrity, grayscale and proof checks.
/// Validation spans may occur several times for one
/// image. JPEG decode includes its inseparable strict entropy validation.
/// Failed or cancelled spans are retained with a partial count; existing
/// scalar durations and source counters retain their previous success rules.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ReaderPreparationProfile {
    pub source_read_us: u64,
    pub integrity_decode_us: u64,
    pub grayscale_us: u64,
    pub rotation_us: u64,
    pub layout_us: u64,
    pub resize_us: u64,
    pub canvas_us: u64,
    pub decoded_sources: u32,
    pub reused_sources: u32,
    pub unchanged_sources: u32,
    pub layout_rebuilds: u32,
    pub rendered_frames: u32,
    pub resized_pixels: u64,
    pub output_bytes: u64,
    pub stages: StageMap,
}

impl ReaderPreparationProfile {
    /// Include this description beside a serialized operation summary.
    pub const STAGE_SCOPE: &'static str = concat!(
        "Leaf stages are non-overlapping measured spans; gaps are not attributed. ",
        "aggregate_* entries are parent intervals and must not be added to their children. ",
        "Path source_hash is inside aggregate_integrity; receipt source_hash precedes it. ",
        "aggregate_receipt_prepare includes integrity, grayscale and proof checks. ",
        "integrity_validation may contain several separate spans per source. ",
        "JPEG decode includes inseparable strict entropy validation. ",
        "Missing stages were not attempted; partial counts retain unsuccessful work. ",
        "cpu_us is same-thread CPU time when available, otherwise null. ",
        "Legacy scalar microseconds preserve their original completion boundaries."
    );
}

fn profile_start(profile: &Option<&mut ReaderPreparationProfile>) -> Option<Span> {
    profile.as_ref().map(|_| Span::start())
}

fn profile_elapsed(profile: &mut Option<&mut ReaderPreparationProfile>, started: Option<Span>,
    stage: &'static str, completed: bool,
    legacy: Option<fn(&mut ReaderPreparationProfile) -> &mut u64>)
{
    if let (Some(profile), Some(started)) = (profile.as_deref_mut(), started) {
        let sample = started.finish();
        if let Some(field) = legacy {
            let value = field(profile);
            *value = value.saturating_add(sample.wall_us);
        }
        profile.stages.entry(stage).or_default().record(sample, completed);
    }
}

fn profile_source(profile: &mut Option<&mut ReaderPreparationProfile>,
    decoded: &crate::image_integrity::ReusableImage<Option<Arc<GrayImage>>>)
{
    if let Some(profile) = profile.as_deref_mut() {
        let count = match decoded {
            crate::image_integrity::ReusableImage::Decoded(_) => &mut profile.decoded_sources,
            crate::image_integrity::ReusableImage::Reused(Some(_)) => &mut profile.reused_sources,
            crate::image_integrity::ReusableImage::Reused(None) => &mut profile.unchanged_sources,
        };
        *count = count.saturating_add(1);
    }
}

/// A render owns no cancellation task or resource lease. The actual worker
/// retains those until this cooperative pixel work has returned.
struct RenderCancellation<'a> {
    flag: Option<&'a AtomicBool>,
    #[cfg(test)]
    probe: Option<&'a dyn Fn(RenderWork, usize)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RenderWork {
    Boundary,
    ResizePlan,
    ResizeWeights,
    ResizeVertical,
    ResizeHorizontal,
    ResizeCopy,
    CanvasFill,
    CanvasPixels,
}

impl<'a> RenderCancellation<'a> {
    fn never() -> Self {
        Self { flag: None, #[cfg(test)] probe: None }
    }

    fn for_flag(flag: &'a AtomicBool) -> Self {
        Self { flag: Some(flag), #[cfg(test)] probe: None }
    }

    #[cfg(test)]
    fn with_probe(flag: &'a AtomicBool, probe: &'a dyn Fn(RenderWork, usize)) -> Self {
        Self { flag: Some(flag), probe: Some(probe) }
    }

    fn is_enabled(&self) -> bool { self.flag.is_some() }

    fn is_cancelled(&self, _work: RenderWork, _completed: usize) -> bool {
        #[cfg(test)]
        if let Some(probe) = self.probe { probe(_work, _completed); }
        self.flag.is_some_and(|flag| flag.load(Ordering::Acquire))
    }
}

#[derive(Clone)]
pub(crate) struct ValidatedGraySource {
    proof: ValidatedImage,
    pixels: Arc<GrayImage>,
}

impl ValidatedGraySource {
    pub(crate) fn proof(&self) -> &ValidatedImage { &self.proof }
}

#[derive(Clone, Copy)]
struct Region {
    rect: SourceRect,
    overlap: bool,
}

#[derive(Clone, Copy)]
struct CenteredSample {
    bounds: SourceRect,
    center_x: f64,
    center_y: f64,
    scale: f64,
}

#[derive(Clone, Copy)]
struct FitGeometry {
    crop: SourceRect,
    width: u32,
    height: u32,
    sample: Option<CenteredSample>,
    origin_x: f64,
    origin_y: f64,
}

#[derive(Clone)]
struct PageLayout {
    viewport: Viewport,
    width: u32,
    height: u32,
    regions: Vec<Region>,
    heuristic: bool,
    source_identity: Option<String>,
}

#[derive(Clone)]
pub struct ReaderCore {
    index: Arc<ImageIndex>,
    settings: ReaderSettings,
    page_index: usize,
    crop_index: usize,
    source_x: f64,
    source_y: f64,
    restore_focus: bool,
    preserve_region_for_pan: bool,
    explicit_pan: bool,
    restore_last_region: bool,
    transition: bool,
    landscape_override: Option<bool>,
    last_viewport: Option<Viewport>,
    layouts: BTreeMap<usize, PageLayout>,
    explicit_panels: BTreeMap<usize, Vec<NormalizedRect>>,
    #[cfg(test)]
    raster_path: TestRasterPath,
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum TestRasterPath { Current, Reference, SingleChannel }

impl ReaderCore {
    pub fn new(index: ImageIndex, settings: ReaderSettings) -> Result<Self> {
        settings.validate()?;
        let mut ids = BTreeSet::new();
        if index.episode_id.is_empty()
            || index.revision.is_empty()
            || index.images.is_empty()
            || index.images.iter().enumerate().any(|(position, page)| {
                page.id.is_empty() || page.index != position || !ids.insert(page.id.clone())
            })
        {
            return Err(ReaderError::InvalidDescriptor);
        }
        Ok(Self {
            index: Arc::new(index),
            settings,
            page_index: 0,
            crop_index: 0,
            source_x: 0.0,
            source_y: 0.0,
            restore_focus: false,
            preserve_region_for_pan: false,
            explicit_pan: false,
            restore_last_region: false,
            transition: false,
            landscape_override: None,
            last_viewport: None,
            layouts: BTreeMap::new(),
            explicit_panels: BTreeMap::new(),
            #[cfg(test)]
            raster_path: TestRasterPath::Current,
        })
    }

    pub fn index(&self) -> &ImageIndex { &self.index }
    pub fn settings(&self) -> &ReaderSettings { &self.settings }
    pub fn page_index(&self) -> usize { self.page_index }
    pub fn page_count(&self) -> usize { self.index.images.len() }
    pub fn crop_index(&self) -> usize { self.crop_index }
    pub fn is_transition(&self) -> bool { self.transition }
    pub fn is_spread(&self) -> bool { self.spread_active() }
    pub fn crop_count(&self) -> usize {
        self.layouts.get(&self.page_index).map_or(1, |layout| layout.regions.len())
    }

    /// Planned retained metadata for a speculative clone, excluding its shared
    /// index. Vec/String capacities are counted; tree nodes and allocator
    /// overhead use a conservative per-entry allowance. This is not an RSS cap.
    pub(crate) fn retained_preparation_metadata_bytes(&self) -> u64 {
        const TREE_ENTRY_ALLOWANCE: u64 = 4096;
        let layouts = self.layouts.values().fold(0u64, |bytes, layout| {
            bytes.saturating_add(TREE_ENTRY_ALLOWANCE)
                .saturating_add(std::mem::size_of::<PageLayout>() as u64)
                .saturating_add((layout.regions.capacity() as u64)
                    .saturating_mul(std::mem::size_of::<Region>() as u64))
                .saturating_add(layout.source_identity.as_ref().map_or(0, |identity| identity.capacity() as u64))
        });
        let panels = self.explicit_panels.values().fold(0u64, |bytes, panels| {
            bytes.saturating_add(TREE_ENTRY_ALLOWANCE)
                .saturating_add(std::mem::size_of::<Vec<NormalizedRect>>() as u64)
                .saturating_add((panels.capacity() as u64)
                    .saturating_mul(std::mem::size_of::<NormalizedRect>() as u64))
        });
        (std::mem::size_of::<Self>() as u64).saturating_add(layouts).saturating_add(panels)
    }

    pub fn update_settings(&mut self, settings: ReaderSettings) -> Result<()> {
        settings.validate()?;
        // The serialized footer preference does not participate in source layout.
        self.settings.footer = settings.footer;
        if self.settings != settings {
            let reflow = self.settings.mode != settings.mode || self.settings.fit != settings.fit
                || self.settings.direction != settings.direction || self.settings.rotation != settings.rotation;
            if settings.zoom_ratio == 1.0 || self.settings.fit != settings.fit {
                self.explicit_pan = false;
            }
            self.settings = settings;
            if reflow {
                self.layouts.clear();
                self.crop_index = 0;
            }
            self.restore_focus = true;
            self.preserve_region_for_pan = !reflow;
            self.restore_last_region = false;
        }
        Ok(())
    }

    pub fn set_page(&mut self, page_index: usize) -> Result<()> {
        if page_index >= self.page_count() {
            return Err(ReaderError::PageOutOfRange);
        }
        self.page_index = page_index;
        self.crop_index = 0;
        self.source_x = 0.0;
        self.source_y = 0.0;
        self.restore_focus = false;
        self.preserve_region_for_pan = false;
        self.explicit_pan = false;
        self.restore_last_region = false;
        self.transition = false;
        Ok(())
    }

    pub fn go_to(&mut self, page_index: usize) -> Result<()> { self.set_page(page_index) }

    /// Return to the first source page without changing presentation preferences.
    pub fn to_start(&mut self) -> Result<()> { self.set_page(0) }

    /// Leave the episode transition at its final source page.
    /// Strip reading returns to the last viewport, even before that page has a layout.
    /// Page and panel reading restart the final page at its first region.
    pub fn back_to_last_view(&mut self) -> Result<()> {
        self.set_page(self.page_count() - 1)?;
        if self.settings.mode == DisplayMode::Strip {
            self.crop_index = self.crop_count().saturating_sub(1);
            self.source_y = 1.0;
            self.restore_last_region = true;
        }
        Ok(())
    }

    /// Set this when spread images are rendered into separate portrait leaf viewports.
    pub fn set_landscape(&mut self, landscape: bool) {
        self.landscape_override = Some(landscape);
    }

    /// Map order is the supplied reading order, not an automatic recognition claim.
    pub fn set_panel_map(&mut self, page_id: &str, panels: Vec<NormalizedRect>) -> Result<()> {
        let page_index = self.index.images.iter().position(|page| page.id == page_id)
            .ok_or(ReaderError::PageOutOfRange)?;
        if panels.is_empty() || panels.len() > MAX_LAYOUT_REGIONS
            || (!self.explicit_panels.contains_key(&page_index) && self.explicit_panels.len() >= MAX_EXPLICIT_PANEL_PAGES)
        {
            return Err(ReaderError::LayoutTooComplex);
        }
        for panel in &panels { panel.validate()?; }
        self.explicit_panels.insert(page_index, panels);
        self.layouts.remove(&page_index);
        if page_index == self.page_index {
            self.restore_focus = true;
            self.preserve_region_for_pan = false;
        }
        Ok(())
    }

    /// Pan coordinates refer to the unrotated original source image.
    /// At unit zoom they locate an existing panel or strip region without
    /// replacing the preset's centered placement with a top-left offset.
    pub fn pan_to(&mut self, source_x: f64, source_y: f64) -> Result<()> {
        validate_coordinates(source_x, source_y)?;
        self.source_x = source_x;
        self.source_y = source_y;
        self.restore_focus = true;
        self.preserve_region_for_pan = true;
        self.explicit_pan = self.settings.zoom_ratio > 1.0;
        self.restore_last_region = false;
        Ok(())
    }

    pub fn restore(&mut self, anchor: &ReadingAnchor) -> Result<()> {
        if anchor.episode_id != self.index.episode_id || anchor.revision != self.index.revision {
            return Err(ReaderError::StaleAnchor);
        }
        validate_coordinates(anchor.source_x, anchor.source_y)?;
        if !anchor.zoom_ratio.is_finite()
            || !(1.0..=8.0).contains(&anchor.zoom_ratio)
            || !matches!(anchor.rotation, 0 | 90 | 180 | 270)
        {
            return Err(ReaderError::InvalidAnchor);
        }
        let page_index = self.index.images.iter().position(|page| page.id == anchor.page_id)
            .ok_or(ReaderError::StaleAnchor)?;
        if page_index != anchor.page_index {
            return Err(ReaderError::StaleAnchor);
        }
        self.page_index = page_index;
        self.crop_index = 0;
        self.source_x = anchor.source_x;
        self.source_y = anchor.source_y;
        self.settings.zoom_ratio = anchor.zoom_ratio;
        self.settings.rotation = anchor.rotation;
        self.settings.mode = match anchor.mode {
            ReadingMode::Panel => DisplayMode::Panel,
            ReadingMode::Strip | ReadingMode::Continuous => DisplayMode::Strip,
            ReadingMode::Page => DisplayMode::Page,
        };
        self.restore_focus = true;
        self.preserve_region_for_pan = false;
        self.explicit_pan = anchor.zoom_ratio > 1.0;
        self.restore_last_region = false;
        self.transition = anchor.finished;
        self.layouts.clear();
        Ok(())
    }

    pub fn anchor(&self, timestamp: i64) -> ReadingAnchor {
        ReadingAnchor {
            episode_id: self.index.episode_id.clone(),
            revision: self.index.revision.clone(),
            page_id: self.index.images[self.page_index].id.clone(),
            page_index: self.page_index,
            source_x: self.source_x,
            source_y: self.source_y,
            zoom_ratio: self.settings.zoom_ratio,
            rotation: self.settings.rotation,
            mode: match self.settings.mode {
                DisplayMode::Page => ReadingMode::Page,
                DisplayMode::Panel => ReadingMode::Panel,
                DisplayMode::Strip => ReadingMode::Strip,
            },
            finished: self.transition,
            updated_at: timestamp,
        }
    }

    /// The returned pair is in physical left-to-right screen order.
    pub fn spread_indices(&self) -> [Option<usize>; 2] {
        if !self.spread_active() { return [Some(self.page_index), None]; }
        let first = self.spread_base();
        let mut pair = if first == 0 && self.settings.cover_alone {
            [None, Some(first)]
        } else {
            [Some(first), (first + 1 < self.page_count()).then_some(first + 1)]
        };
        if self.settings.direction == PageDirection::RightToLeft {
            pair.reverse();
        }
        pair
    }

    /// Forward always follows source order; the presentation maps touch zones.
    pub fn step(&mut self, forward: bool) -> ReaderMove {
        if self.transition {
            if forward { return ReaderMove::EndOfEpisode; }
            let _ = self.back_to_last_view();
            return ReaderMove::Moved;
        }
        if forward {
            if !self.spread_active() && self.crop_index + 1 < self.crop_count() {
                self.crop_index += 1;
                self.focus_current_region();
                return ReaderMove::Moved;
            }
            let next = if self.spread_active() {
                let base = self.spread_base();
                base + if base == 0 && self.settings.cover_alone { 1 } else { 2 }
            } else { self.page_index + 1 };
            if next >= self.page_count() {
                self.transition = true;
                return ReaderMove::EndOfEpisode;
            }
            let _ = self.set_page(next);
            ReaderMove::Moved
        } else {
            if !self.spread_active() && self.crop_index > 0 {
                self.crop_index -= 1;
                self.focus_current_region();
                return ReaderMove::Moved;
            }
            let previous = if self.spread_active() {
                let base = self.spread_base();
                if base == 0 { return ReaderMove::AtStart; }
                if base == 1 && self.settings.cover_alone { 0 } else { base.saturating_sub(2) }
            } else {
                if self.page_index == 0 { return ReaderMove::AtStart; }
                self.page_index - 1
            };
            let spread = self.spread_active();
            self.page_index = previous;
            self.crop_index = self.crop_count().saturating_sub(1);
            self.source_x = 0.0;
            self.source_y = if spread { 0.0 } else { 1.0 };
            self.restore_last_region = !spread && !self.layouts.contains_key(&self.page_index);
            self.restore_focus = false;
            self.focus_current_region();
            ReaderMove::Moved
        }
    }

    /// Decode work belongs on an application worker, never on the UI thread.
    pub fn render(&mut self, path: &Path, viewport: Viewport) -> Result<RenderedPage> {
        viewport.validate()?;
        let source = load_validated_source(path)?;
        self.render_validated_source(&source, viewport)
    }

    /// Run the normal path API with optional stage timing enabled for this call.
    /// File-system cache state is not changed by profiling; the first source
    /// decode and later retained-source reuse must be reported separately.
    pub fn render_profiled(&mut self, path: &Path, viewport: Viewport)
        -> Result<(RenderedPage, ReaderPreparationProfile)>
    {
        viewport.validate()?;
        let mut profile = ReaderPreparationProfile::default();
        let source = load_validated_source_with_profile(path, ReaderResources::shared(), Some(&mut profile))?;
        let frame = self.render_validated_source_profiled(&source, viewport, &mut profile)?;
        Ok((frame, profile))
    }

    /// Exercise layouts directly with an immutable synthetic source in tests.
    #[cfg(test)]
    pub(crate) fn render_source(&mut self, source: &GrayImage, viewport: Viewport) -> Result<RenderedPage> {
        self.render_source_identity(source, viewport, None, None)
    }

    pub(crate) fn render_validated_source(&mut self, source: &ValidatedGraySource, viewport: Viewport) -> Result<RenderedPage> {
        self.render_source_identity(&source.pixels, viewport, Some(source.proof.sha256()), None)
    }

    pub(crate) fn render_validated_source_profiled(&mut self, source: &ValidatedGraySource, viewport: Viewport,
        profile: &mut ReaderPreparationProfile) -> Result<RenderedPage>
    {
        self.render_source_identity(&source.pixels, viewport, Some(source.proof.sha256()), Some(profile))
    }

    /// Cancel obsolete pixel work without publishing a partial frame. This is
    /// cooperative: allocator, image-decoder and OS I/O calls have no hard
    /// cancellation deadline. A cancelled worker core must not be presented.
    pub(crate) fn render_validated_source_cancellable(&mut self, source: &ValidatedGraySource, viewport: Viewport,
        cancelled: &AtomicBool, profile: Option<&mut ReaderPreparationProfile>) -> Result<Option<RenderedPage>>
    {
        self.render_source_identity_cancellable(&source.pixels, viewport, Some(source.proof.sha256()),
            &RenderCancellation::for_flag(cancelled), profile)
    }

    fn render_source_identity(&mut self, source: &GrayImage, viewport: Viewport, identity: Option<&str>,
        profile: Option<&mut ReaderPreparationProfile>) -> Result<RenderedPage>
    {
        Ok(self.render_source_identity_cancellable(source, viewport, identity, &RenderCancellation::never(), profile)?
            .expect("An uncancelled reader render cannot return cancellation"))
    }

    fn render_source_identity_cancellable(&mut self, source: &GrayImage, viewport: Viewport, identity: Option<&str>,
        cancellation: &RenderCancellation<'_>, mut profile: Option<&mut ReaderPreparationProfile>) -> Result<Option<RenderedPage>>
    {
        if cancellation.is_cancelled(RenderWork::Boundary, 0) { return Ok(None); }
        let started = profile_start(&profile);
        let validated = (|| {
            viewport.validate()?;
            if source.width() == 0 || source.height() == 0
                || u64::from(source.width()) * u64::from(source.height()) > MAX_SOURCE_PIXELS {
                return Err(ReaderError::SourceImageTooLarge);
            }
            Ok(())
        })();
        profile_elapsed(&mut profile, started, "frame_validation", validated.is_ok(), None);
        validated?;
        let started = profile_start(&profile);
        let grayscale = match self.settings.rotation {
            90 => Cow::Owned(image::imageops::rotate90(source)),
            180 => Cow::Owned(image::imageops::rotate180(source)),
            270 => Cow::Owned(image::imageops::rotate270(source)),
            _ => Cow::Borrowed(source),
        };
        profile_elapsed(&mut profile, started, "rotation", true, Some(|profile| &mut profile.rotation_us));
        if cancellation.is_cancelled(RenderWork::Boundary, 0) { return Ok(None); }
        let started = profile_start(&profile);
        let layout_result = (|| -> Result<_> {
            let width = grayscale.width();
            let height = grayscale.height();
            let viewport_changed = self.last_viewport.is_some_and(|previous| previous != viewport);
            self.last_viewport = Some(viewport);
            let source_changed = self.layouts.get(&self.page_index)
                .is_some_and(|layout| layout.source_identity.as_deref() != identity);
            let replace_layout = self.layouts.get(&self.page_index).is_none_or(|layout| {
                layout.viewport != viewport || layout.width != width || layout.height != height || source_changed
            });
            if replace_layout {
                if let Some(profile) = profile.as_deref_mut() {
                    profile.layout_rebuilds = profile.layout_rebuilds.saturating_add(1);
                }
                if viewport_changed || source_changed {
                    self.restore_focus = true;
                    self.preserve_region_for_pan = false;
                }
                let (regions, heuristic) = match self.settings.mode {
                    DisplayMode::Panel => {
                        if let Some(panels) = self.explicit_panels.get(&self.page_index) {
                            (panels.iter().map(|panel| Region {
                                rect: panel.pixel_rect(width, height, self.settings.rotation), overlap: false,
                            }).collect(), false)
                        } else { (panel_regions(&grayscale, viewport, self.settings.direction)?, true) }
                    }
                    // Strip and panel regions belong to their reading modes;
                    // fit choices never add hidden windows to a source page.
                    DisplayMode::Strip => (strip_regions(&grayscale, viewport)?, false),
                    DisplayMode::Page => (vec![Region { rect: SourceRect::whole(width, height), overlap: false }], false),
                };
                self.layouts.insert(self.page_index, PageLayout { viewport, width, height, regions, heuristic,
                    source_identity: identity.map(str::to_owned) });
                self.trim_layouts();
            }
            let layout = self.layouts.get(&self.page_index).ok_or(ReaderError::InvalidDescriptor)?;
            if self.restore_last_region {
                self.crop_index = layout.regions.len() - 1;
                self.restore_last_region = false;
                self.restore_focus = false;
            } else if self.restore_focus {
                let (focus_x, focus_y) = rotate_point(self.source_x, self.source_y, self.settings.rotation);
                let px = ((focus_x * f64::from(width)).round() as u32).min(width - 1);
                let py = ((focus_y * f64::from(height)).round() as u32).min(height - 1);
                let preferred = self.preserve_region_for_pan.then_some(self.crop_index);
                self.crop_index = region_for_focus(&layout.regions, px, py, preferred);
                self.restore_focus = false;
            }
            self.preserve_region_for_pan = false;
            self.crop_index = self.crop_index.min(layout.regions.len() - 1);
            let region = layout.regions[self.crop_index];
            let focus = self.explicit_pan.then(|| {
                let point = rotate_point(self.source_x, self.source_y, self.settings.rotation);
                (point.0 * f64::from(width), point.1 * f64::from(height))
            });
            let geometry = fit_geometry(region.rect, viewport, self.settings.fit, self.settings.zoom_ratio, focus);
            if geometry.sample.is_none()
                && triangle_intermediate_bytes(geometry.crop.width, geometry.height)
                    .min(triangle_intermediate_bytes(geometry.crop.height, geometry.width))
                    > MAX_DECODE_ALLOCATION
            {
                return Err(ReaderError::SourceImageTooLarge);
            }
            let crop_count = layout.regions.len();
            let heuristic = layout.heuristic;
            Ok((width, height, geometry, crop_count, heuristic, region.overlap))
        })();
        let completed = layout_result.is_ok();
        profile_elapsed(&mut profile, started, "layout", completed,
            completed.then_some(|profile: &mut ReaderPreparationProfile| &mut profile.layout_us));
        let (width, height, geometry, crop_count, heuristic, overlap_from_previous) = layout_result?;
        let FitGeometry { crop, width: target_width, height: target_height, sample, origin_x, origin_y } = geometry;
        if cancellation.is_cancelled(RenderWork::Boundary, 0) { return Ok(None); }
        let started = profile_start(&profile);
        let resized = if let Some(sample) = sample {
            resize_centered_sample(grayscale.as_ref(), sample, target_width, target_height, cancellation)
        } else {
            self.resize_crop_cancellable(grayscale.as_ref(), crop, target_width, target_height, cancellation)
        };
        let completed = resized.is_some();
        profile_elapsed(&mut profile, started, "resize", completed,
            completed.then_some(|profile: &mut ReaderPreparationProfile| &mut profile.resize_us));
        let Some(resized) = resized else { return Ok(None); };
        let started = profile_start(&profile);
        drop(grayscale);
        let left = (viewport.width - target_width) / 2;
        let top = (viewport.height - target_height) / 2;
        let painted = self.paint_canvas_cancellable(&resized, viewport, left, top, cancellation);
        let completed = painted.is_some();
        profile_elapsed(&mut profile, started, "canvas", completed,
            completed.then_some(|profile: &mut ReaderPreparationProfile| &mut profile.canvas_us));
        let Some((canvas, dark_pixels)) = painted else { return Ok(None); };
        if let Some(profile) = profile.as_deref_mut() {
            profile.rendered_frames = profile.rendered_frames.saturating_add(1);
            profile.resized_pixels = profile.resized_pixels.saturating_add(u64::from(target_width) * u64::from(target_height));
            profile.output_bytes = profile.output_bytes.saturating_add(u64::from(viewport.width) * u64::from(viewport.height) * 4);
        }
        let (source_x, source_y) = unrotate_point(
            origin_x / f64::from(width), origin_y / f64::from(height), self.settings.rotation);
        self.source_x = source_x;
        self.source_y = source_y;
        Ok(Some(RenderedPage {
            rgba: canvas, width: viewport.width, height: viewport.height,
            source_width: width, source_height: height, crop, page_index: self.page_index,
            crop_index: self.crop_index, crop_count, overlap_from_previous,
            heuristic_panels: heuristic,
            dark_fraction: dark_pixels as f32 / (u64::from(viewport.width) * u64::from(viewport.height)) as f32,
        }))
    }

    fn resize_crop_cancellable(&self, source: &GrayImage, crop: SourceRect, width: u32, height: u32,
        cancellation: &RenderCancellation<'_>) -> Option<GrayImage>
    {
        if !cancellation.is_enabled() { return Some(self.resize_crop(source, crop, width, height)); }
        if transpose_triangle_resize(crop, width, height) {
            if cancellation.is_cancelled(RenderWork::Boundary, 0) { return None; }
            let view = image::imageops::crop_imm(source, crop.x, crop.y, crop.width, crop.height);
            let transposed = image::imageops::rotate90(&*view);
            let resized = self.resize_crop_cancellable(&transposed,
                SourceRect::whole(crop.height, crop.width), height, width, cancellation)?;
            if cancellation.is_cancelled(RenderWork::Boundary, 0) { return None; }
            let output = image::imageops::rotate270(&resized);
            return (!cancellation.is_cancelled(RenderWork::Boundary, 0)).then_some(output);
        }
        #[cfg(test)]
        if self.raster_path == TestRasterPath::Reference {
            if cancellation.is_cancelled(RenderWork::Boundary, 0) { return None; }
            let resized = self.resize_crop(source, crop, width, height);
            return (!cancellation.is_cancelled(RenderWork::Boundary, 0)).then_some(resized);
        }
        triangle::resize_cancellable(source, crop, width, height, cancellation)
    }

    fn resize_crop(&self, source: &GrayImage, crop: SourceRect, width: u32, height: u32) -> GrayImage {
        if transpose_triangle_resize(crop, width, height) {
            let view = image::imageops::crop_imm(source, crop.x, crop.y, crop.width, crop.height);
            let transposed = image::imageops::rotate90(&*view);
            let resized = self.resize_crop(&transposed, SourceRect::whole(crop.height, crop.width), height, width);
            return image::imageops::rotate270(&resized);
        }
        #[cfg(test)]
        if self.raster_path == TestRasterPath::Reference {
            let view = image::imageops::crop_imm(source, crop.x, crop.y, crop.width, crop.height);
            return image::imageops::resize(&*view, width, height, image::imageops::FilterType::Triangle);
        }
        triangle::resize(source, crop, width, height)
    }

    fn paint_canvas(&self, source: &GrayImage, viewport: Viewport, left: u32, top: u32) -> (Vec<u8>, u64) {
        #[cfg(test)]
        if self.raster_path == TestRasterPath::Reference && !self.settings.uses_hardware_dither() {
            return profile_tests::canvas_reference(source, viewport, left, top, self.settings.dither);
        }
        paint_quantized_canvas(source, viewport, left, top, self.settings.dither,
            self.settings.uses_hardware_dither())
    }

    fn paint_canvas_cancellable(&self, source: &GrayImage, viewport: Viewport, left: u32, top: u32,
        cancellation: &RenderCancellation<'_>) -> Option<(Vec<u8>, u64)>
    {
        if !cancellation.is_enabled() { return Some(self.paint_canvas(source, viewport, left, top)); }
        #[cfg(test)]
        if self.raster_path == TestRasterPath::Reference {
            if cancellation.is_cancelled(RenderWork::Boundary, 0) { return None; }
            let canvas = self.paint_canvas(source, viewport, left, top);
            return (!cancellation.is_cancelled(RenderWork::Boundary, 0)).then_some(canvas);
        }
        paint_quantized_canvas_cancellable(source, viewport, left, top, self.settings.dither,
            self.settings.uses_hardware_dither(), cancellation)
    }

    fn spread_base(&self) -> usize {
        if self.settings.cover_alone {
            if self.page_index == 0 { 0 } else { 1 + ((self.page_index - 1) / 2) * 2 }
        } else { (self.page_index / 2) * 2 }
    }

    fn spread_active(&self) -> bool {
        self.settings.landscape_spread
            && self.settings.mode == DisplayMode::Page
            && self.settings.fit == FitMode::Page
            && self.landscape_override.unwrap_or_else(||
                self.last_viewport.is_some_and(|viewport| viewport.width > viewport.height))
    }

    fn focus_current_region(&mut self) {
        if let Some(layout) = self.layouts.get(&self.page_index) {
            if let Some(region) = layout.regions.get(self.crop_index) {
                let (x, y) = unrotate_point(f64::from(region.rect.x) / f64::from(layout.width),
                    f64::from(region.rect.y) / f64::from(layout.height), self.settings.rotation);
                self.source_x = x;
                self.source_y = y;
                self.restore_focus = false;
                self.preserve_region_for_pan = false;
                self.explicit_pan = false;
                // An explicit region move supersedes a deferred jump to the last viewport.
                self.restore_last_region = false;
            }
        }
    }

    fn trim_layouts(&mut self) {
        while self.layouts.len() > MAX_REMEMBERED_LAYOUTS {
            let key = self.layouts.keys().copied().max_by_key(|index| index.abs_diff(self.page_index));
            if let Some(key) = key { self.layouts.remove(&key); } else { break; }
        }
    }
}

fn validate_coordinates(x: f64, y: f64) -> Result<()> {
    if !x.is_finite() || !y.is_finite() || !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return Err(ReaderError::InvalidAnchor);
    }
    Ok(())
}

#[cfg(test)]
fn load_gray_source_with_resources(path: &Path, resources: &ReaderResources) -> Result<Arc<GrayImage>> {
    Ok(load_validated_source_with_resources(path, resources)?.pixels)
}

pub(crate) fn load_validated_source(path: &Path) -> Result<ValidatedGraySource> {
    load_validated_source_with_resources(path, ReaderResources::shared())
}

fn load_validated_source_with_resources(path: &Path, resources: &ReaderResources) -> Result<ValidatedGraySource> {
    load_validated_source_with_profile(path, resources, None)
}

fn load_validated_source_with_profile(path: &Path, resources: &ReaderResources,
    mut profile: Option<&mut ReaderPreparationProfile>) -> Result<ValidatedGraySource>
{
    let encoded = read_source_bytes_profiled(path, &mut profile)?;
    decode_source_bytes_profiled(&encoded, resources, None, profile)?.ok_or(ReaderError::InvalidDescriptor)
}

pub(crate) fn load_changed_source(path: &Path, previous: Option<&ValidatedImage>) -> Result<Option<ValidatedGraySource>> {
    load_changed_source_with_profile(path, previous, None)
}

/// Profile the existing path source API without changing its cancellation rules.
/// The application checks its render cancellation around this demonstration
/// source operation; receipt sources retain their cancellable input API.
pub(crate) fn load_changed_source_profiled(path: &Path, previous: Option<&ValidatedImage>,
    profile: &mut ReaderPreparationProfile) -> Result<Option<ValidatedGraySource>>
{
    load_changed_source_with_profile(path, previous, Some(profile))
}

fn load_changed_source_with_profile(path: &Path, previous: Option<&ValidatedImage>,
    mut profile: Option<&mut ReaderPreparationProfile>) -> Result<Option<ValidatedGraySource>>
{
    let encoded = read_source_bytes_profiled(path, &mut profile)?;
    decode_source_bytes_profiled(&encoded, ReaderResources::shared(), previous, profile)
}

/// Prepare pixels from the resolver's single immutable read and digest.
/// The outer None is cancellation; an inner None is an unchanged sealed source.
pub(crate) fn load_hashed_source(input: &HashedImage, previous: Option<&ValidatedImage>, cancelled: &AtomicBool)
    -> std::result::Result<Option<(ValidatedImage, Option<ValidatedGraySource>)>, IntegrityError>
{
    load_hashed_source_with_profile(input, previous, cancelled, None)
}

pub(crate) fn load_hashed_source_profiled(input: &HashedImage, previous: Option<&ValidatedImage>, cancelled: &AtomicBool,
    profile: &mut ReaderPreparationProfile)
    -> std::result::Result<Option<(ValidatedImage, Option<ValidatedGraySource>)>, IntegrityError>
{
    load_hashed_source_with_profile(input, previous, cancelled, Some(profile))
}

fn load_hashed_source_with_profile(input: &HashedImage, previous: Option<&ValidatedImage>, cancelled: &AtomicBool,
    mut profile: Option<&mut ReaderPreparationProfile>)
    -> std::result::Result<Option<(ValidatedImage, Option<ValidatedGraySource>)>, IntegrityError>
{
    let resources = ReaderResources::shared();
    let is_cancelled = || cancelled.load(Ordering::Acquire);
    // A pre-cancelled call did no image work and retains the original empty
    // profile outcome. The unprofiled path keeps its existing check ordering.
    if profile.is_some() && is_cancelled() { return Ok(None); }
    let started = profile_start(&profile);
    let result = crate::image_integrity::decode_hashed_image_with_reuse_cancellable_profiled(input, None,
        |digest| reusable_source(resources, previous, digest), &is_cancelled,
        profile.as_deref_mut().map(|profile| &mut profile.stages));
    let completed = matches!(&result, Ok(Some(_)));
    profile_elapsed(&mut profile, started, "aggregate_integrity", completed,
        completed.then_some(|profile: &mut ReaderPreparationProfile| &mut profile.integrity_decode_us));
    let Some((proof, decoded)) = result.map_err(|error| {
        if let Some(evidence) = error.encoding_evidence() { resources.remove_digest(evidence.sha256()); }
        error
    })? else { return Ok(None); };
    profile_source(&mut profile, &decoded);
    let started = profile_start(&profile);
    let source = prepare_gray_source(&proof, decoded, resources, &is_cancelled);
    let completed = profile.is_none() || !is_cancelled();
    profile_elapsed(&mut profile, started, "grayscale", completed, Some(|profile| &mut profile.grayscale_us));
    if is_cancelled() { return Ok(None); }
    Ok(Some((proof, source)))
}

fn read_source_bytes_profiled(path: &Path, profile: &mut Option<&mut ReaderPreparationProfile>) -> Result<Vec<u8>> {
    let started = profile_start(profile);
    let result = read_source_bytes(path);
    let completed = result.is_ok();
    profile_elapsed(profile, started, "source_read", completed,
        completed.then_some(|profile: &mut ReaderPreparationProfile| &mut profile.source_read_us));
    result
}

fn read_source_bytes(path: &Path) -> Result<Vec<u8>> {
    let file = File::open(path)?;
    if file.metadata()?.len() > MAX_ENCODED_BYTES { return Err(ReaderError::EncodedImageTooLarge); }
    let mut encoded = Vec::new();
    file.take(MAX_ENCODED_BYTES + 1).read_to_end(&mut encoded)?;
    if encoded.len() as u64 > MAX_ENCODED_BYTES { return Err(ReaderError::EncodedImageTooLarge); }
    Ok(encoded)
}

#[cfg(test)]
fn decode_bounded_bytes(encoded: &[u8]) -> Result<Arc<GrayImage>> {
    decode_bounded_bytes_with_resources(encoded, ReaderResources::shared())
}

#[cfg(test)]
fn decode_bounded_bytes_with_resources(encoded: &[u8], resources: &ReaderResources) -> Result<Arc<GrayImage>> {
    decode_source_bytes(encoded, resources, None)?.map(|source| source.pixels).ok_or(ReaderError::InvalidDescriptor)
}

fn decode_source_bytes(encoded: &[u8], resources: &ReaderResources, previous: Option<&ValidatedImage>)
    -> Result<Option<ValidatedGraySource>>
{
    decode_source_bytes_profiled(encoded, resources, previous, None)
}

fn decode_source_bytes_profiled(encoded: &[u8], resources: &ReaderResources, previous: Option<&ValidatedImage>,
    mut profile: Option<&mut ReaderPreparationProfile>) -> Result<Option<ValidatedGraySource>>
{
    if encoded.len() as u64 > MAX_ENCODED_BYTES { return Err(ReaderError::EncodedImageTooLarge); }
    let started = profile_start(&profile);
    let result = crate::image_integrity::decode_image_with_reuse_profiled(encoded, None,
        |digest| reusable_source(resources, previous, digest),
        profile.as_deref_mut().map(|profile| &mut profile.stages));
    let completed = result.is_ok();
    profile_elapsed(&mut profile, started, "aggregate_integrity", completed,
        completed.then_some(|profile: &mut ReaderPreparationProfile| &mut profile.integrity_decode_us));
    let (proof, decoded) = result.map_err(|error| {
        if let Some(evidence) = error.encoding_evidence() { resources.remove_digest(evidence.sha256()); }
        match error.kind {
            crate::image_integrity::IntegrityFailureKind::ResourceLimit => ReaderError::SourceImageTooLarge,
            crate::image_integrity::IntegrityFailureKind::Unsupported => ReaderError::UnsupportedFormat,
            crate::image_integrity::IntegrityFailureKind::Corrupt => ReaderError::CorruptImage,
        }
    })?;
    profile_source(&mut profile, &decoded);
    let started = profile_start(&profile);
    let source = prepare_gray_source(&proof, decoded, resources, &|| false);
    profile_elapsed(&mut profile, started, "grayscale", true, Some(|profile| &mut profile.grayscale_us));
    Ok(source)
}

fn reusable_source(resources: &ReaderResources, previous: Option<&ValidatedImage>, digest: &str)
    -> Option<(ValidatedImage, Option<Arc<GrayImage>>)>
{
    if let Some(proof) = previous.filter(|proof| proof.sha256() == digest) {
        Some((proof.clone(), None))
    } else { resources.lookup_digest(digest).map(|(proof, pixels)| (proof, Some(pixels))) }
}

fn prepare_gray_source(proof: &ValidatedImage,
    decoded: crate::image_integrity::ReusableImage<Option<Arc<GrayImage>>>,
    resources: &ReaderResources, is_cancelled: &dyn Fn() -> bool) -> Option<ValidatedGraySource>
{
    if is_cancelled() { return None; }
    let decoded = match decoded {
        crate::image_integrity::ReusableImage::Decoded(decoded) => decoded,
        crate::image_integrity::ReusableImage::Reused(Some(pixels)) => return Some(ValidatedGraySource { proof: proof.clone(), pixels }),
        crate::image_integrity::ReusableImage::Reused(None) => return None,
    };
    let grayscale = match decoded {
        // A grayscale source already has exactly the required opaque pixels.
        image::DynamicImage::ImageLuma8(grayscale) => grayscale,
        decoded => {
            let (width, height) = decoded.dimensions();
            let mut grayscale = GrayImage::new(width, height);
            for (x, y, pixel) in decoded.pixels() {
                if x == 0 && y % 32 == 0 && is_cancelled() { return None; }
                let alpha = u32::from(pixel[3]);
                let luma = (54 * u32::from(pixel[0]) + 183 * u32::from(pixel[1]) + 19 * u32::from(pixel[2]) + 128) / 256;
                let composite = ((luma * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
                grayscale.put_pixel(x, y, image::Luma([composite]));
            }
            grayscale
        },
    };
    if is_cancelled() { return None; }
    let grayscale = Arc::new(grayscale);
    resources.insert_source(proof, grayscale.clone());
    Some(ValidatedGraySource { proof: proof.clone(), pixels: grayscale })
}

/// A fit preset changes one selected source view, never its reading-region count.
fn fit_geometry(region: SourceRect, viewport: Viewport, fit: FitMode, zoom: f64,
    focus: Option<(f64, f64)>) -> FitGeometry
{
    let source_width = f64::from(region.width);
    let source_height = f64::from(region.height);
    if fit == FitMode::Page {
        let crop = centered_source_window(region, source_width / zoom, source_height / zoom, focus);
        return FitGeometry { crop, width: viewport.width, height: viewport.height, sample: None,
            origin_x: f64::from(crop.x), origin_y: f64::from(crop.y) };
    }
    let width_scale = f64::from(viewport.width) / source_width;
    let height_scale = f64::from(viewport.height) / source_height;
    let scale = match fit {
        FitMode::Original => 1.0f64.min(width_scale).min(height_scale),
        FitMode::Height => height_scale,
        FitMode::Width => width_scale,
        FitMode::Page => unreachable!("Stretch geometry was handled above"),
    } * zoom;
    let visible_width = source_width.min(f64::from(viewport.width) / scale);
    let visible_height = source_height.min(f64::from(viewport.height) / scale);
    let output_width = (source_width * scale).round().clamp(1.0, f64::from(viewport.width)) as u32;
    let output_height = (source_height * scale).round().clamp(1.0, f64::from(viewport.height)) as u32;
    if scale > 1.0 && (visible_width < source_width || visible_height < source_height) {
        let origin_x = focus.map_or(f64::from(region.x) + (source_width - visible_width) / 2.0,
            |point| point.0.clamp(f64::from(region.x), f64::from(region.x) + source_width - visible_width));
        let origin_y = focus.map_or(f64::from(region.y) + (source_height - visible_height) / 2.0,
            |point| point.1.clamp(f64::from(region.y), f64::from(region.y) + source_height - visible_height));
        let sample = CenteredSample { bounds: region, center_x: origin_x + visible_width / 2.0,
            center_y: origin_y + visible_height / 2.0, scale };
        let crop = guarded_source_window(region, origin_x, origin_y, visible_width, visible_height);
        return FitGeometry { crop, width: output_width, height: output_height, sample: Some(sample), origin_x, origin_y };
    }
    // At downsampling scales, an integer source window differs from the ideal
    // centered affine by at most one output pixel. Keep the existing Triangle
    // antialiasing and compute both output dimensions from the same scale.
    let crop = centered_source_window(region, visible_width, visible_height, focus);
    let width = (f64::from(crop.width) * scale).round().clamp(1.0, f64::from(viewport.width)) as u32;
    let height = (f64::from(crop.height) * scale).round().clamp(1.0, f64::from(viewport.height)) as u32;
    FitGeometry { crop, width, height, sample: None, origin_x: f64::from(crop.x), origin_y: f64::from(crop.y) }
}

fn centered_source_window(region: SourceRect, width: f64, height: f64, focus: Option<(f64, f64)>) -> SourceRect {
    let width = width.round().clamp(1.0, f64::from(region.width)) as u32;
    let height = height.round().clamp(1.0, f64::from(region.height)) as u32;
    let x = focus.map_or(region.x + (region.width - width) / 2,
        |point| (point.0.round() as u32).clamp(region.x, region.x + region.width - width));
    let y = focus.map_or(region.y + (region.height - height) / 2,
        |point| (point.1.round() as u32).clamp(region.y, region.y + region.height - height));
    SourceRect { x, y, width, height }
}

fn snap_source_boundary(value: f64) -> f64 {
    let integer = value.round();
    if (value - integer).abs() <= 1e-9 { integer } else { value }
}

/// Metadata uses a guarded integer envelope; fractional sampling uses its own
/// continuous window and the anchor retains that window's normalized origin.
fn guarded_source_window(region: SourceRect, x: f64, y: f64, width: f64, height: f64) -> SourceRect {
    let left = snap_source_boundary(x).floor().clamp(f64::from(region.x), f64::from(region.x + region.width - 1)) as u32;
    let top = snap_source_boundary(y).floor().clamp(f64::from(region.y), f64::from(region.y + region.height - 1)) as u32;
    let right = snap_source_boundary(x + width).ceil().clamp(f64::from(left + 1), f64::from(region.x + region.width)) as u32;
    let bottom = snap_source_boundary(y + height).ceil().clamp(f64::from(top + 1), f64::from(region.y + region.height)) as u32;
    SourceRect { x: left, y: top, width: right - left, height: bottom - top }
}

fn triangle_intermediate_bytes(source_width: u32, target_height: u32) -> u64 {
    u64::from(source_width).saturating_mul(u64::from(target_height)).saturating_mul(std::mem::size_of::<f32>() as u64)
}

/// A failed Triangle weight plan uses a full source-width intermediate. Stretch
/// can make that unsafe for extreme aspect ratios. Swapping the axes retains
/// Triangle filtering with a bounded alternative pass order; it adds only a
/// source-sized rotation and a viewport-sized output, never a full enlarged page.
fn transpose_triangle_resize(crop: SourceRect, width: u32, height: u32) -> bool {
    let normal = triangle_intermediate_bytes(crop.width, height);
    let alternate = triangle_intermediate_bytes(crop.height, width);
    normal > MAX_DECODE_ALLOCATION && alternate < normal
}

/// Triangle upsampling is bilinear. Sample only the bounded visible output,
/// rather than allocating the potentially enormous full enlarged source.
fn resize_centered_sample(source: &GrayImage, sample: CenteredSample, width: u32, height: u32,
    cancellation: &RenderCancellation<'_>) -> Option<GrayImage>
{
    if cancellation.is_cancelled(RenderWork::Boundary, 0) { return None; }
    let mut output = source.buffer_with_dimensions(width, height);
    let output_pixels: &mut [u8] = output.as_mut();
    let source_pixels = source.as_raw();
    let stride = source.width() as usize;
    let right = sample.bounds.x + sample.bounds.width - 1;
    let bottom = sample.bounds.y + sample.bounds.height - 1;
    for y in 0..height {
        let source_y = (sample.center_y + (f64::from(y) + 0.5 - f64::from(height) / 2.0) / sample.scale - 0.5)
            .clamp(f64::from(sample.bounds.y), f64::from(bottom));
        let top = source_y.floor() as u32;
        let next_y = top.saturating_add(1).min(bottom);
        let y_weight = source_y - f64::from(top);
        for first in (0..width as usize).step_by(RENDER_PIXEL_BLOCK) {
            let completed = y as usize * width as usize + first;
            if cancellation.is_cancelled(RenderWork::ResizeHorizontal, completed) { return None; }
            for x in first..(first + RENDER_PIXEL_BLOCK).min(width as usize) {
                let source_x = (sample.center_x + (x as f64 + 0.5 - f64::from(width) / 2.0) / sample.scale - 0.5)
                    .clamp(f64::from(sample.bounds.x), f64::from(right));
                let left = source_x.floor() as u32;
                let next_x = left.saturating_add(1).min(right);
                let x_weight = source_x - f64::from(left);
                let top_left = f64::from(source_pixels[top as usize * stride + left as usize]);
                let top_right = f64::from(source_pixels[top as usize * stride + next_x as usize]);
                let bottom_left = f64::from(source_pixels[next_y as usize * stride + left as usize]);
                let bottom_right = f64::from(source_pixels[next_y as usize * stride + next_x as usize]);
                let first = top_left + (top_right - top_left) * x_weight;
                let second = bottom_left + (bottom_right - bottom_left) * x_weight;
                let value = first + (second - first) * y_weight;
                // Keep half-way values stable after normalized anchor round trips.
                output_pixels[y as usize * width as usize + x] = (value + 0.5 + 1e-9).floor().clamp(0.0, 255.0) as u8;
            }
        }
    }
    (!cancellation.is_cancelled(RenderWork::Boundary, 0)).then_some(output)
}

fn strip_regions(image: &GrayImage, viewport: Viewport) -> Result<Vec<Region>> {
    let width = image.width();
    let height = image.height();
    let target = ((u64::from(viewport.height) * u64::from(width)) / u64::from(viewport.width))
        .max(1).min(u64::from(height)) as u32;
    let overlap = ((64.0 * f64::from(viewport.scale_factor) * f64::from(width) / f64::from(viewport.width)).round() as u32)
        .min(target.saturating_sub(1));
    let gutters = white_runs(image, true, SourceRect::whole(width, height))?;
    let mut regions = Vec::new();
    let mut top = 0u32;
    let mut overlaps_previous = false;
    loop {
        if regions.len() >= MAX_LAYOUT_REGIONS { return Err(ReaderError::LayoutTooComplex); }
        let bottom = top.saturating_add(target).min(height);
        if bottom == height {
            regions.push(Region { rect: SourceRect { x: 0, y: top, width, height: height - top }, overlap: overlaps_previous });
            break;
        }
        let earliest_cut = top.saturating_add((target * 45 / 100).max(1));
        let cut = gutters.iter().map(|(start, end)| start + (end - start) / 2)
            .filter(|cut| *cut >= earliest_cut && *cut <= bottom).max();
        let next = cut.unwrap_or_else(|| bottom.saturating_sub(overlap).max(top + 1));
        regions.push(Region {
            rect: SourceRect { x: 0, y: top, width, height: cut.unwrap_or(bottom) - top },
            overlap: overlaps_previous,
        });
        top = next;
        overlaps_previous = cut.is_none() && overlap > 0;
    }
    Ok(regions)
}

fn panel_regions(image: &GrayImage, viewport: Viewport, direction: PageDirection) -> Result<Vec<Region>> {
    let whole = SourceRect::whole(image.width(), image.height());
    let horizontal = white_runs(image, true, whole)?;
    let bands = content_bands(image.height(), &horizontal);
    let minimum_width = (image.width() / 20).max(8);
    let minimum_height = (image.height() / 40).max(8);
    let mut regions = Vec::new();
    for (top, bottom) in bands {
        if bottom - top < minimum_height { continue; }
        let band = SourceRect { x: 0, y: top, width: image.width(), height: bottom - top };
        let columns = content_bands(image.width(), &white_runs(image, false, band)?);
        let mut band_regions = Vec::new();
        for (left, right) in columns {
            if right - left < minimum_width { continue; }
            band_regions.push(Region { rect: SourceRect { x: left, y: top, width: right - left, height: bottom - top }, overlap: false });
        }
        if direction == PageDirection::RightToLeft { band_regions.reverse(); }
        regions.extend(band_regions);
        if regions.len() > MAX_LAYOUT_REGIONS { return Err(ReaderError::LayoutTooComplex); }
    }
    if regions.len() <= 1 {
        return strip_regions(image, viewport);
    }
    Ok(regions)
}

/// Detect broad, nearly white gutters while sampling at most 512 pixels per line.
fn white_runs(image: &GrayImage, horizontal: bool, area: SourceRect) -> Result<Vec<(u32, u32)>> {
    let length = if horizontal { area.height } else { area.width };
    let cross = if horizontal { area.width } else { area.height };
    let stride = (cross / 512).max(1);
    let minimum_run = (cross / 500).max(3);
    let mut runs = Vec::new();
    let mut start = None;
    for position in 0..length {
        let mut samples = 0u32;
        let mut white = 0u32;
        for other in (0..cross).step_by(stride as usize) {
            let (x, y) = if horizontal { (area.x + other, area.y + position) } else { (area.x + position, area.y + other) };
            samples += 1;
            if image.get_pixel(x, y)[0] >= 245 { white += 1; }
        }
        let clear = samples > 0 && u64::from(white) * 100 >= u64::from(samples) * 98;
        match (start, clear) {
            (None, true) => start = Some(position),
            (Some(from), false) => {
                if position - from >= minimum_run && from > 0 && position < length {
                    if runs.len() >= MAX_LAYOUT_REGIONS { return Err(ReaderError::LayoutTooComplex); }
                    runs.push((from, position));
                }
                start = None;
            }
            _ => {}
        }
    }
    Ok(runs)
}

fn content_bands(length: u32, gutters: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut bands = Vec::new();
    let mut start = 0;
    for &(from, to) in gutters {
        if from > start { bands.push((start, from)); }
        start = to;
    }
    if start < length { bands.push((start, length)); }
    bands
}

fn nearest_region(regions: &[Region], x: u32, y: u32) -> usize {
    regions.iter().enumerate().min_by_key(|(_, region)| {
        let dx = x.abs_diff(region.rect.x);
        let dy = y.abs_diff(region.rect.y);
        u64::from(dx) * u64::from(dx) + u64::from(dy) * u64::from(dy)
    }).map_or(0, |(index, _)| index)
}

fn region_for_focus(regions: &[Region], x: u32, y: u32, preferred: Option<usize>) -> usize {
    if let Some(index) = preferred.filter(|index| regions.get(*index).is_some_and(|region| region.rect.contains(x, y))) {
        return index;
    }
    regions.iter().position(|region| region.rect.x == x && region.rect.y == y)
        .or_else(|| regions.iter().rposition(|region| region.rect.contains(x, y)))
        .unwrap_or_else(|| nearest_region(regions, x, y))
}

fn rotate_point(x: f64, y: f64, rotation: u16) -> (f64, f64) {
    match rotation {
        90 => (1.0 - y, x),
        180 => (1.0 - x, 1.0 - y),
        270 => (y, 1.0 - x),
        _ => (x, y),
    }
}

fn unrotate_point(x: f64, y: f64, rotation: u16) -> (f64, f64) {
    rotate_point(x, y, (360 - rotation) % 360)
}

const fn make_tone_tables() -> [[u8; 256]; 17] {
    let bayer: [[i16; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let mut tables = [[0u8; 256]; 17];
    let mut phase = 0;
    while phase < 17 {
        let bias = if phase == 16 { 0 } else { bayer[phase / 4][phase % 4] - 8 };
        let mut value = 0;
        while value < 256 {
            let adjusted = value as i16 + bias;
            let adjusted = if adjusted < 0 { 0 } else if adjusted > 255 { 255 } else { adjusted } as u16;
            let level = (adjusted + 8) / 17;
            tables[phase][value] = ((if level > 15 { 15 } else { level }) * 17) as u8;
            value += 1;
        }
        phase += 1;
    }
    tables
}

const TONE_TABLES: [[u8; 256]; 17] = make_tone_tables();
const RENDER_PIXEL_BLOCK: usize = 256;
const RENDER_COPY_BLOCK_BYTES: usize = 64 * 1024;

/// Paint complete, opaque RGBA rows without changing the crop-local Bayer phase.
fn paint_quantized_canvas(source: &GrayImage, viewport: Viewport, left: u32, top: u32,
    dither: bool, hardware_dither: bool) -> (Vec<u8>, u64)
{
    let mut canvas = vec![255; viewport.width as usize * viewport.height as usize * 4];
    let stride = viewport.width as usize * 4;
    let width = source.width() as usize;
    let mut dark = 0;
    for (y, row) in source.as_raw().chunks_exact(width).enumerate() {
        let start = (top as usize + y) * stride + left as usize * 4;
        let output = &mut canvas[start..start + width * 4];
        for (x, (&value, output)) in row.iter().zip(output.chunks_exact_mut(4)).enumerate() {
            let phase = if dither { (y % 4) * 4 + x % 4 } else { 16 };
            let tone = if dither && hardware_dither { value } else { TONE_TABLES[phase][usize::from(value)] };
            dark += u64::from(tone <= 68);
            output.copy_from_slice(&[tone, tone, tone, 255]);
        }
    }
    (canvas, dark)
}

/// Keep the normal canvas path intact; only application workers opt into these
/// bounded cancellation checks. No partly painted canvas escapes this function.
fn paint_quantized_canvas_cancellable(source: &GrayImage, viewport: Viewport, left: u32, top: u32,
    dither: bool, hardware_dither: bool, cancellation: &RenderCancellation<'_>) -> Option<(Vec<u8>, u64)>
{
    if !cancellation.is_enabled() { return Some(paint_quantized_canvas(source, viewport, left, top, dither, hardware_dither)); }
    if cancellation.is_cancelled(RenderWork::Boundary, 0) { return None; }
    let length = viewport.width as usize * viewport.height as usize * 4;
    let mut canvas = Vec::with_capacity(length);
    while canvas.len() < length {
        if cancellation.is_cancelled(RenderWork::CanvasFill, canvas.len()) { return None; }
        canvas.resize((canvas.len() + RENDER_COPY_BLOCK_BYTES).min(length), 255);
    }
    let stride = viewport.width as usize * 4;
    let width = source.width() as usize;
    let mut dark = 0;
    for (y, row) in source.as_raw().chunks_exact(width).enumerate() {
        let start = (top as usize + y) * stride + left as usize * 4;
        let output = &mut canvas[start..start + width * 4];
        for first in (0..width).step_by(RENDER_PIXEL_BLOCK) {
            if cancellation.is_cancelled(RenderWork::CanvasPixels, y * width + first) { return None; }
            let end = (first + RENDER_PIXEL_BLOCK).min(width);
            for (offset, (&value, output)) in row[first..end].iter()
                .zip(output[first * 4..end * 4].chunks_exact_mut(4)).enumerate() {
                let x = first + offset;
                let phase = if dither { (y % 4) * 4 + x % 4 } else { 16 };
                let tone = if dither && hardware_dither { value } else { TONE_TABLES[phase][usize::from(value)] };
                dark += u64::from(tone <= 68);
                output.copy_from_slice(&[tone, tone, tone, 255]);
            }
        }
    }
    if cancellation.is_cancelled(RenderWork::Boundary, 0) { return None; }
    Some((canvas, dark))
}

/// Keep the original arithmetic as an independent exhaustive test reference.
#[cfg(test)]
fn quantize(value: u8, x: u32, y: u32, dither: bool) -> u8 {
    const BAYER: [[i16; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let bias = if dither { BAYER[(y % 4) as usize][(x % 4) as usize] - 8 } else { 0 };
    let adjusted = (i16::from(value) + bias).clamp(0, 255) as u16;
    (((adjusted + 8) / 17).min(15) * 17) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::PageDescriptor;

    fn index(pages: usize) -> ImageIndex {
        ImageIndex {
            episode_id: "episode".into(), revision: "revision".into(),
            images: (0..pages).map(|index| PageDescriptor { id: format!("page-{index}"), index, width: 100, height: 140 }).collect(),
            ..ImageIndex::default()
        }
    }

    #[test]
    fn cover_spreads_follow_source_order_in_both_directions() {
        let mut reader = ReaderCore::new(index(6), ReaderSettings::default()).unwrap();
        reader.set_landscape(true);
        assert_eq!(reader.spread_indices(), [None, Some(0)]);
        assert_eq!(reader.step(true), ReaderMove::Moved);
        assert_eq!(reader.spread_indices(), [Some(1), Some(2)]);
        reader.settings.direction = PageDirection::RightToLeft;
        assert_eq!(reader.spread_indices(), [Some(2), Some(1)]);
        assert_eq!(reader.step(false), ReaderMove::Moved);
        assert_eq!(reader.spread_indices(), [Some(0), None]);
        assert_eq!(reader.step(false), ReaderMove::AtStart);
    }

    #[test]
    fn a_coverless_spread_restores_the_selected_source_leaf_in_both_directions() {
        for direction in [PageDirection::LeftToRight, PageDirection::RightToLeft] {
            let settings = ReaderSettings { direction, cover_alone: false, ..Default::default() };
            let mut reader = ReaderCore::new(index(6), settings.clone()).unwrap();
            reader.set_landscape(true);
            reader.set_page(3).unwrap();
            let expected = if direction == PageDirection::LeftToRight {
                [Some(2), Some(3)]
            } else { [Some(3), Some(2)] };
            assert_eq!(reader.spread_indices(), expected);

            let anchor = reader.anchor(123);
            let mut restored = ReaderCore::new(index(6), settings).unwrap();
            restored.set_landscape(true);
            restored.restore(&anchor).unwrap();
            assert_eq!(restored.page_index(), 3);
            assert_eq!(restored.spread_indices(), expected);
            assert_eq!(restored.step(true), ReaderMove::Moved);
            let expected_next = if direction == PageDirection::LeftToRight {
                [Some(4), Some(5)]
            } else { [Some(5), Some(4)] };
            assert_eq!(restored.spread_indices(), expected_next);
        }
    }

    #[test]
    fn returning_to_start_clears_a_finished_strip_position_and_restores_the_first_view() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("strip.png");
        GrayImage::from_pixel(100, 400, image::Luma([0])).save(&path).unwrap();
        let mut descriptor = index(2);
        for page in &mut descriptor.images { page.height = 400; }
        let settings = ReaderSettings { mode: DisplayMode::Strip, ..Default::default() };
        let mut reader = ReaderCore::new(descriptor.clone(), settings.clone()).unwrap();
        reader.set_page(1).unwrap();
        let frame = reader.render(&path, Viewport::new(100, 100)).unwrap();
        assert!(frame.crop_count > 1);
        while reader.step(true) == ReaderMove::Moved {}
        assert!(reader.is_transition());

        reader.to_start().unwrap();
        let anchor = reader.anchor(124);
        assert_eq!(anchor.page_index, 0);
        assert_eq!((anchor.source_x, anchor.source_y), (0.0, 0.0));
        assert_eq!(reader.crop_index(), 0);
        assert!(!anchor.finished);
        let mut restored = ReaderCore::new(descriptor, settings).unwrap();
        restored.restore(&anchor).unwrap();
        let first = restored.render(&path, Viewport::new(100, 100)).unwrap();
        assert_eq!(first.page_index, 0);
        assert_eq!(first.crop_index, 0);
        assert_eq!(first.crop.y, 0);
        assert!(!first.overlap_from_previous);
    }

    #[test]
    fn a_restored_strip_transition_returns_to_the_final_viewport_without_a_cached_layout() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("strip.png");
        GrayImage::from_pixel(100, 400, image::Luma([0])).save(&path).unwrap();
        let mut descriptor = index(2);
        for page in &mut descriptor.images { page.height = 400; }
        let settings = ReaderSettings { mode: DisplayMode::Strip, ..Default::default() };
        let mut reader = ReaderCore::new(descriptor.clone(), settings.clone()).unwrap();
        reader.set_page(1).unwrap();
        reader.render(&path, Viewport::new(100, 100)).unwrap();
        while reader.step(true) == ReaderMove::Moved {}
        let finished = reader.anchor(125);
        assert!(finished.finished);

        let mut restored = ReaderCore::new(descriptor.clone(), settings.clone()).unwrap();
        restored.restore(&finished).unwrap();
        assert_eq!(restored.step(false), ReaderMove::Moved);
        assert_eq!(restored.page_index(), 1);
        assert!(!restored.is_transition());
        let final_view = restored.render(&path, Viewport::new(100, 100)).unwrap();
        assert_eq!(final_view.crop_index + 1, final_view.crop_count);
        assert_eq!(final_view.crop.y + final_view.crop.height, final_view.source_height);

        let anchor = restored.anchor(126);
        let mut reopened = ReaderCore::new(descriptor, settings).unwrap();
        reopened.restore(&anchor).unwrap();
        let reopened_view = reopened.render(&path, Viewport::new(100, 100)).unwrap();
        assert_eq!(reopened_view.crop, final_view.crop);
        assert_eq!(reopened_view.rgba, final_view.rgba);
    }

    #[test]
    fn seeking_back_after_a_strip_transition_keeps_the_selected_viewport() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("strip.png");
        GrayImage::from_pixel(100, 400, image::Luma([0])).save(&path).unwrap();
        let mut descriptor = index(1);
        descriptor.images[0].height = 400;
        let settings = ReaderSettings { mode: DisplayMode::Strip, ..Default::default() };
        let mut reader = ReaderCore::new(descriptor, settings).unwrap();
        let first = reader.render(&path, Viewport::new(100, 100)).unwrap();
        assert!(first.crop_count > 3);
        while reader.step(true) == ReaderMove::Moved {}
        assert!(reader.is_transition());

        reader.back_to_last_view().unwrap();
        let target = reader.crop_count() - 3;
        while reader.crop_index() > target {
            assert_eq!(reader.step(false), ReaderMove::Moved);
        }
        let selected = reader.anchor(127);
        let frame = reader.render(&path, Viewport::new(100, 100)).unwrap();
        assert_eq!(frame.crop_index, target);
        assert_eq!(frame.crop.y, (selected.source_y * f64::from(frame.source_height)).round() as u32);
        assert!(frame.crop.y + frame.crop.height < frame.source_height);
        assert!(!reader.is_transition());
    }

    #[test]
    fn returning_to_the_last_page_uses_its_first_panel_or_centered_single_page() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("page.png");
        GrayImage::from_pixel(100, 400, image::Luma([0])).save(&path).unwrap();
        for settings in [
            ReaderSettings { mode: DisplayMode::Panel, ..Default::default() },
            ReaderSettings { fit: FitMode::Width, ..Default::default() },
        ] {
            let panel_mode = settings.mode == DisplayMode::Panel;
            let mut descriptor = index(2);
            for page in &mut descriptor.images { page.height = 400; }
            let mut reader = ReaderCore::new(descriptor, settings).unwrap();
            reader.set_page(1).unwrap();
            let frame = reader.render(&path, Viewport::new(100, 100)).unwrap();
            assert_eq!(frame.crop_count > 1, panel_mode);
            while reader.step(true) == ReaderMove::Moved {}
            assert!(reader.is_transition());

            reader.back_to_last_view().unwrap();
            assert_eq!(reader.page_index(), 1);
            assert_eq!(reader.crop_index(), 0);
            let frame = reader.render(&path, Viewport::new(100, 100)).unwrap();
            assert_eq!(frame.crop_index, 0);
            assert_eq!(frame.crop.y, if panel_mode { 0 } else { 150 });
            assert!(!reader.is_transition());
        }
    }

    #[test]
    fn stale_revision_cannot_restore_an_old_page_position() {
        let mut reader = ReaderCore::new(index(3), ReaderSettings::default()).unwrap();
        reader.set_page(2).unwrap();
        let mut anchor = reader.anchor(123);
        anchor.revision = "other".into();
        assert!(matches!(reader.restore(&anchor), Err(ReaderError::StaleAnchor)));
        assert_eq!(reader.page_index(), 2);
    }

    #[test]
    fn a_tall_panel_overlaps_but_a_visible_gutter_does_not() {
        let mut image = GrayImage::from_pixel(100, 400, image::Luma([0]));
        for y in 90..100 { for x in 0..100 { image.put_pixel(x, y, image::Luma([255])); } }
        let regions = strip_regions(&image, Viewport::new(100, 100)).unwrap();
        assert!(regions[0].rect.height >= 90 && regions[0].rect.height <= 100);
        assert!(!regions[1].overlap);
        assert!(regions.iter().skip(2).any(|region| region.overlap));
        assert_eq!(regions.last().unwrap().rect.y + regions.last().unwrap().rect.height, 400);
    }

    #[test]
    fn panel_heuristic_respects_a_full_width_and_column_gutter() {
        let mut image = GrayImage::from_pixel(200, 200, image::Luma([0]));
        for y in 95..105 { for x in 0..200 { image.put_pixel(x, y, image::Luma([255])); } }
        for y in 0..95 { for x in 95..105 { image.put_pixel(x, y, image::Luma([255])); } }
        let regions = panel_regions(&image, Viewport::new(100, 140), PageDirection::RightToLeft).unwrap();
        assert_eq!(regions.len(), 3);
        assert!(regions[0].rect.x > regions[1].rect.x);
        assert!(regions[2].rect.y > regions[0].rect.y);
    }

    #[test]
    fn end_transition_is_reversible_and_never_opens_another_episode() {
        let mut reader = ReaderCore::new(index(2), ReaderSettings::default()).unwrap();
        reader.set_page(1).unwrap();
        assert_eq!(reader.step(true), ReaderMove::EndOfEpisode);
        assert!(reader.anchor(1).finished);
        assert_eq!(reader.step(true), ReaderMove::EndOfEpisode);
        assert_eq!(reader.step(false), ReaderMove::Moved);
        assert_eq!(reader.page_index(), 1);
        assert!(!reader.is_transition());
    }

    #[test]
    fn quantization_only_emits_sixteen_panel_tones() {
        for value in 0..=255 { for y in 0..4 { for x in 0..4 {
            assert_eq!(quantize(value, x, y, true) % 17, 0);
        } } }
        assert!(Viewport::new(10_000, 10_000).validate().is_err());
        let (x, y) = unrotate_point(0.3, 0.2, 90);
        assert!((x - 0.2).abs() < 0.00001 && (y - 0.7).abs() < 0.00001);
    }

    #[test]
    fn a_long_narrow_source_is_not_rejected_by_a_thumbnail_edge_limit() {
        let image = GrayImage::from_pixel(8, 9000, image::Luma([128]));
        let mut encoded = Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(image).write_to(&mut encoded, ImageFormat::Png).unwrap();
        let decoded = decode_bounded_bytes(encoded.get_ref()).unwrap();
        assert_eq!(decoded.dimensions(), (8, 9000));
    }

    #[test]
    fn transparent_pixels_are_composited_over_paper_before_grayscale() {
        let image = RgbaImage::from_pixel(2, 1, Rgba([0, 0, 0, 0]));
        let mut encoded = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image).write_to(&mut encoded, ImageFormat::Png).unwrap();
        assert!(decode_bounded_bytes(encoded.get_ref()).unwrap().pixels().all(|pixel| pixel[0] == 255));
    }

    #[test]
    fn pan_within_an_overlapping_strip_keeps_the_current_region() {
        let regions: Vec<_> = [0, 36, 72].into_iter().map(|y| Region {
            rect: SourceRect { x: 0, y, width: 100, height: 100 }, overlap: y > 0,
        }).collect();
        assert_eq!(region_for_focus(&regions, 0, 90, Some(2)), 2);
        assert_eq!(region_for_focus(&regions, 0, 72, None), 2);
        assert_eq!(region_for_focus(&regions, 0, 90, None), 2);
    }

    #[test]
    fn width_fit_has_one_centered_source_view_and_does_not_skip_pages_as_a_spread() {
        let settings = ReaderSettings { fit: FitMode::Width, ..ReaderSettings::default() };
        let mut descriptor = index(4);
        for page in &mut descriptor.images { page.height = 400; }
        let mut reader = ReaderCore::new(descriptor, settings).unwrap();
        reader.set_landscape(true);
        let source = GrayImage::from_pixel(100, 400, image::Luma([0]));
        let frame = reader.render_source(&source, Viewport::new(120, 90)).unwrap();
        assert!(!reader.is_spread());
        assert_eq!(frame.crop_count, 1);
        assert!(frame.crop.y > 0);
        assert_eq!(reader.step(true), ReaderMove::Moved);
        assert_eq!(reader.page_index(), 1);
        assert_eq!(reader.crop_index(), 0);
    }

    #[test]
    fn explicit_fixture_panels_remain_separate_from_recognition() {
        let mut reader = ReaderCore::new(index(1), ReaderSettings::default()).unwrap();
        reader.set_panel_map("page-0", vec![NormalizedRect { x: 0.1, y: 0.2, width: 0.4, height: 0.3 }]).unwrap();
        let panel = reader.explicit_panels[&0][0];
        assert_eq!(panel.pixel_rect(100, 200, 0), SourceRect { x: 10, y: 40, width: 40, height: 60 });
        assert_eq!(panel.pixel_rect(200, 100, 90), SourceRect { x: 100, y: 10, width: 60, height: 40 });
        assert!(reader.set_panel_map("page-0", vec![NormalizedRect { x: 0.8, y: 0.2, width: 0.4, height: 0.3 }]).is_err());
    }
}

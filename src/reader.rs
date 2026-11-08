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
    sync::Arc,
};

use image::{GenericImageView, GrayImage, Rgba, RgbaImage};
#[cfg(test)]
use std::io::Cursor;
#[cfg(test)]
use image::ImageFormat;
use serde::{Deserialize, Serialize};

use crate::{domain::{ImageIndex, ReadingAnchor, ReadingMode}, image_integrity::ValidatedImage};

mod resources;
pub(crate) use resources::ReaderResources;

#[cfg(test)]
mod reuse_tests;

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
    #[default]
    Page,
    Width,
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
            footer: true,
            rotation: 0,
            zoom_ratio: 1.0,
        }
    }
}

impl ReaderSettings {
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
    restore_last_region: bool,
    transition: bool,
    landscape_override: Option<bool>,
    last_viewport: Option<Viewport>,
    layouts: BTreeMap<usize, PageLayout>,
    explicit_panels: BTreeMap<usize, Vec<NormalizedRect>>,
}

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
            restore_last_region: false,
            transition: false,
            landscape_override: None,
            last_viewport: None,
            layouts: BTreeMap::new(),
            explicit_panels: BTreeMap::new(),
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

    pub fn update_settings(&mut self, settings: ReaderSettings) -> Result<()> {
        settings.validate()?;
        // The serialized footer preference does not participate in source layout.
        self.settings.footer = settings.footer;
        if self.settings != settings {
            let reflow = self.settings.mode != settings.mode || self.settings.fit != settings.fit
                || self.settings.direction != settings.direction || self.settings.rotation != settings.rotation;
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
    pub fn pan_to(&mut self, source_x: f64, source_y: f64) -> Result<()> {
        validate_coordinates(source_x, source_y)?;
        self.source_x = source_x;
        self.source_y = source_y;
        self.restore_focus = true;
        self.preserve_region_for_pan = true;
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

    /// Exercise layouts directly with an immutable synthetic source in tests.
    #[cfg(test)]
    pub(crate) fn render_source(&mut self, source: &GrayImage, viewport: Viewport) -> Result<RenderedPage> {
        self.render_source_identity(source, viewport, None)
    }

    pub(crate) fn render_validated_source(&mut self, source: &ValidatedGraySource, viewport: Viewport) -> Result<RenderedPage> {
        self.render_source_identity(&source.pixels, viewport, Some(source.proof.sha256()))
    }

    fn render_source_identity(&mut self, source: &GrayImage, viewport: Viewport, identity: Option<&str>) -> Result<RenderedPage> {
        viewport.validate()?;
        if source.width() == 0 || source.height() == 0
            || u64::from(source.width()) * u64::from(source.height()) > MAX_SOURCE_PIXELS {
            return Err(ReaderError::SourceImageTooLarge);
        }
        let grayscale = match self.settings.rotation {
            90 => Cow::Owned(image::imageops::rotate90(source)),
            180 => Cow::Owned(image::imageops::rotate180(source)),
            270 => Cow::Owned(image::imageops::rotate270(source)),
            _ => Cow::Borrowed(source),
        };
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
                DisplayMode::Strip => (strip_regions(&grayscale, viewport)?, false),
                DisplayMode::Page if self.settings.fit == FitMode::Width => {
                    (strip_regions(&grayscale, viewport)?, false)
                }
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
        let crop = zoom_crop(region.rect, width, height, self.settings.zoom_ratio,
            rotate_point(self.source_x, self.source_y, self.settings.rotation));
        let crop_count = layout.regions.len();
        let heuristic = layout.heuristic;
        let crop_image = image::imageops::crop_imm(grayscale.as_ref(), crop.x, crop.y, crop.width, crop.height);
        let ratio = (f64::from(viewport.width) / f64::from(crop.width))
            .min(f64::from(viewport.height) / f64::from(crop.height));
        let target_width = (f64::from(crop.width) * ratio).round().clamp(1.0, f64::from(viewport.width)) as u32;
        let target_height = (f64::from(crop.height) * ratio).round().clamp(1.0, f64::from(viewport.height)) as u32;
        let resized = image::imageops::resize(&*crop_image, target_width, target_height, image::imageops::FilterType::Triangle);
        drop(grayscale);
        let mut canvas = RgbaImage::from_pixel(viewport.width, viewport.height, Rgba([255, 255, 255, 255]));
        let left = (viewport.width - target_width) / 2;
        let top = if self.settings.fit == FitMode::Width || self.settings.mode == DisplayMode::Strip {
            0
        } else { (viewport.height - target_height) / 2 };
        let mut dark_pixels = 0u64;
        for (x, y, pixel) in resized.enumerate_pixels() {
            let tone = quantize(pixel[0], x, y, self.settings.dither);
            if tone <= 68 { dark_pixels += 1; }
            canvas.put_pixel(left + x, top + y, Rgba([tone, tone, tone, 255]));
        }
        let (source_x, source_y) = unrotate_point(
            f64::from(crop.x) / f64::from(width), f64::from(crop.y) / f64::from(height), self.settings.rotation);
        self.source_x = source_x;
        self.source_y = source_y;
        Ok(RenderedPage {
            rgba: canvas.into_raw(), width: viewport.width, height: viewport.height,
            source_width: width, source_height: height, crop, page_index: self.page_index,
            crop_index: self.crop_index, crop_count, overlap_from_previous: region.overlap,
            heuristic_panels: heuristic,
            dark_fraction: dark_pixels as f32 / (u64::from(viewport.width) * u64::from(viewport.height)) as f32,
        })
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
    let encoded = read_source_bytes(path)?;
    decode_source_bytes(&encoded, resources, None)?.ok_or(ReaderError::InvalidDescriptor)
}

pub(crate) fn load_changed_source(path: &Path, previous: Option<&ValidatedImage>) -> Result<Option<ValidatedGraySource>> {
    let encoded = read_source_bytes(path)?;
    decode_source_bytes(&encoded, ReaderResources::shared(), previous)
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
    if encoded.len() as u64 > MAX_ENCODED_BYTES { return Err(ReaderError::EncodedImageTooLarge); }
    let (proof, decoded) = crate::image_integrity::decode_image_with_reuse(encoded, None,
        |digest| {
            if let Some(proof) = previous.filter(|proof| proof.sha256() == digest) {
                Some((proof.clone(), None))
            } else { resources.lookup_digest(digest).map(|(proof, pixels)| (proof, Some(pixels))) }
        }).map_err(|error| {
        if let Some(evidence) = error.encoding_evidence() { resources.remove_digest(evidence.sha256()); }
        match error.kind {
            crate::image_integrity::IntegrityFailureKind::ResourceLimit => ReaderError::SourceImageTooLarge,
            crate::image_integrity::IntegrityFailureKind::Unsupported => ReaderError::UnsupportedFormat,
            crate::image_integrity::IntegrityFailureKind::Corrupt => ReaderError::CorruptImage,
        }
    })?;
    let decoded = match decoded {
        crate::image_integrity::ReusableImage::Decoded(decoded) => decoded,
        crate::image_integrity::ReusableImage::Reused(Some(pixels)) => return Ok(Some(ValidatedGraySource { proof, pixels })),
        crate::image_integrity::ReusableImage::Reused(None) => return Ok(None),
    };
    let (width, height) = decoded.dimensions();
    let mut grayscale = GrayImage::new(width, height);
    for (x, y, pixel) in decoded.pixels() {
        let alpha = u32::from(pixel[3]);
        let luma = (54 * u32::from(pixel[0]) + 183 * u32::from(pixel[1]) + 19 * u32::from(pixel[2]) + 128) / 256;
        let composite = ((luma * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
        grayscale.put_pixel(x, y, image::Luma([composite]));
    }
    let grayscale = Arc::new(grayscale);
    resources.insert_source(&proof, grayscale.clone());
    Ok(Some(ValidatedGraySource { proof, pixels: grayscale }))
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

fn zoom_crop(region: SourceRect, source_width: u32, source_height: u32, zoom: f64, focus: (f64, f64)) -> SourceRect {
    if zoom <= 1.0 { return region; }
    let width = (f64::from(region.width) / zoom).round().max(1.0) as u32;
    let height = (f64::from(region.height) / zoom).round().max(1.0) as u32;
    let x = (focus.0 * f64::from(source_width)).round() as u32;
    let y = (focus.1 * f64::from(source_height)).round() as u32;
    SourceRect {
        x: x.clamp(region.x, region.x + region.width - width),
        y: y.clamp(region.y, region.y + region.height - height),
        width, height,
    }
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
    fn returning_to_the_last_page_starts_at_its_first_panel_or_width_view() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("page.png");
        GrayImage::from_pixel(100, 400, image::Luma([0])).save(&path).unwrap();
        for settings in [
            ReaderSettings { mode: DisplayMode::Panel, ..Default::default() },
            ReaderSettings { fit: FitMode::Width, ..Default::default() },
        ] {
            let mut descriptor = index(2);
            for page in &mut descriptor.images { page.height = 400; }
            let mut reader = ReaderCore::new(descriptor, settings).unwrap();
            reader.set_page(1).unwrap();
            let frame = reader.render(&path, Viewport::new(100, 100)).unwrap();
            assert!(frame.crop_count > 1);
            while reader.step(true) == ReaderMove::Moved {}
            assert!(reader.is_transition());

            reader.back_to_last_view().unwrap();
            assert_eq!(reader.page_index(), 1);
            assert_eq!(reader.crop_index(), 0);
            let frame = reader.render(&path, Viewport::new(100, 100)).unwrap();
            assert_eq!(frame.crop_index, 0);
            assert_eq!(frame.crop.y, 0);
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
    fn width_fit_cannot_skip_strip_regions_as_a_spread() {
        let settings = ReaderSettings { fit: FitMode::Width, ..ReaderSettings::default() };
        let mut reader = ReaderCore::new(index(4), settings).unwrap();
        reader.set_landscape(true);
        reader.layouts.insert(0, PageLayout {
            viewport: Viewport::new(1200, 900), width: 100, height: 200, heuristic: false, source_identity: None,
            regions: vec![
                Region { rect: SourceRect { x: 0, y: 0, width: 100, height: 100 }, overlap: false },
                Region { rect: SourceRect { x: 0, y: 100, width: 100, height: 100 }, overlap: false },
            ],
        });
        assert!(!reader.is_spread());
        assert_eq!(reader.step(true), ReaderMove::Moved);
        assert_eq!(reader.page_index(), 0);
        assert_eq!(reader.crop_index(), 1);
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

//! Shared, bounded resources for local image preparation.
//!
//! The source cache retains only immutable, unrotated grayscale pixels. Its byte
//! budget counts the capacity of pixel buffers owned by cache entries, once per
//! entry. An evicted buffer may remain alive through an in-flight caller's Arc;
//! the application's shared compute admission bounds those callers separately.
//! Entry metadata has an additional count limit and is not included in the pixel
//! byte total.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard, OnceLock},
};

use image::GrayImage;

use super::{CPU_RESIZE_VERSION, CpuResizeKey, PROCESSED_SOURCE_VERSION, ProcessedSourceKey};

use crate::image_integrity::{
    MAX_ENCODED_BYTES, MAX_SOURCE_PIXELS, VALIDATION_VERSION, ValidatedImage,
};

pub(crate) const DEFAULT_SOURCE_CACHE_BYTES: usize = 32 * 1024 * 1024;
const DEFAULT_PROCESSED_CACHE_BYTES: usize = 16 * 1024 * 1024;
const MAX_SOURCE_CACHE_ENTRIES: usize = 128;
const MAX_PROCESSED_CACHE_ENTRIES: usize = 16;

struct SourceEntry {
    proof: ValidatedImage,
    source: Arc<GrayImage>,
    bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SourceCacheUsage {
    pub(crate) entries: usize,
    pub(crate) retained_bytes: usize,
    pub(crate) byte_budget: usize,
}

/// A byte-bounded LRU keyed by the exact validated encoded content identity.
pub(crate) struct GraySourceCache {
    entries: VecDeque<SourceEntry>,
    retained_bytes: usize,
    byte_budget: usize,
}

impl GraySourceCache {
    /// A zero budget disables retention without preventing the caller's decode.
    pub(crate) fn new(byte_budget: usize) -> Self {
        Self { entries: VecDeque::new(), retained_bytes: 0, byte_budget }
    }

    pub(crate) fn byte_budget(&self) -> usize { self.byte_budget }
    pub(crate) fn retained_bytes(&self) -> usize { self.retained_bytes }
    pub(crate) fn len(&self) -> usize { self.entries.len() }

    pub(crate) fn usage(&self) -> SourceCacheUsage {
        SourceCacheUsage { entries: self.len(), retained_bytes: self.retained_bytes(), byte_budget: self.byte_budget() }
    }

    /// A caller must supply a sealed proof obtained from the current input.
    /// A hit never substitutes a path, service page identifier, or bare digest
    /// for the proof's encoded length, validation version, and source geometry.
    #[cfg(test)]
    pub(crate) fn cached_source(&mut self, proof: &ValidatedImage) -> Option<Arc<GrayImage>> {
        let position = self.position(proof)?;
        let entry = self.entries.get(position)?;
        if entry.proof != *proof || !valid_source(proof, &entry.source) {
            self.remove_at(position);
            return None;
        }
        let entry = self.entries.remove(position)?;
        let source = entry.source.clone();
        self.entries.push_back(entry);
        Some(source)
    }

    /// Return the original sealed proof with pixels for a digest computed from
    /// the current input. The integrity layer must still check that input's
    /// encoded length and requested policy before accepting this reuse result.
    pub(crate) fn lookup_digest(&mut self, digest: &str) -> Option<(ValidatedImage, Arc<GrayImage>)> {
        let position = self.entries.iter().rposition(|entry| entry.proof.sha256() == digest)?;
        let entry = self.entries.get(position)?;
        if !valid_source(&entry.proof, &entry.source) {
            self.remove_at(position);
            return None;
        }
        let entry = self.entries.remove(position)?;
        let result = (entry.proof.clone(), entry.source.clone());
        self.entries.push_back(entry);
        Some(result)
    }

    /// Retain grayscale pixels derived from this proof's decoded source, before
    /// rotation or cropping. The caller is responsible for that derivation;
    /// this layer checks identity metadata, dimensions, and allocation budgets.
    /// False means the cache did not retain the supplied Arc. In particular,
    /// an otherwise valid oversized source remains usable by its caller.
    pub(crate) fn insert(&mut self, proof: &ValidatedImage, source: Arc<GrayImage>) -> bool {
        // Replace an identity exactly once, including when its new value is
        // invalid or too large to retain. Never keep a stale duplicate behind.
        self.remove(proof);
        if !valid_source(proof, &source) { return false; }
        let bytes = source.as_raw().capacity();
        if bytes > self.byte_budget { return false; }
        while bytes > self.byte_budget - self.retained_bytes
            || self.entries.len() >= MAX_SOURCE_CACHE_ENTRIES
        {
            let Some(entry) = self.entries.pop_front() else { return false; };
            self.retained_bytes -= entry.bytes;
        }
        self.retained_bytes += bytes;
        self.entries.push_back(SourceEntry { proof: proof.clone(), source, bytes });
        true
    }

    pub(crate) fn remove(&mut self, proof: &ValidatedImage) -> bool {
        let Some(position) = self.position(proof) else { return false; };
        self.remove_at(position);
        true
    }

    /// Remove every encoded-length variant when validation rejects a digest.
    pub(crate) fn remove_digest(&mut self, digest: &str) -> usize {
        let mut removed = 0;
        let mut position = 0;
        while position < self.entries.len() {
            if self.entries[position].proof.sha256() == digest {
                self.remove_at(position);
                removed += 1;
            } else { position += 1; }
        }
        removed
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.retained_bytes = 0;
    }

    fn position(&self, proof: &ValidatedImage) -> Option<usize> {
        self.entries.iter().position(|entry| entry.proof.sha256() == proof.sha256()
            && entry.proof.byte_len() == proof.byte_len())
    }

    fn remove_at(&mut self, position: usize) {
        if let Some(entry) = self.entries.remove(position) { self.retained_bytes -= entry.bytes; }
    }
}

fn valid_source(proof: &ValidatedImage, source: &GrayImage) -> bool {
    let pixels = u64::from(proof.width()) * u64::from(proof.height());
    proof.validation_version() == VALIDATION_VERSION
        && proof.byte_len() > 0 && proof.byte_len() <= MAX_ENCODED_BYTES
        && proof.width() > 0 && proof.height() > 0 && pixels <= MAX_SOURCE_PIXELS
        && source.width() == proof.width() && source.height() == proof.height()
        && u64::try_from(source.as_raw().len()).ok() == Some(pixels)
}

#[derive(Clone, Debug, PartialEq)]
enum ProcessedCacheKey {
    Native(ProcessedSourceKey),
    CpuTriangle(CpuResizeKey),
}

impl ProcessedCacheKey {
    fn proof(&self) -> &ValidatedImage {
        match self {
            Self::Native(key) => &key.proof,
            Self::CpuTriangle(key) => &key.proof,
        }
    }

    fn valid_source(&self, source: &GrayImage) -> bool {
        match self {
            Self::Native(key) => valid_processed_source(key, source),
            Self::CpuTriangle(key) => valid_cpu_resized_source(key, source),
        }
    }
}

struct ProcessedEntry {
    key: ProcessedCacheKey,
    source: Arc<GrayImage>,
    bytes: usize,
}

/// A separate bounded LRU with distinct native and CPU preparation identities.
/// Resized pixels never enter the canonical source cache.
struct ProcessedSourceCache {
    entries: VecDeque<ProcessedEntry>,
    retained_bytes: usize,
    byte_budget: usize,
}

impl ProcessedSourceCache {
    fn new(byte_budget: usize) -> Self {
        Self { entries: VecDeque::new(), retained_bytes: 0, byte_budget }
    }

    fn lookup(&mut self, matches: impl Fn(&ProcessedCacheKey) -> bool) -> Option<Arc<GrayImage>> {
        let position = self.entries.iter().position(|entry| matches(&entry.key))?;
        let entry = self.entries.remove(position)?;
        self.retained_bytes -= entry.bytes;
        if !entry.key.valid_source(&entry.source) { return None; }
        let pixels = entry.source.clone();
        self.retained_bytes += entry.bytes;
        self.entries.push_back(entry);
        Some(pixels)
    }

    fn insert(&mut self, key: ProcessedCacheKey, source: Arc<GrayImage>) -> bool {
        if let Some(position) = self.entries.iter().position(|entry| entry.key == key) {
            if let Some(entry) = self.entries.remove(position) { self.retained_bytes -= entry.bytes; }
        }
        if !key.valid_source(&source) { return false; }
        let bytes = source.as_raw().capacity();
        if bytes > self.byte_budget { return false; }
        while bytes > self.byte_budget - self.retained_bytes
            || self.entries.len() >= MAX_PROCESSED_CACHE_ENTRIES
        {
            let Some(entry) = self.entries.pop_front() else { return false; };
            self.retained_bytes -= entry.bytes;
        }
        self.retained_bytes += bytes;
        self.entries.push_back(ProcessedEntry { key, source, bytes });
        true
    }

    fn remove_digest(&mut self, digest: &str) -> usize {
        let mut removed = 0;
        self.entries.retain(|entry| {
            if entry.key.proof().sha256() == digest {
                self.retained_bytes -= entry.bytes;
                removed += 1;
                false
            } else { true }
        });
        removed
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.retained_bytes = 0;
    }

    fn usage(&self) -> SourceCacheUsage {
        SourceCacheUsage { entries: self.entries.len(), retained_bytes: self.retained_bytes,
            byte_budget: self.byte_budget }
    }
}

fn valid_processed_source(key: &ProcessedSourceKey, source: &GrayImage) -> bool {
    let pixels = u64::from(key.geometry.width) * u64::from(key.geometry.height);
    key.version == PROCESSED_SOURCE_VERSION
        && key.proof.validation_version() == VALIDATION_VERSION
        && key.proof.format == image::ImageFormat::Jpeg
        && key.proof.byte_len() > 0 && key.proof.byte_len() <= MAX_ENCODED_BYTES
        && key.preparation.geometry(key.proof.width(), key.proof.height()) == Some(key.geometry)
        && crate::native_media::qualified_geometry((key.proof.width(), key.proof.height()),
            (key.geometry.width, key.geometry.height))
        && source.dimensions() == (key.geometry.width, key.geometry.height)
        && u64::try_from(source.as_raw().len()).ok() == Some(pixels)
}

fn valid_cpu_resized_source(key: &CpuResizeKey, source: &GrayImage) -> bool {
    let proof = &key.proof;
    let preparation = key.preparation;
    let source_pixels = u64::from(proof.width()) * u64::from(proof.height());
    let target_pixels = u64::from(key.geometry.width) * u64::from(key.geometry.height);
    key.version == CPU_RESIZE_VERSION
        && proof.validation_version() == VALIDATION_VERSION
        && proof.byte_len() > 0 && proof.byte_len() <= MAX_ENCODED_BYTES
        && proof.width() > 0 && proof.height() > 0 && source_pixels <= MAX_SOURCE_PIXELS
        && preparation.mode == super::DisplayMode::Page
        && preparation.fit == super::FitMode::Page
        && preparation.rotation == 0 && preparation.zoom_ratio == 1.0
        && preparation.viewport.validate().is_ok()
        && key.geometry.crop == super::SourceRect::whole(proof.width(), proof.height())
        && key.geometry.sample.is_none()
        && (key.geometry.width, key.geometry.height)
            == (preparation.viewport.width, preparation.viewport.height)
        && (key.geometry.width, key.geometry.height) != (proof.width(), proof.height())
        && key.geometry == super::fit_geometry(key.geometry.crop, preparation.viewport,
            super::FitMode::Page, 1.0, None)
        && source.dimensions() == (key.geometry.width, key.geometry.height)
        && u64::try_from(source.as_raw().len()).ok() == Some(target_pixels)
}

pub(crate) struct ReaderResources {
    sources: Mutex<GraySourceCache>,
    processed: Mutex<ProcessedSourceCache>,
}

impl ReaderResources {
    pub(crate) fn new(source_cache_bytes: usize) -> Self {
        Self { sources: Mutex::new(GraySourceCache::new(source_cache_bytes)),
            processed: Mutex::new(ProcessedSourceCache::new(source_cache_bytes.min(DEFAULT_PROCESSED_CACHE_BYTES))) }
    }

    pub(crate) fn shared() -> &'static Self {
        static RESOURCES: OnceLock<ReaderResources> = OnceLock::new();
        RESOURCES.get_or_init(|| Self::new(DEFAULT_SOURCE_CACHE_BYTES))
    }

    #[cfg(test)]
    pub(crate) fn cached_source(&self, proof: &ValidatedImage) -> Option<Arc<GrayImage>> {
        self.lock_sources().cached_source(proof)
    }

    pub(crate) fn lookup_digest(&self, digest: &str) -> Option<(ValidatedImage, Arc<GrayImage>)> {
        self.lock_sources().lookup_digest(digest)
    }

    pub(crate) fn insert_source(&self, proof: &ValidatedImage, source: Arc<GrayImage>) -> bool {
        self.lock_sources().insert(proof, source)
    }

    pub(crate) fn remove_digest(&self, digest: &str) -> usize {
        let sources = self.lock_sources().remove_digest(digest);
        sources + self.lock_processed().remove_digest(digest)
    }

    pub(super) fn processed_source(&self, key: &ProcessedSourceKey) -> Option<Arc<GrayImage>> {
        self.lock_processed().lookup(|entry| matches!(entry,
            ProcessedCacheKey::Native(retained) if retained == key))
    }

    pub(super) fn insert_processed_source(&self, key: ProcessedSourceKey, source: Arc<GrayImage>) -> bool {
        self.lock_processed().insert(ProcessedCacheKey::Native(key), source)
    }

    /// Retain only complete Triangle output before final-frame pixel processing.
    pub(super) fn cpu_resized_source(&self, key: &CpuResizeKey) -> Option<Arc<GrayImage>> {
        self.lock_processed().lookup(|entry| matches!(entry,
            ProcessedCacheKey::CpuTriangle(retained) if retained == key))
    }

    pub(super) fn insert_cpu_resized_source(&self, key: CpuResizeKey, source: Arc<GrayImage>) -> bool {
        self.lock_processed().insert(ProcessedCacheKey::CpuTriangle(key), source)
    }

    #[cfg(test)]
    pub(crate) fn clear_sources(&self) { self.lock_sources().clear(); }
    pub(crate) fn cache_usage(&self) -> SourceCacheUsage { self.lock_sources().usage() }
    pub(crate) fn processed_cache_usage(&self) -> SourceCacheUsage { self.lock_processed().usage() }

    fn lock_processed(&self) -> MutexGuard<'_, ProcessedSourceCache> {
        match self.processed.lock() {
            Ok(cache) => cache,
            Err(error) => {
                let mut cache = error.into_inner();
                cache.clear();
                self.processed.clear_poison();
                cache
            }
        }
    }

    fn lock_sources(&self) -> MutexGuard<'_, GraySourceCache> {
        match self.sources.lock() {
            Ok(cache) => cache,
            Err(error) => {
                // A failed mutation must not leave uncertain byte accounting.
                let mut cache = error.into_inner();
                cache.clear();
                self.sources.clear_poison();
                cache
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{DynamicImage, ImageFormat, Luma};

    use super::*;
    use super::super::{CenteredSample, DisplayMode, FitMode, SourcePreparation, SourceRect, Viewport};

    fn encoded_source(width: u32, height: u32, tone: u8) -> Vec<u8> {
        let source = GrayImage::from_fn(width, height, |x, y| Luma([tone.wrapping_add((x + y) as u8)]));
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageLuma8(source).write_to(&mut encoded, ImageFormat::Png)
            .expect("Encode a complete synthetic source");
        encoded.into_inner()
    }

    fn decoded_source(encoded: &[u8]) -> (ValidatedImage, Arc<GrayImage>) {
        let (proof, decoded) = crate::image_integrity::decode_image(encoded, None)
            .expect("Validate the complete synthetic source");
        (proof, Arc::new(decoded.into_luma8()))
    }

    fn source(width: u32, height: u32, tone: u8) -> (ValidatedImage, Arc<GrayImage>) {
        decoded_source(&encoded_source(width, height, tone))
    }

    fn cpu_key(proof: &ValidatedImage, viewport: Viewport) -> CpuResizeKey {
        let preparation = SourcePreparation { mode: DisplayMode::Page, fit: FitMode::Page,
            rotation: 0, zoom_ratio: 1.0, viewport,
            media_acceleration: crate::native_media::Acceleration::Off };
        CpuResizeKey { proof: proof.clone(), preparation,
            geometry: super::super::fit_geometry(SourceRect::whole(proof.width(), proof.height()),
                viewport, FitMode::Page, 1.0, None), version: CPU_RESIZE_VERSION }
    }

    fn native_and_cpu_keys() -> (ProcessedSourceKey, CpuResizeKey, Arc<GrayImage>) {
        let source = GrayImage::from_pixel(1920, 2560, Luma([102]));
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageLuma8(source).write_to(&mut encoded, ImageFormat::Jpeg)
            .expect("Encode a complete qualified synthetic JPEG");
        let (proof, source) = decoded_source(&encoded.into_inner());
        let mut cpu = cpu_key(&proof, Viewport::new(1860, 2480));
        cpu.preparation.media_acceleration = crate::native_media::Acceleration::DecodeAndResize;
        let native = ProcessedSourceKey { proof, preparation: cpu.preparation,
            geometry: cpu.geometry, version: PROCESSED_SOURCE_VERSION };
        (native, cpu, source)
    }

    #[test]
    fn processed_namespaces_share_budget_without_cross_hits_and_invalidate_together() {
        let (native, cpu, original) = native_and_cpu_keys();
        let native_pixels = Arc::new(GrayImage::from_pixel(1860, 2480, Luma([51])));
        let cpu_pixels = Arc::new(GrayImage::from_pixel(1860, 2480, Luma([119])));
        let budget = native_pixels.as_raw().capacity() + cpu_pixels.as_raw().capacity();
        let resources = ReaderResources::new(budget);
        assert!(resources.insert_processed_source(native.clone(), native_pixels.clone()));
        assert!(resources.cpu_resized_source(&cpu).is_none());
        assert!(resources.insert_cpu_resized_source(cpu.clone(), cpu_pixels.clone()));
        assert!(Arc::ptr_eq(&resources.processed_source(&native).unwrap(), &native_pixels));
        assert!(Arc::ptr_eq(&resources.cpu_resized_source(&cpu).unwrap(), &cpu_pixels));
        assert_eq!(resources.processed_cache_usage(), SourceCacheUsage {
            entries: 2, retained_bytes: budget, byte_budget: budget });
        assert!(resources.cached_source(&native.proof).is_none());

        let mut other_policy = cpu.clone();
        other_policy.preparation.media_acceleration = crate::native_media::Acceleration::Off;
        assert!(resources.insert_cpu_resized_source(other_policy.clone(), cpu_pixels));
        assert!(resources.processed_source(&native).is_none());
        assert!(resources.cpu_resized_source(&cpu).is_some());
        assert!(resources.cpu_resized_source(&other_policy).is_some());
        assert_eq!(resources.processed_cache_usage().retained_bytes, budget);
        assert!(resources.insert_processed_source(native.clone(), native_pixels));
        assert!(resources.cpu_resized_source(&cpu).is_none());
        assert!(resources.cpu_resized_source(&other_policy).is_some());
        assert!(resources.insert_source(&native.proof, original));
        assert_eq!(resources.remove_digest(native.proof.sha256()), 3);
        assert!(resources.processed_source(&native).is_none());
        assert!(resources.cpu_resized_source(&other_policy).is_none());
        assert!(resources.cached_source(&native.proof).is_none());
        assert_eq!(resources.processed_cache_usage().retained_bytes, 0);
    }

    #[test]
    fn cpu_resizes_require_complete_proofs_and_the_exact_preparation_identity() {
        let (first, _) = source(8, 8, 17);
        let (second, _) = source(8, 8, 34);
        let key = cpu_key(&first, Viewport::new(4, 4));
        let pixels = Arc::new(GrayImage::from_pixel(4, 4, Luma([85])));
        let resources = ReaderResources::new(1024);
        assert!(resources.insert_cpu_resized_source(key.clone(), pixels.clone()));
        assert!(Arc::ptr_eq(&resources.cpu_resized_source(&key).unwrap(), &pixels));
        let mut other = key.clone();
        other.proof = second;
        assert!(resources.cpu_resized_source(&other).is_none());
        other = key.clone();
        other.preparation.media_acceleration = crate::native_media::Acceleration::DecodeAndResize;
        assert!(resources.cpu_resized_source(&other).is_none());
        other = key.clone();
        other.version += 1;
        assert!(resources.cpu_resized_source(&other).is_none());
        assert!(!resources.insert_cpu_resized_source(other, pixels.clone()));
        let native = ProcessedSourceKey { proof: first, preparation: key.preparation,
            geometry: key.geometry, version: PROCESSED_SOURCE_VERSION };
        assert!(!resources.insert_processed_source(native, pixels));
        assert_eq!(resources.processed_cache_usage().entries, 1);
    }

    #[test]
    fn cpu_cache_rejects_non_page_geometry_and_partial_outputs() {
        let (proof, _) = source(8, 8, 17);
        let key = cpu_key(&proof, Viewport::new(4, 4));
        let pixels = Arc::new(GrayImage::from_pixel(4, 4, Luma([85])));
        let invalid: &[fn(&mut CpuResizeKey)] = &[
            |key| key.preparation.mode = DisplayMode::Panel,
            |key| key.preparation.mode = DisplayMode::Strip,
            |key| key.preparation.fit = FitMode::Original,
            |key| key.preparation.fit = FitMode::Height,
            |key| key.preparation.fit = FitMode::Width,
            |key| key.preparation.rotation = 90,
            |key| key.preparation.zoom_ratio = 2.0,
            |key| key.preparation.zoom_ratio = f64::NAN,
            |key| key.preparation.viewport.width = 0,
            |key| key.preparation.viewport.scale_factor = f32::NAN,
            |key| key.geometry.crop.x = 1,
            |key| key.geometry.crop.width = 7,
            |key| key.geometry.width = 3,
            |key| key.geometry.height = 3,
            |key| key.geometry.origin_x = 1.0,
            |key| key.geometry.origin_y = 1.0,
            |key| key.geometry.sample = Some(CenteredSample { bounds: key.geometry.crop,
                center_x: 4.0, center_y: 4.0, scale: 0.5 }),
        ];
        for mutate in invalid {
            let resources = ReaderResources::new(1024);
            let mut invalid = key.clone();
            mutate(&mut invalid);
            assert!(!resources.insert_cpu_resized_source(invalid, pixels.clone()));
            assert_eq!(resources.processed_cache_usage().entries, 0);
        }
        let resources = ReaderResources::new(1024);
        assert!(!resources.insert_cpu_resized_source(key.clone(),
            Arc::new(GrayImage::from_pixel(4, 3, Luma([85])))));
        let extra_storage = Arc::new(GrayImage::from_raw(4, 4, vec![85; 17]).unwrap());
        assert!(!resources.insert_cpu_resized_source(key, extra_storage));
        assert!(!resources.insert_cpu_resized_source(cpu_key(&proof, Viewport::new(8, 8)),
            Arc::new(GrayImage::from_pixel(8, 8, Luma([85])))));
        assert_eq!(resources.processed_cache_usage().entries, 0);
    }

    #[test]
    fn cpu_cache_counts_owned_capacity_and_preserves_zero_budget_behavior() {
        let (proof, _) = source(8, 8, 17);
        let key = cpu_key(&proof, Viewport::new(4, 4));
        let mut allocation = Vec::with_capacity(80);
        allocation.extend_from_slice(&[85; 16]);
        let pixels = Arc::new(GrayImage::from_raw(4, 4, allocation).unwrap());
        for budget in [0, 16] {
            let resources = ReaderResources::new(budget);
            assert!(!resources.insert_cpu_resized_source(key.clone(), pixels.clone()));
            assert!(resources.cpu_resized_source(&key).is_none());
            assert_eq!(resources.processed_cache_usage(), SourceCacheUsage {
                entries: 0, retained_bytes: 0, byte_budget: budget });
        }
        assert_eq!(Arc::strong_count(&pixels), 1);
        assert_eq!(pixels.as_raw().len(), 16);
    }

    #[test]
    fn cache_eviction_tracks_byte_capacity_and_actual_access_order() {
        let (first, first_source) = source(4, 4, 17);
        let (second, second_source) = source(4, 4, 68);
        let (third, third_source) = source(4, 4, 136);
        let budget = first_source.as_raw().capacity() + second_source.as_raw().capacity();
        let mut cache = GraySourceCache::new(budget);
        assert!(cache.insert(&first, first_source.clone()));
        assert!(cache.insert(&second, second_source));
        assert!(Arc::ptr_eq(&cache.cached_source(&first).unwrap(), &first_source));
        assert!(cache.insert(&third, third_source.clone()));
        assert!(cache.cached_source(&second).is_none());
        assert!(cache.cached_source(&first).is_some());
        assert!(Arc::ptr_eq(&cache.cached_source(&third).unwrap(), &third_source));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.retained_bytes(), first_source.as_raw().capacity() + third_source.as_raw().capacity());
        assert!(cache.retained_bytes() <= cache.byte_budget());
    }

    #[test]
    fn oversized_allocations_bypass_retention_without_discarding_the_caller_source() {
        let (proof, source) = source(4, 4, 17);
        let pixel_bytes = source.as_raw().len();
        let mut allocation = Vec::with_capacity(pixel_bytes + 64);
        allocation.extend_from_slice(source.as_raw());
        let source = Arc::new(GrayImage::from_raw(proof.width(), proof.height(), allocation).unwrap());
        let mut cache = GraySourceCache::new(pixel_bytes);
        assert!(!cache.insert(&proof, source.clone()));
        assert_eq!(source.as_raw().len(), pixel_bytes);
        assert_eq!(Arc::strong_count(&source), 1);
        assert_eq!(cache.usage(), SourceCacheUsage { entries: 0, retained_bytes: 0, byte_budget: pixel_bytes });
        assert!(cache.cached_source(&proof).is_none());
    }

    #[test]
    fn duplicate_replacement_and_invalid_geometry_cannot_inflate_the_cache() {
        let (proof, source) = source(4, 4, 17);
        let mut cache = GraySourceCache::new(source.as_raw().capacity());
        assert!(cache.insert(&proof, source.clone()));
        let replacement = Arc::new(source.as_ref().clone());
        assert!(cache.insert(&proof, replacement.clone()));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.retained_bytes(), replacement.as_raw().capacity());
        assert!(Arc::ptr_eq(&cache.cached_source(&proof).unwrap(), &replacement));
        assert!(!cache.insert(&proof, Arc::new(GrayImage::new(1, 1))));
        assert_eq!(cache.retained_bytes(), 0);
        assert!(cache.cached_source(&proof).is_none());
    }

    #[test]
    fn identical_pixels_with_different_encoded_content_require_distinct_proofs() {
        let encoded = encoded_source(4, 4, 17);
        let (first, first_source) = decoded_source(&encoded);
        let split = encoded.len() - 12;
        let mut variant = encoded[..split].to_vec();
        let payload = b"fixture\0distinct-encoded-identity";
        variant.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        variant.extend_from_slice(b"tEXt");
        variant.extend_from_slice(payload);
        let mut checksum = crc32fast::Hasher::new();
        checksum.update(b"tEXt"); checksum.update(payload);
        variant.extend_from_slice(&checksum.finalize().to_be_bytes());
        variant.extend_from_slice(&encoded[split..]);
        let (second, second_source) = decoded_source(&variant);
        assert_ne!(first.sha256(), second.sha256());
        assert_ne!(first.byte_len(), second.byte_len());
        assert_eq!(first_source.as_raw(), second_source.as_raw());
        let budget = first_source.as_raw().capacity() + second_source.as_raw().capacity();
        let mut cache = GraySourceCache::new(budget);
        assert!(cache.insert(&first, first_source));
        assert!(cache.cached_source(&second).is_none());
        assert!(cache.insert(&second, second_source));
        assert!(cache.remove(&first));
        assert!(!cache.remove(&first));
        assert!(cache.cached_source(&second).is_some());
        let (retained_proof, retained_source) = cache.lookup_digest(second.sha256()).unwrap();
        assert_eq!(retained_proof, second);
        assert_eq!(retained_source.dimensions(), (second.width(), second.height()));
        assert!(cache.lookup_digest(first.sha256()).is_none());
        assert_eq!(cache.remove_digest(second.sha256()), 1);
        assert_eq!(cache.usage().entries, 0);
        assert_eq!(cache.retained_bytes(), 0);
    }

    #[test]
    fn clearing_counts_only_cache_ownership_while_in_flight_arcs_remain_valid() {
        let (proof, source) = source(4, 4, 17);
        let resources = ReaderResources::new(source.as_raw().capacity());
        let weak = Arc::downgrade(&source);
        assert!(resources.insert_source(&proof, source.clone()));
        let in_flight = resources.cached_source(&proof).unwrap();
        drop(source);
        resources.clear_sources();
        assert_eq!(resources.cache_usage().retained_bytes, 0);
        assert!(weak.upgrade().is_some());
        assert_eq!(in_flight.dimensions(), (proof.width(), proof.height()));
        drop(in_flight);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn a_zero_cache_budget_does_not_retain_sources() {
        let (proof, source) = source(4, 4, 17);
        let resources = ReaderResources::new(0);
        assert!(!resources.insert_source(&proof, source));
        assert_eq!(resources.cache_usage().entries, 0);
        assert!(resources.lookup_digest(proof.sha256()).is_none());
    }
}

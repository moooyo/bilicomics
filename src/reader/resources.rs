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

use crate::image_integrity::{
    MAX_ENCODED_BYTES, MAX_SOURCE_PIXELS, VALIDATION_VERSION, ValidatedImage,
};

pub(crate) const DEFAULT_SOURCE_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_SOURCE_CACHE_ENTRIES: usize = 128;

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

pub(crate) struct ReaderResources {
    sources: Mutex<GraySourceCache>,
}

impl ReaderResources {
    pub(crate) fn new(source_cache_bytes: usize) -> Self {
        Self { sources: Mutex::new(GraySourceCache::new(source_cache_bytes)) }
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
        self.lock_sources().remove_digest(digest)
    }

    #[cfg(test)]
    pub(crate) fn clear_sources(&self) { self.lock_sources().clear(); }
    pub(crate) fn cache_usage(&self) -> SourceCacheUsage { self.lock_sources().usage() }

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

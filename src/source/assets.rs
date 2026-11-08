//! Reuse only immutable, digest-pinned signing bytes.
//!
//! File metadata is an invalidation hint, never a substitute for validating new
//! bytes. An in-flight user owns its snapshot even when the path changes later.

use std::{
    fs::{self, File, Metadata},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}},
    time::SystemTime,
};

use sha2::{Digest, Sha256};

use super::{Result, SourceError};

const MAX_ASSET_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    length: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl FileStamp {
    fn new(metadata: &Metadata) -> Self {
        Self {
            length: metadata.len(), modified: metadata.modified().ok(), created: metadata.created().ok(),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (metadata.dev(), metadata.ino(), metadata.ctime(), metadata.ctime_nsec())
            },
        }
    }
}

pub(super) struct VerifiedSigningAsset { bytes: Arc<[u8]> }

impl VerifiedSigningAsset {
    pub(super) fn bytes(&self) -> &[u8] { &self.bytes }
}

struct Entry {
    path: PathBuf,
    stamp: FileStamp,
    snapshot: Arc<VerifiedSigningAsset>,
}

pub(super) struct SigningAssetCache {
    expected_sha256: String,
    entry: Mutex<Option<Entry>>,
    reads: AtomicUsize,
    hashes: AtomicUsize,
    hits: AtomicUsize,
}

impl SigningAssetCache {
    pub(super) fn new(expected_sha256: &str) -> Self {
        Self { expected_sha256: expected_sha256.into(), entry: Mutex::new(None),
            reads: AtomicUsize::new(0), hashes: AtomicUsize::new(0), hits: AtomicUsize::new(0) }
    }

    /// There is one bounded entry per crypto provider, not an unbounded path map.
    /// Missing and invalid files clear the entry; failures are never cached.
    pub(super) fn load_if_present(&self, path: &Path) -> Result<Option<Arc<VerifiedSigningAsset>>> {
        self.load_if_present_cancellable(path, &|| false)
    }

    pub(super) fn load_if_present_cancellable(
        &self, path: &Path, is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Arc<VerifiedSigningAsset>>> {
        check_cancelled(is_cancelled)?;
        let mut cached = self.entry.lock().unwrap_or_else(|error| error.into_inner());
        check_cancelled(is_cancelled)?;
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => { *cached = None; return Ok(None); },
            Err(_) => { *cached = None; return Err(SourceError::storage("signing_asset_read_failed")); },
        };
        if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > MAX_ASSET_BYTES {
            *cached = None;
            return Err(SourceError::capability("signing_asset_integrity_failed"));
        }
        let observed = FileStamp::new(&metadata);
        if let Some(entry) = cached.as_ref().filter(|entry| entry.path == path && entry.stamp == observed) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Some(entry.snapshot.clone()));
        }
        *cached = None;
        let file = File::open(path).map_err(|_| SourceError::storage("signing_asset_read_failed"))?;
        let before = FileStamp::new(&file.metadata().map_err(|_| SourceError::storage("signing_asset_read_failed"))?);
        if before != observed { return Err(SourceError::capability("signing_asset_changed")); }
        check_cancelled(is_cancelled)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(before.length as usize)
            .map_err(|_| SourceError::capability("signing_asset_allocation_failed"))?;
        self.reads.fetch_add(1, Ordering::Relaxed);
        let mut limited = file.take(MAX_ASSET_BYTES + 1);
        limited.read_to_end(&mut bytes).map_err(|_| SourceError::storage("signing_asset_read_failed"))?;
        check_cancelled(is_cancelled)?;
        let after = FileStamp::new(&limited.get_ref().metadata().map_err(|_| SourceError::storage("signing_asset_read_failed"))?);
        let current = fs::symlink_metadata(path).map_err(|_| SourceError::storage("signing_asset_read_failed"))?;
        if before != after || !current.file_type().is_file() || FileStamp::new(&current) != before
            || bytes.len() as u64 != before.length || bytes.len() as u64 > MAX_ASSET_BYTES {
            return Err(SourceError::capability("signing_asset_changed"));
        }
        self.hashes.fetch_add(1, Ordering::Relaxed);
        if format!("{:x}", Sha256::digest(&bytes)) != self.expected_sha256 {
            return Err(SourceError::capability("signing_asset_integrity_failed"));
        }
        check_cancelled(is_cancelled)?;
        let snapshot = Arc::new(VerifiedSigningAsset { bytes: Arc::from(bytes.into_boxed_slice()) });
        *cached = Some(Entry { path: path.to_owned(), stamp: before, snapshot: snapshot.clone() });
        Ok(Some(snapshot))
    }

    #[cfg(test)]
    fn statistics(&self) -> (usize, usize, usize) {
        (self.reads.load(Ordering::Relaxed), self.hashes.load(Ordering::Relaxed), self.hits.load(Ordering::Relaxed))
    }
}

fn check_cancelled(is_cancelled: &dyn Fn() -> bool) -> Result<()> {
    if is_cancelled() { Err(SourceError::new(super::ErrorKind::Cancelled, "source_work_cancelled")) }
    else { Ok(()) }
}

#[cfg(test)]
#[path = "assets_tests.rs"]
mod tests;

//! One immutable read connects receipt verification to reader preparation.

use super::{
    IntegrityFailureKind, LocalPageResolver, MAX_IMAGE_BYTES, MAX_SOURCE_PIXELS,
    PageReceipt, ReceiptValidationUnavailable, Store, VALIDATION_VERSION, cancelled,
    mark_unverifiable_records, now, publication_lock, receipt_path, same_receipt_identity,
};
use crate::image_integrity::{HashedImage, IntegrityError, ValidatedImage};
use crate::{image_integrity::profile, performance::{Span, StageMap}};
use anyhow::{Context, Result, anyhow, ensure};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
#[cfg(unix)]
use std::{fs::Metadata, path::PathBuf};

struct OpenedPage {
    receipt: PageReceipt,
    #[cfg(unix)]
    path: PathBuf,
    file: File,
    #[cfg(unix)]
    stamp: ObjectStamp,
}

/// Metadata guards the opened object against publication races, not content.
/// The immutable bytes still receive a complete digest on every operation.
#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct ObjectStamp {
    device: u64,
    inode: u64,
    bytes: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

#[cfg(unix)]
impl ObjectStamp {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self { device: metadata.dev(), inode: metadata.ino(), bytes: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()) }
    }
}

enum PreparedPage<T> {
    Ready(ValidatedImage, T),
    Rejected,
    Invalid(IntegrityError),
}

enum FinalizedPage<T> { Ready(T), Missing, Retry }

impl LocalPageResolver {
    /// Read and hash one receipt-backed object, then prepare from those bytes.
    /// The callback must only prepare an in-memory result; an object replacement
    /// may invoke it once more. It must return a proof bound to this exact input.
    /// Cancellation is an error, never a missing page or corruption evidence.
    pub(crate) fn with_verified_page<T>(
        &self, episode_id: &str, revision: &str, page_id: &str, cancellation: &AtomicBool,
        mut prepare: impl FnMut(&HashedImage) -> std::result::Result<Option<(ValidatedImage, T)>, IntegrityError>,
    ) -> Result<Option<T>> {
        self.with_verified_page_with_profile(episode_id, revision, page_id, cancellation,
            None::<&mut StageMap>, |profile| profile, |input, _| prepare(input))
    }

    /// Keep the caller's profile context separate from the resolver layer.
    /// The callback can add reader stages to that same operation-local context
    /// without borrowing it concurrently with receipt and input measurement.
    pub(crate) fn with_verified_page_profiled<T, P>(
        &self, episode_id: &str, revision: &str, page_id: &str, cancellation: &AtomicBool,
        context: &mut P, stages: fn(&mut P) -> &mut StageMap,
        mut prepare: impl FnMut(&HashedImage, &mut P) -> std::result::Result<Option<(ValidatedImage, T)>, IntegrityError>,
    ) -> Result<Option<T>> {
        self.with_verified_page_with_profile(episode_id, revision, page_id, cancellation,
            Some(context), stages, |input, context| {
                prepare(input, context.expect("The profiled resolver owns its context"))
            })
    }

    fn with_verified_page_with_profile<T, P>(
        &self, episode_id: &str, revision: &str, page_id: &str, cancellation: &AtomicBool,
        mut context: Option<&mut P>, stages: fn(&mut P) -> &mut StageMap,
        mut prepare: impl FnMut(&HashedImage, Option<&mut P>) -> std::result::Result<Option<(ValidatedImage, T)>, IntegrityError>,
    ) -> Result<Option<T>> {
        for _ in 0..2 {
            check_cancelled(cancellation)?;
            let publication = publication_lock();
            let (publication_guard, opened) = {
                let mut timing = context.as_deref_mut().map(stages);
                profile::measure(&mut timing, "receipt_lookup", || -> Result<_> {
                    let publication_guard = publication.lock().map_err(|_| anyhow!("page publication lock failed"))?;
                    let opened = self.with_store(|store| {
                        check_cancelled(cancellation)?;
                        open_page(store, &self.account, &self.pages, episode_id, revision, page_id)
                    })?;
                    Ok((publication_guard, opened))
                }, Result::is_ok)
            }?;
            let Some(mut opened) = opened else { return Ok(None); };

            // Unix provides stable opened-file identities. Other desktop
            // platforms retain the existing serialized publication boundary.
            #[cfg(unix)]
            drop(publication_guard);
            #[cfg(test)]
            run_opened_hook();

            let prepared = match read_page(&mut opened.file, &opened.receipt, cancellation,
                context.as_deref_mut().map(stages))? {
                None => PreparedPage::Rejected,
                Some(input) => {
                    let prepare_started = context.as_ref().map(|_| Span::start());
                    let prepared = (|| -> Result<PreparedPage<T>> {
                        let result = prepare(&input, context.as_deref_mut());
                        check_cancelled(cancellation)?;
                        Ok(match result {
                            Ok(Some((proof, value))) => {
                                ensure!(proof.sha256() == input.sha256() && proof.byte_len() == input.byte_len()
                                    && proof.validation_version() == VALIDATION_VERSION,
                                    "The prepared page proof belongs to different encoded bytes");
                                if proof.width() != opened.receipt.width || proof.height() != opened.receipt.height {
                                    PreparedPage::Rejected
                                } else { PreparedPage::Ready(proof, value) }
                            },
                            Ok(None) => return Err(cancelled().into()),
                            Err(error) => {
                                ensure!(error.kind != IntegrityFailureKind::Corrupt || error.encoding_evidence().is_some(),
                                    "The prepared corrupt failure has no encoded-byte evidence");
                                if let Some(evidence) = error.encoding_evidence() {
                                    ensure!(evidence.sha256() == input.sha256() && evidence.byte_len() == input.byte_len(),
                                        "The prepared failure belongs to different encoded bytes");
                                }
                                PreparedPage::Invalid(error)
                            },
                        })
                    })();
                    let completed = matches!(&prepared, Ok(PreparedPage::Ready(_, _)));
                    finish_resolver_stage(&mut context, stages, "aggregate_receipt_prepare", prepare_started, completed);
                    prepared?
                },
            };
            check_cancelled(cancellation)?;

            let recheck_started = context.as_ref().map(|_| Span::start());
            #[cfg(unix)]
            let _publication_guard = match publication.lock() {
                Ok(guard) => guard,
                Err(_) => {
                    finish_resolver_stage(&mut context, stages, "receipt_recheck", recheck_started, false);
                    return Err(anyhow!("page publication lock failed"));
                },
            };
            #[cfg(not(unix))]
            let _publication_guard = publication_guard;
            let finalized = self.with_store(|store| {
                check_cancelled(cancellation)?;
                let Some(current) = store.page_receipt(&self.account, episode_id, revision, page_id)? else {
                    return Ok(FinalizedPage::Missing);
                };
                if !same_receipt_identity(&current, &opened.receipt) {
                    return Ok(FinalizedPage::Retry);
                }
                #[cfg(unix)]
                if !opened.is_current_object()? { return Ok(FinalizedPage::Retry); }
                match prepared {
                    PreparedPage::Ready(proof, value) => {
                        // Current proofs do not need another FULL SQLite write.
                        if current.decode_validation_version != VALIDATION_VERSION
                            && store.promote_page_validation(&self.account, &current, &proof, now())?.is_none() {
                            return Ok(FinalizedPage::Retry);
                        }
                        Ok(FinalizedPage::Ready(value))
                    },
                    PreparedPage::Rejected => {
                        store.invalidate_page(&self.account, episode_id, revision, page_id)?;
                        Ok(FinalizedPage::Missing)
                    },
                    PreparedPage::Invalid(error) => {
                        match error.kind {
                            IntegrityFailureKind::Unsupported | IntegrityFailureKind::ResourceLimit => {
                                let failure = ReceiptValidationUnavailable { kind: error.kind, code: error.code };
                                if mark_unverifiable_records(store, &self.account, &current, &failure, false)? {
                                    return Err(failure.into());
                                }
                            },
                            IntegrityFailureKind::Corrupt => {
                                if current.decode_validation_version == 0 {
                                    if let Some(evidence) = error.encoding_evidence() {
                                        // Only an exact corrupt legacy encoding can release
                                        // its historical identity for a later repair.
                                        store.reject_legacy_invalid_page(&self.account, &current, evidence)?;
                                    }
                                }
                                store.invalidate_page(&self.account, episode_id, revision, page_id)?;
                            },
                        }
                        Ok(FinalizedPage::Missing)
                    },
                }
            });
            finish_resolver_stage(&mut context, stages, "receipt_recheck", recheck_started, finalized.is_ok());
            let finalized = finalized?;
            match finalized {
                FinalizedPage::Ready(value) => return Ok(Some(value)),
                FinalizedPage::Missing => return Ok(None),
                FinalizedPage::Retry => {},
            }
            // The old file, input and any prepared result are dropped before
            // the bounded retry allocates another image.
        }
        Err(anyhow!("The page object changed repeatedly during local preparation"))
    }

    fn with_store<T>(&self, apply: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
        // Every caller already owns publication, keeping one lock order.
        let mut connection = self.connection.lock().map_err(|_| anyhow!("local page connection lock failed"))?;
        if connection.is_none() { *connection = Some(Store::open(&self.database)?); }
        apply(connection.as_ref().context("The local page connection is unavailable")?)
    }
}

#[cfg(unix)]
impl OpenedPage {
    fn is_current_object(&self) -> Result<bool> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error).context("Recheck the prepared page object"),
        };
        Ok(metadata.is_file() && !metadata.file_type().is_symlink()
            && ObjectStamp::from_metadata(&metadata) == self.stamp)
    }
}

fn open_page(store: &Store, account: &str, root: &Path,
    episode_id: &str, revision: &str, page_id: &str) -> Result<Option<OpenedPage>>
{
    let Some(receipt) = store.page_receipt(account, episode_id, revision, page_id)? else { return Ok(None); };
    let path = receipt_path(root, &receipt.file_name)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("Inspect the local page object"),
    };
    if metadata.as_ref().is_none_or(|metadata| !metadata.is_file() || metadata.file_type().is_symlink()
        || metadata.len() != receipt.bytes || receipt.bytes == 0 || receipt.width == 0 || receipt.height == 0) {
        store.invalidate_page(account, episode_id, revision, page_id)?;
        return Ok(None);
    }
    let limit = if receipt.bytes > MAX_IMAGE_BYTES { Some("image_size_limit") }
        else if u64::from(receipt.width) * u64::from(receipt.height) > MAX_SOURCE_PIXELS { Some("image_pixel_limit") }
        else { None };
    if let Some(code) = limit {
        let failure = ReceiptValidationUnavailable { kind: IntegrityFailureKind::ResourceLimit, code };
        if mark_unverifiable_records(store, account, &receipt, &failure, false)? { return Err(failure.into()); }
        return Ok(None);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let file = options.open(&path).context("Open the local page object")?;
    let opened = file.metadata().context("Inspect the opened local page object")?;
    ensure!(opened.is_file() && opened.len() == receipt.bytes, "The local page object changed while opening");
    #[cfg(unix)]
    let stamp = {
        let stamp = ObjectStamp::from_metadata(&opened);
        ensure!(metadata.as_ref().is_some_and(|metadata| ObjectStamp::from_metadata(metadata) == stamp),
            "The local page object was replaced while opening");
        stamp
    };
    Ok(Some(OpenedPage { receipt, #[cfg(unix)] path, file, #[cfg(unix)] stamp }))
}

fn read_page(file: &mut File, receipt: &PageReceipt, cancellation: &AtomicBool,
    mut timing: Option<&mut StageMap>) -> Result<Option<HashedImage>>
{
    let encoded = profile::measure(&mut timing, "source_read", || -> Result<_> {
        check_cancelled(cancellation)?;
        let mut encoded = Vec::with_capacity(receipt.bytes as usize);
        let mut buffer = [0u8; 64 * 1024];
        loop {
            check_cancelled(cancellation)?;
            let count = file.read(&mut buffer).context("Read the local page object")?;
            if count == 0 { break; }
            if encoded.len() as u64 + count as u64 > receipt.bytes { return Ok(None); }
            encoded.extend_from_slice(&buffer[..count]);
        }
        check_cancelled(cancellation)?;
        if encoded.len() as u64 != receipt.bytes { return Ok(None); }
        Ok(Some(encoded))
    }, |result| matches!(result, Ok(Some(_))))?;
    let Some(encoded) = encoded else { return Ok(None); };
    let input = HashedImage::new_profiled(encoded, timing)?;
    check_cancelled(cancellation)?;
    Ok((input.sha256().eq_ignore_ascii_case(&receipt.sha256)).then_some(input))
}

fn finish_resolver_stage<P>(context: &mut Option<&mut P>, stages: fn(&mut P) -> &mut StageMap,
    stage: &'static str, span: Option<Span>, completed: bool)
{
    if let (Some(context), Some(span)) = (context.as_deref_mut(), span) {
        stages(context).entry(stage).or_default().record(span.finish(), completed);
    }
}

fn check_cancelled(cancellation: &AtomicBool) -> Result<()> {
    ensure!(!cancellation.load(Ordering::Acquire), cancelled());
    Ok(())
}

#[cfg(test)]
thread_local! {
    static OPENED_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn run_opened_hook() {
    let hook = OPENED_HOOK.with(|hook| hook.borrow_mut().take());
    if let Some(hook) = hook { hook(); }
}

#[cfg(test)]
#[path = "local_source_tests.rs"]
mod tests;

use super::*;
use crate::{
    domain::{ImageIndex, PageDescriptor},
    image_integrity::{identity_computation_count, validate_image},
    reader::{ReaderCore, ReaderPreparationProfile, ReaderSettings, ValidatedGraySource,
        Viewport, load_hashed_source, load_hashed_source_profiled},
    store::{DownloadRecord, DownloadStatus},
};
use image::{DynamicImage, GrayImage, ImageFormat, Luma};
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::{io::Cursor, sync::{Arc, Mutex}};

const ACCOUNT: &str = "local-source-fixture";

struct Fixture {
    store: Store,
    resolver: LocalPageResolver,
    receipt: PageReceipt,
    encoded: Vec<u8>,
    path: std::path::PathBuf,
    _directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self { Self::with_encoded(encoded_image(31)) }

    fn with_encoded(encoded: Vec<u8>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let pages = directory.path().join("pages");
        fs::create_dir(&pages).unwrap();
        let database = directory.path().join("state.sqlite3");
        let mut store = Store::open(&database).unwrap();
        let digest = format!("{:x}", Sha256::digest(&encoded));
        let receipt = PageReceipt {
            episode_id: "local-source-episode".to_owned(), revision: "local-source-v1".to_owned(),
            page_id: "local-source-page".to_owned(), page_index: 0,
            file_name: format!("{digest}.png"), sha256: digest, bytes: encoded.len() as u64,
            width: 9, height: 13, retained: false, verified_at: 1, decode_validation_version: 0,
        };
        let index = ImageIndex {
            episode_id: receipt.episode_id.clone(), revision: receipt.revision.clone(),
            images: vec![PageDescriptor { id: receipt.page_id.clone(), index: 0, width: 9, height: 13 }],
            ..Default::default()
        };
        let record = DownloadRecord {
            id: "local-source-task".to_owned(), comic_id: "local-source-comic".to_owned(),
            episode_id: receipt.episode_id.clone(), revision: receipt.revision.clone(),
            title: "Local source fixture".to_owned(), comic_title: "Local source fixture".to_owned(),
            status: DownloadStatus::Running, total_pages: 1, generation: 1,
            created_at: 1, updated_at: 1, ..Default::default()
        };
        store.save_index(ACCOUNT, &index).unwrap();
        store.save_download(ACCOUNT, &record).unwrap();
        store.begin_page_commit_generation(ACCOUNT, &record.id, record.generation, &receipt).unwrap();
        let path = pages.join(&receipt.file_name);
        fs::write(&path, &encoded).unwrap();
        store.commit_page_generation(ACCOUNT, &record.id, record.generation, &receipt).unwrap();
        let resolver = LocalPageResolver { database, pages, account: ACCOUNT.to_owned(),
            connection: Arc::new(Mutex::new(None)) };
        Self { store, resolver, receipt, encoded, path, _directory: directory }
    }

    fn read(&self, cancellation: &AtomicBool) -> Result<Option<ValidatedGraySource>> {
        self.resolver.with_verified_page(&self.receipt.episode_id, &self.receipt.revision,
            &self.receipt.page_id, cancellation, |input| load_hashed_source(input, None, cancellation))
            .map(Option::flatten)
    }

    fn current(&self) -> Option<PageReceipt> {
        self.store.page_receipt(ACCOUNT, &self.receipt.episode_id, &self.receipt.revision,
            &self.receipt.page_id).unwrap()
    }

    fn history_count(&self) -> i64 {
        rusqlite::Connection::open(self.store.path()).unwrap().query_row(
            "SELECT count(*) FROM page_identity_history WHERE account_key=?1 AND episode_id=?2 AND revision=?3 AND page_id=?4",
            params![ACCOUNT, self.receipt.episode_id, self.receipt.revision, self.receipt.page_id],
            |row| row.get(0)).unwrap()
    }
}

fn encoded_image(tone: u8) -> Vec<u8> {
    let image = GrayImage::from_fn(9, 13, |x, y| Luma([tone.wrapping_add((x + y * 9) as u8)]));
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(image).write_to(&mut output, ImageFormat::Png).unwrap();
    output.into_inner()
}

fn profiled_fixture(label: &str) -> Fixture {
    let mut encoded = encoded_image(31);
    let end = encoded.split_off(encoded.len() - 12);
    let text = format!("resolver-profile\0{label}");
    encoded.extend_from_slice(&u32::try_from(text.len()).unwrap().to_be_bytes());
    encoded.extend_from_slice(b"tEXt");
    encoded.extend_from_slice(text.as_bytes());
    let mut checksum = crc32fast::Hasher::new();
    checksum.update(b"tEXt");
    checksum.update(text.as_bytes());
    encoded.extend_from_slice(&checksum.finalize().to_be_bytes());
    encoded.extend_from_slice(&end);
    Fixture::with_encoded(encoded)
}

fn assert_receipt_stage(profile: &ReaderPreparationProfile, name: &'static str,
    completed: u32, partial: u32)
{
    let stage = &profile.stages[name];
    assert_eq!((stage.attempts, stage.completed, stage.partial), (1, completed, partial),
        "Unexpected completion accounting for {name}");
}

#[test]
fn receipt_profiling_preserves_the_proof_frame_and_single_hash_per_read() {
    let fixture = profiled_fixture("equivalent-frame");
    let cancellation = AtomicBool::new(false);
    let hashes = identity_computation_count();
    let plain = fixture.read(&cancellation).unwrap().unwrap();
    assert_eq!(identity_computation_count(), hashes + 1);
    let promoted = fixture.current().unwrap();

    let mut profile = ReaderPreparationProfile::default();
    let profiled = fixture.resolver.with_verified_page_profiled(&fixture.receipt.episode_id,
        &fixture.receipt.revision, &fixture.receipt.page_id, &cancellation,
        &mut profile, |profile| &mut profile.stages,
        |input, profile| load_hashed_source_profiled(input, None, &cancellation, profile))
        .unwrap().flatten().unwrap();
    assert_eq!(identity_computation_count(), hashes + 2);
    assert_eq!(profiled.proof(), plain.proof());
    assert_eq!(fixture.current(), Some(promoted));
    assert_eq!(fixture.history_count(), 1);
    for name in ["receipt_lookup", "source_read", "source_hash", "aggregate_receipt_prepare", "receipt_recheck"] {
        assert_receipt_stage(&profile, name, 1, 0);
    }

    let index = ImageIndex {
        episode_id: fixture.receipt.episode_id.clone(), revision: fixture.receipt.revision.clone(),
        images: vec![PageDescriptor { id: fixture.receipt.page_id.clone(), index: 0,
            width: fixture.receipt.width, height: fixture.receipt.height }], ..Default::default()
    };
    let settings = ReaderSettings { landscape_spread: false, ..Default::default() };
    let mut plain_core = ReaderCore::new(index.clone(), settings.clone()).unwrap();
    let mut profiled_core = ReaderCore::new(index, settings).unwrap();
    let viewport = Viewport::new(17, 23);
    let expected = plain_core.render_validated_source(&plain, viewport).unwrap();
    let actual = profiled_core.render_validated_source_profiled(&profiled, viewport, &mut profile).unwrap();
    assert_eq!((actual.width, actual.height), (expected.width, expected.height));
    assert_eq!((actual.source_width, actual.source_height), (expected.source_width, expected.source_height));
    assert_eq!((actual.page_index, actual.crop_index, actual.crop_count),
        (expected.page_index, expected.crop_index, expected.crop_count));
    assert_eq!(actual.crop, expected.crop);
    assert_eq!(actual.overlap_from_previous, expected.overlap_from_previous);
    assert_eq!(actual.heuristic_panels, expected.heuristic_panels);
    assert_eq!(actual.dark_fraction, expected.dark_fraction);
    assert_eq!(actual.rgba, expected.rgba);
    assert_eq!(profiled_core.anchor(123), plain_core.anchor(123));
    assert_eq!(identity_computation_count(), hashes + 2);
}

#[test]
fn a_pre_cancelled_profiled_receipt_read_does_not_attempt_work_or_change_storage() {
    let fixture = profiled_fixture("pre-cancelled");
    let cancellation = AtomicBool::new(true);
    let hashes = identity_computation_count();
    let mut profile = ReaderPreparationProfile::default();
    let result = fixture.resolver.with_verified_page_profiled::<(), _>(&fixture.receipt.episode_id,
        &fixture.receipt.revision, &fixture.receipt.page_id, &cancellation,
        &mut profile, |profile| &mut profile.stages,
        |_, _| panic!("Pre-cancelled receipt work must not invoke preparation"));
    assert!(result.unwrap_err().downcast_ref::<super::super::AcquisitionFailure>()
        .is_some_and(|failure| failure.code == "download_cancelled"));
    assert_eq!(profile, ReaderPreparationProfile::default());
    assert_eq!(identity_computation_count(), hashes);
    assert!(fixture.resolver.connection.lock().unwrap().is_none());
    assert_eq!(fixture.current(), Some(fixture.receipt.clone()));
    assert_eq!(fixture.history_count(), 1);
    assert_eq!(fs::read(&fixture.path).unwrap(), fixture.encoded);
}

#[test]
fn one_receipt_read_hashes_once_and_a_current_proof_does_not_write_again() {
    let fixture = Fixture::new();
    let cancellation = AtomicBool::new(false);
    let before = identity_computation_count();
    let source = fixture.read(&cancellation).unwrap().unwrap();
    assert_eq!(identity_computation_count(), before + 1);
    assert_eq!(source.proof().sha256(), fixture.receipt.sha256);
    let promoted = fixture.current().unwrap();
    assert_eq!(promoted.decode_validation_version, VALIDATION_VERSION);
    assert!(fixture.resolver.connection.lock().unwrap().is_some());
    let connection = rusqlite::Connection::open(fixture.store.path()).unwrap();
    connection.execute_batch(
        "CREATE TRIGGER reject_unnecessary_receipt_write BEFORE UPDATE ON page_receipts
         BEGIN SELECT RAISE(FAIL, 'A verified local read must not rewrite its receipt'); END;").unwrap();
    let next = fixture.resolver.clone().with_verified_page(&fixture.receipt.episode_id,
        &fixture.receipt.revision, &fixture.receipt.page_id, &cancellation,
        |input| load_hashed_source(input, Some(source.proof()), &cancellation)).unwrap();
    assert!(matches!(next, Some(None)));
    assert_eq!(identity_computation_count(), before + 2);
    assert_eq!(fixture.current().unwrap(), promoted);
}

#[test]
fn cached_pixels_do_not_hide_same_path_corruption_or_release_historical_identity() {
    let fixture = Fixture::new();
    let cancellation = AtomicBool::new(false);
    let source = fixture.read(&cancellation).unwrap().unwrap();
    let mut corrupt = fixture.encoded.clone();
    corrupt[0] ^= 0x80;
    fs::write(&fixture.path, corrupt).unwrap();
    let changed = fixture.resolver.with_verified_page(&fixture.receipt.episode_id,
        &fixture.receipt.revision, &fixture.receipt.page_id, &cancellation,
        |input| load_hashed_source(input, Some(source.proof()), &cancellation)).unwrap();
    assert!(changed.is_none());
    assert!(fixture.current().is_none());
    assert_eq!(fixture.history_count(), 1);
}

#[test]
fn a_wrong_preparation_proof_does_not_invalidate_the_real_page() {
    let fixture = Fixture::new();
    let other = validate_image(&encoded_image(197), None).unwrap();
    let cancellation = AtomicBool::new(false);
    let result = fixture.resolver.with_verified_page(&fixture.receipt.episode_id,
        &fixture.receipt.revision, &fixture.receipt.page_id, &cancellation,
        |_| Ok(Some((other.clone(), ()))));
    assert!(result.unwrap_err().to_string().contains("different encoded bytes"));
    assert_eq!(fixture.current(), Some(fixture.receipt.clone()));
    assert_eq!(fixture.history_count(), 1);
    assert_eq!(fs::read(&fixture.path).unwrap(), fixture.encoded);
}

#[test]
fn a_wrong_failure_proof_does_not_invalidate_the_real_page() {
    let fixture = Fixture::new();
    let failure = validate_image(b"Another incomplete encoded object", None).unwrap_err();
    let cancellation = AtomicBool::new(false);
    let result = fixture.resolver.with_verified_page::<()>(&fixture.receipt.episode_id,
        &fixture.receipt.revision, &fixture.receipt.page_id, &cancellation,
        |_| Err(failure.clone()));
    assert!(result.unwrap_err().to_string().contains("different encoded bytes"));
    assert_eq!(fixture.current(), Some(fixture.receipt.clone()));
    assert_eq!(fixture.history_count(), 1);
}

#[test]
fn an_exact_corrupt_legacy_encoding_releases_only_its_unproved_identity() {
    let mut bytes = encoded_image(43);
    bytes.truncate(bytes.len() - 12);
    let fixture = Fixture::with_encoded(bytes);
    assert!(fixture.read(&AtomicBool::new(false)).unwrap().is_none());
    assert!(fixture.current().is_none());
    assert_eq!(fixture.history_count(), 0);
}

#[test]
fn cancellation_after_decode_does_not_promote_or_invalidate_a_legacy_receipt() {
    let fixture = profiled_fixture("cancel-after-decoding");
    let cancellation = AtomicBool::new(false);
    let hashes = identity_computation_count();
    let mut profile = ReaderPreparationProfile::default();
    let result = fixture.resolver.with_verified_page_profiled(&fixture.receipt.episode_id,
        &fixture.receipt.revision, &fixture.receipt.page_id, &cancellation,
        &mut profile, |profile| &mut profile.stages, |input, profile| {
            let prepared = load_hashed_source_profiled(input, None, &cancellation, profile);
            assert!(matches!(&prepared, Ok(Some((_, Some(_))))));
            cancellation.store(true, Ordering::Release);
            prepared
        });
    assert!(result.map(|_| ()).unwrap_err().downcast_ref::<super::super::AcquisitionFailure>()
        .is_some_and(|failure| failure.code == "download_cancelled"));
    assert_eq!(fixture.current(), Some(fixture.receipt.clone()));
    assert_eq!(fixture.history_count(), 1);
    assert_eq!(identity_computation_count(), hashes + 1);
    for name in ["receipt_lookup", "source_read", "source_hash", "aggregate_integrity", "decode"] {
        assert_receipt_stage(&profile, name, 1, 0);
    }
    assert_receipt_stage(&profile, "aggregate_receipt_prepare", 0, 1);
    assert!(!profile.stages.contains_key("receipt_recheck"));
    assert_eq!((profile.decoded_sources, profile.reused_sources, profile.unchanged_sources), (1, 0, 0));
    assert_eq!(fs::read(&fixture.path).unwrap(), fixture.encoded);
}

#[cfg(unix)]
#[test]
fn a_same_identity_atomic_repair_cannot_be_invalidated_by_the_old_bad_file() {
    let fixture = Fixture::new();
    let mut corrupt = fixture.encoded.clone();
    corrupt[0] ^= 0x80;
    fs::write(&fixture.path, &corrupt).unwrap();
    let path = fixture.path.clone();
    let replacement = path.with_extension("replacement");
    let complete = fixture.encoded.clone();
    OPENED_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(move || {
        let publication = publication_lock();
        let _publication = publication.try_lock().expect("The opened read must release publication before I/O");
        fs::write(&replacement, complete).unwrap();
        fs::rename(replacement, path).unwrap();
    })));
    let source = fixture.read(&AtomicBool::new(false)).unwrap().unwrap();
    assert_eq!(source.proof().sha256(), fixture.receipt.sha256);
    assert_eq!(fixture.current().unwrap().decode_validation_version, VALIDATION_VERSION);
    assert_eq!(fixture.history_count(), 1);
    assert_eq!(fs::read(&fixture.path).unwrap(), fixture.encoded);
}

#[cfg(unix)]
#[test]
fn receipt_removal_during_preparation_cannot_be_resurrected_by_a_proof() {
    let fixture = Fixture::new();
    let cancellation = AtomicBool::new(false);
    let result = fixture.resolver.with_verified_page(&fixture.receipt.episode_id,
        &fixture.receipt.revision, &fixture.receipt.page_id, &cancellation, |input| {
            let prepared = load_hashed_source(input, None, &cancellation);
            let publication = publication_lock();
            let _publication = publication.try_lock().expect("Decode must not hold publication");
            fixture.store.invalidate_page(ACCOUNT, &fixture.receipt.episode_id,
                &fixture.receipt.revision, &fixture.receipt.page_id).unwrap();
            prepared
        }).unwrap();
    assert!(result.is_none());
    assert!(fixture.current().is_none());
    assert_eq!(fixture.history_count(), 1);
}

#[cfg(unix)]
#[test]
fn a_concurrent_retain_survives_legacy_source_promotion() {
    let fixture = Fixture::new();
    let cancellation = AtomicBool::new(false);
    let result = fixture.resolver.with_verified_page(&fixture.receipt.episode_id,
        &fixture.receipt.revision, &fixture.receipt.page_id, &cancellation, |input| {
            let prepared = load_hashed_source(input, None, &cancellation);
            let publication = publication_lock();
            let _publication = publication.try_lock().expect("Decode must not hold publication");
            assert!(fixture.store.retain_page(ACCOUNT, &fixture.receipt.episode_id,
                &fixture.receipt.revision, &fixture.receipt.page_id).unwrap());
            prepared
        }).unwrap();
    assert!(result.flatten().is_some());
    let current = fixture.current().unwrap();
    assert!(current.retained);
    assert_eq!(current.decode_validation_version, VALIDATION_VERSION);
}

#[cfg(unix)]
#[test]
fn cancellation_before_read_leaves_receipts_and_history_unchanged() {
    let fixture = Fixture::new();
    let cancellation = Arc::new(AtomicBool::new(false));
    let hook_cancellation = cancellation.clone();
    OPENED_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(move || {
        hook_cancellation.store(true, Ordering::Release);
    })));
    assert!(fixture.read(&cancellation).is_err());
    assert_eq!(fixture.current(), Some(fixture.receipt.clone()));
    assert_eq!(fixture.history_count(), 1);
}

#[cfg(unix)]
#[test]
fn continuous_atomic_replacement_is_bounded_and_never_drops_the_receipt() {
    let fixture = Fixture::new();
    let cancellation = AtomicBool::new(false);
    let mut attempts = 0;
    let result = fixture.resolver.with_verified_page(&fixture.receipt.episode_id,
        &fixture.receipt.revision, &fixture.receipt.page_id, &cancellation, |input| {
            attempts += 1;
            let prepared = load_hashed_source(input, None, &cancellation);
            let publication = publication_lock();
            let _publication = publication.try_lock().expect("Decode must not hold publication");
            let replacement = fixture.path.with_extension("replacement");
            fs::write(&replacement, &fixture.encoded).unwrap();
            fs::rename(replacement, &fixture.path).unwrap();
            prepared
        });
    assert!(result.map(|_| ()).unwrap_err().to_string().contains("changed repeatedly"));
    assert_eq!(attempts, 2);
    assert_eq!(fixture.current(), Some(fixture.receipt.clone()));
    assert_eq!(fixture.history_count(), 1);
}

use super::{
    AcquiredPage, AcquisitionFailure, AcquisitionFuture, AcquisitionSource, DownloadEventKind,
    DownloadManager, PreparedPageBatch, ReceiptValidationUnavailable, revalidate_receipt,
    verify_receipt_file,
};
use crate::{
    domain::{ImageIndex, PageDescriptor},
    image_integrity::IntegrityFailureKind,
    store::{DownloadRecord, DownloadStatus, PageReceipt, Store},
};
use flate2::{Compression, write::ZlibEncoder};
use image::{AnimationDecoder, ImageDecoder};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Cursor, Write},
    sync::{Arc, atomic::{AtomicUsize, Ordering}},
};
use tokio::runtime::Handle;

const ACCOUNT: &str = "legacy-preservation-account";

#[derive(Default)]
struct NoAcquisitionSource {
    index_calls: AtomicUsize,
    page_calls: AtomicUsize,
    prepare_calls: AtomicUsize,
}

impl NoAcquisitionSource {
    fn failure() -> AcquisitionFailure {
        AcquisitionFailure { code: "preservation_fixture_acquisition_forbidden".to_owned(), retryable: false }
    }

    fn assert_unused(&self) {
        assert_eq!(self.index_calls.load(Ordering::SeqCst), 0);
        assert_eq!(self.page_calls.load(Ordering::SeqCst), 0);
        assert_eq!(self.prepare_calls.load(Ordering::SeqCst), 0);
    }
}

impl AcquisitionSource for NoAcquisitionSource {
    fn image_index<'a>(&'a self, _episode_id: &'a str) -> AcquisitionFuture<'a, ImageIndex> {
        Box::pin(async move {
            self.index_calls.fetch_add(1, Ordering::SeqCst);
            Err(Self::failure())
        })
    }

    fn download_page<'a>(&'a self, _index: &'a ImageIndex, _page_index: usize)
        -> AcquisitionFuture<'a, AcquiredPage>
    {
        Box::pin(async move {
            self.page_calls.fetch_add(1, Ordering::SeqCst);
            Err(Self::failure())
        })
    }

    fn page_batch_size(&self) -> usize { 20 }

    fn prepare_page_batch<'a>(&'a self, _index: &'a ImageIndex, _page_indices: &'a [usize])
        -> AcquisitionFuture<'a, Arc<dyn PreparedPageBatch>>
    {
        Box::pin(async move {
            self.prepare_calls.fetch_add(1, Ordering::SeqCst);
            Err(Self::failure())
        })
    }
}

struct LegalLegacyImage {
    bytes: Vec<u8>,
    extension: &'static str,
    width: u32,
    height: u32,
}

fn oracle_digest(bytes: &[u8]) -> String {
    let mut digest = String::new();
    for byte in Sha256::digest(bytes) { digest.push_str(&format!("{byte:02x}")); }
    digest
}

fn legal_animated_gif() -> LegalLegacyImage {
    let (width, height) = (11, 13);
    let tones = [17, 91];
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        let frames = tones.map(|tone| image::Frame::new(image::RgbaImage::from_pixel(
            width, height, image::Rgba([tone, tone, tone, 255]))));
        encoder.encode_frames(frames.into_iter()).unwrap();
    }
    // Independent full animation decoding establishes that this is a real two-frame image.
    let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(&bytes)).unwrap();
    let frames = decoder.into_frames().collect_frames().unwrap();
    assert_eq!(frames.len(), 2);
    for (frame, tone) in frames.iter().zip(tones) {
        assert_eq!(frame.buffer().dimensions(), (width, height));
        assert!(frame.buffer().pixels().all(|pixel| *pixel == image::Rgba([tone, tone, tone, 255])));
    }
    LegalLegacyImage { bytes, extension: "gif", width, height }
}

fn legal_large_monochrome_png() -> LegalLegacyImage {
    fn chunk(output: &mut Vec<u8>, kind: &[u8; 4], bytes: &[u8]) {
        output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        output.extend_from_slice(kind);
        output.extend_from_slice(bytes);
        let mut crc = crc32fast::Hasher::new();
        crc.update(kind);
        crc.update(bytes);
        output.extend_from_slice(&crc.finalize().to_be_bytes());
    }
    let (width, height) = (8_000_u32, 4_001_u32);
    let pixels = u64::from(width) * u64::from(height);
    assert_eq!(pixels, 32_008_000);
    assert!(pixels > 32_000_000 && pixels <= 64_000_000);
    let row = vec![0_u8; width as usize / 8 + 1];
    assert_eq!(row.len(), 1_001);
    let mut compressed = ZlibEncoder::new(Vec::new(), Compression::fast());
    for _ in 0..height { compressed.write_all(&row).unwrap(); }
    let compressed = compressed.finish().unwrap();
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[1, 0, 0, 0, 0]);
    chunk(&mut bytes, b"IHDR", &header);
    chunk(&mut bytes, b"IDAT", &compressed);
    chunk(&mut bytes, b"IEND", &[]);
    // Decode the entire bit-packed image independently of the application's pixel policy.
    let decoder = image::codecs::png::PngDecoder::new(Cursor::new(&bytes)).unwrap();
    assert_eq!(decoder.dimensions(), (width, height));
    assert_eq!(decoder.color_type(), image::ColorType::L8);
    assert_eq!(decoder.total_bytes(), pixels);
    let mut decoded = vec![0xff; decoder.total_bytes() as usize];
    decoder.read_image(&mut decoded).unwrap();
    assert!(decoded.iter().all(|pixel| *pixel == 0));
    LegalLegacyImage { bytes, extension: "png", width, height }
}

fn seed_legacy_publication(
    manager: &DownloadManager, episode_id: &str, image: &LegalLegacyImage, commit: bool,
) -> (DownloadRecord, PageReceipt, ImageIndex) {
    let revision = format!("{episode_id}-legacy-v1");
    let task = DownloadRecord {
        id: format!("preservation-task-{episode_id}"), comic_id: format!("comic-{episode_id}"),
        episode_id: episode_id.to_owned(), revision: revision.clone(),
        title: "Legacy preservation fixture".to_owned(), comic_title: "Preservation fixture comic".to_owned(),
        status: DownloadStatus::Running, total_pages: 1, generation: 1,
        retained: true, created_at: 1, updated_at: 1, ..Default::default()
    };
    let index = ImageIndex {
        episode_id: episode_id.to_owned(), revision: revision.clone(),
        images: vec![PageDescriptor { id: "legacy-page-0".to_owned(), index: 0,
            width: image.width, height: image.height }],
        ..Default::default()
    };
    let digest = oracle_digest(&image.bytes);
    let receipt = PageReceipt {
        episode_id: episode_id.to_owned(), revision, page_id: index.images[0].id.clone(), page_index: 0,
        file_name: format!("{digest}.{}", image.extension), sha256: digest,
        bytes: image.bytes.len() as u64, width: image.width, height: image.height,
        retained: true, verified_at: 1, decode_validation_version: 0,
    };
    let mut store = Store::open(manager.database_path()).unwrap();
    store.save_index(ACCOUNT, &index).unwrap();
    store.save_download(ACCOUNT, &task).unwrap();
    // Raw version-zero publication reproduces data acquired before complete decode proofs.
    store.begin_page_commit_generation(ACCOUNT, &task.id, task.generation, &receipt).unwrap();
    let mut file = File::create(manager.pages_root().join(&receipt.file_name)).unwrap();
    file.write_all(&image.bytes).unwrap();
    file.sync_all().unwrap();
    drop(file);
    if commit { store.commit_page_generation(ACCOUNT, &task.id, task.generation, &receipt).unwrap(); }
    (store.load_download(ACCOUNT, &task.id).unwrap().unwrap(), receipt, index)
}

fn seed_legacy_completed(
    manager: &DownloadManager, episode_id: &str, image: &LegalLegacyImage,
) -> (DownloadRecord, PageReceipt, ImageIndex) {
    let (task, receipt, index) = seed_legacy_publication(manager, episode_id, image, true);
    assert_eq!(task.completed_pages, 1);
    // Only the migration fixture bypasses the new writer's prohibition on unproved Completed.
    let connection = rusqlite::Connection::open(manager.database_path()).unwrap();
    assert_eq!(connection.execute(
        "UPDATE downloads SET status='completed',payload=json_set(payload,'$.status','completed')
         WHERE account_key=?1 AND task_id=?2",
        rusqlite::params![ACCOUNT, task.id],
    ).unwrap(), 1);
    let completed = Store::open(manager.database_path()).unwrap().load_download(ACCOUNT, &task.id).unwrap().unwrap();
    assert_eq!(completed.status, DownloadStatus::Completed);
    (completed, receipt, index)
}

fn assert_unavailable(error: anyhow::Error, expected: IntegrityFailureKind) {
    let unavailable = error.downcast_ref::<ReceiptValidationUnavailable>()
        .expect("a preserved legacy image must report typed validation unavailability");
    assert_eq!(unavailable.kind, expected);
    assert!(!unavailable.code.is_empty());
}

fn assert_original_preserved(
    manager: &DownloadManager, task: &DownloadRecord, receipt: &PageReceipt,
    index: &ImageIndex, image: &LegalLegacyImage,
) {
    let store = Store::open(manager.database_path()).unwrap();
    let current = store.page_receipt(ACCOUNT, &receipt.episode_id, &receipt.revision, &receipt.page_id)
        .unwrap().expect("an unavailable image must keep its receipt");
    assert_eq!(&current, receipt);
    assert_eq!(current.decode_validation_version, 0);
    let stored_index = store.load_index(ACCOUNT, &receipt.episode_id, Some(&receipt.revision)).unwrap().unwrap();
    assert_eq!(&stored_index, index);
    let state = store.load_download(ACCOUNT, &task.id).unwrap().unwrap();
    assert_eq!(state.status, DownloadStatus::Failed);
    assert_ne!(state.status, DownloadStatus::Completed);
    assert!(store.page_object_referenced(ACCOUNT, &receipt.file_name).unwrap());
    let original = fs::read(manager.pages_root().join(&receipt.file_name)).unwrap();
    assert_eq!(original.as_slice(), image.bytes.as_slice());
    assert_eq!(oracle_digest(&original), receipt.sha256);
    let connection = rusqlite::Connection::open(manager.database_path()).unwrap();
    let history: (String, i64, i64) = connection.query_row(
        "SELECT sha256,bytes,decode_validation_version FROM page_identity_history
         WHERE account_key=?1 AND episode_id=?2 AND revision=?3 AND page_id=?4",
        rusqlite::params![ACCOUNT, receipt.episode_id, receipt.revision, receipt.page_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap();
    assert_eq!(history, (receipt.sha256.clone(), receipt.bytes as i64, 0));
    assert!(!verify_receipt_file(manager.pages_root(), receipt).unwrap());
}

async fn assert_preserved_through_recovery(
    episode_id: &str, image: LegalLegacyImage, expected: IntegrityFailureKind,
) {
    let directory = tempfile::tempdir().unwrap();
    let source = Arc::new(NoAcquisitionSource::default());
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    let (task, receipt, index) = seed_legacy_completed(&manager, episode_id, &image);
    for _ in 0..2 {
        manager.recover().unwrap();
        assert_original_preserved(&manager, &task, &receipt, &index, &image);
        let resolver = manager.page_resolver();
        let error = resolver.local_page(&receipt.episode_id, &receipt.revision, &receipt.page_id)
            .expect_err("an unavailable image must not become an offline reading path");
        assert_unavailable(error, expected);
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let error = resolver.with_verified_page(&receipt.episode_id, &receipt.revision, &receipt.page_id,
            &cancelled, |input| crate::reader::load_hashed_source(input, None, &cancelled))
            .map(|_| ()).expect_err("single-read preparation must preserve unsupported originals");
        assert_unavailable(error, expected);
        let store = Store::open(manager.database_path()).unwrap();
        let error = revalidate_receipt(&store, ACCOUNT, manager.pages_root(), &receipt)
            .expect_err("an unavailable image must not produce a verified receipt");
        assert_unavailable(error, expected);
        drop(store);
        tokio::task::yield_now().await;
        assert_original_preserved(&manager, &task, &receipt, &index, &image);
        let events = manager.drain_events(1_024);
        assert!(!events.iter().any(|event| matches!(event.kind,
            DownloadEventKind::Completed | DownloadEventKind::PageReady { .. })));
        source.assert_unused();
    }
}

#[tokio::test]
async fn a_legal_unsupported_legacy_animation_keeps_its_original_and_receipt() {
    assert_preserved_through_recovery("legacy-animation", legal_animated_gif(), IntegrityFailureKind::Unsupported).await;
}

#[tokio::test]
async fn a_legal_legacy_image_above_the_new_pixel_budget_keeps_its_original_and_receipt() {
    assert_preserved_through_recovery("legacy-large-monochrome", legal_large_monochrome_png(), IntegrityFailureKind::ResourceLimit).await;
}

#[tokio::test]
async fn an_unavailable_pending_journal_survives_recovery_after_its_task_becomes_ineligible() {
    let directory = tempfile::tempdir().unwrap();
    let image = legal_animated_gif();
    let source = Arc::new(NoAcquisitionSource::default());
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    let (task, receipt, index) = seed_legacy_publication(&manager, "legacy-pending-animation", &image, false);
    assert_eq!(task.status, DownloadStatus::Running);
    assert_eq!(task.generation, 1);
    assert_eq!(task.completed_pages, 0);
    for _ in 0..2 {
        manager.recover().unwrap();
        let store = Store::open(manager.database_path()).unwrap();
        let pending = store.pending_page_commits(ACCOUNT).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].task_id, task.id);
        assert_eq!(pending[0].generation, task.generation);
        assert_eq!(pending[0].receipt, receipt);
        let state = store.load_download(ACCOUNT, &task.id).unwrap().unwrap();
        assert_eq!(state.status, DownloadStatus::Failed);
        assert!(store.page_receipt(ACCOUNT, &receipt.episode_id, &receipt.revision, &receipt.page_id).unwrap().is_none());
        let stored_index = store.load_index(ACCOUNT, &receipt.episode_id, Some(&receipt.revision)).unwrap().unwrap();
        assert_eq!(stored_index, index);
        assert!(store.page_object_referenced(ACCOUNT, &receipt.file_name).unwrap());
        assert!(manager.page_resolver().local_page(&receipt.episode_id, &receipt.revision, &receipt.page_id)
            .unwrap().is_none());
        assert!(revalidate_receipt(&store, ACCOUNT, manager.pages_root(), &receipt).unwrap().is_none());
        let history_count: i64 = rusqlite::Connection::open(manager.database_path()).unwrap().query_row(
            "SELECT COUNT(*) FROM page_identity_history
             WHERE account_key=?1 AND episode_id=?2 AND revision=?3 AND page_id=?4",
            rusqlite::params![ACCOUNT, receipt.episode_id, receipt.revision, receipt.page_id],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(history_count, 0);
        let original = fs::read(manager.pages_root().join(&receipt.file_name)).unwrap();
        assert_eq!(original.as_slice(), image.bytes.as_slice());
        assert_eq!(oracle_digest(&original), receipt.sha256);
        drop(store);
        tokio::task::yield_now().await;
        let events = manager.drain_events(1_024);
        assert!(!events.iter().any(|event| matches!(event.kind,
            DownloadEventKind::Completed | DownloadEventKind::PageReady { .. })));
        source.assert_unused();
    }
}

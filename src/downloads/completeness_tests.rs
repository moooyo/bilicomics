use super::{
    AcquiredPage, AcquisitionFailure, AcquisitionFuture, AcquisitionSource, DownloadEvent,
    DownloadEventKind, DownloadManager, DownloadRequest, PreparedPageBatch,
};
use crate::{domain::{ImageIndex, PageDescriptor}, store::{DownloadRecord, DownloadStatus, PageReceipt, Store}};
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::Cursor,
    path::Path,
    sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}},
    time::Duration,
};
use tokio::{runtime::Handle, sync::Semaphore};

const ACCOUNT: &str = "completeness-oracle-account";
const WIDTH: u32 = 11;
const HEIGHT: u32 = 13;

#[derive(Clone)]
struct ExpectedPage {
    id: String,
    bytes: Vec<u8>,
    digest: String,
    pixels: Vec<u8>,
}

struct Manifest {
    episode_id: String,
    revision: String,
    pages: Vec<ExpectedPage>,
}

impl Manifest {
    fn new(episode_id: &str, page_count: usize) -> Arc<Self> {
        Self::versioned(episode_id, &format!("{episode_id}-manifest-v1"), page_count, 0)
    }

    fn versioned(episode_id: &str, revision: &str, page_count: usize, color_offset: u8) -> Arc<Self> {
        // The expected chapter is defined before any acquisition or batch selection exists.
        let pages = (1..=page_count).map(|page_number| {
            let image = image::RgbaImage::from_fn(WIDTH, HEIGHT, |x, y| {
                image::Rgba([(page_number as u8).wrapping_add(color_offset), x as u8, y as u8, 255])
            });
            let pixels = image.as_raw().clone();
            let mut output = Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image).write_to(&mut output, image::ImageFormat::Png).unwrap();
            let bytes = output.into_inner();
            ExpectedPage {
                id: format!("manifest-leaf-{page_number:04}"), digest: oracle_digest(&bytes), bytes, pixels,
            }
        }).collect::<Vec<_>>();
        assert_eq!(pages.iter().map(|page| &page.digest).collect::<HashSet<_>>().len(), page_count);
        Arc::new(Self {
            episode_id: episode_id.to_owned(), revision: revision.to_owned(), pages,
        })
    }
}

struct SourceState {
    index_calls: AtomicUsize,
    requested: Mutex<Vec<usize>>,
    prepared: Mutex<Vec<Vec<usize>>>,
    held_page: Option<usize>,
    held_gate: Semaphore,
    fail_last: AtomicBool,
}

struct ManifestSource {
    manifest: Arc<Manifest>,
    state: Arc<SourceState>,
}

impl ManifestSource {
    fn new(manifest: Arc<Manifest>, held_page: Option<usize>, fail_last: bool) -> Arc<Self> {
        Arc::new(Self {
            manifest,
            state: Arc::new(SourceState {
                index_calls: AtomicUsize::new(0),
                requested: Mutex::new(Vec::new()), prepared: Mutex::new(Vec::new()),
                held_page, held_gate: Semaphore::new(0), fail_last: AtomicBool::new(fail_last),
            }),
        })
    }

    fn requested(&self) -> Vec<usize> { self.state.requested.lock().unwrap().clone() }

    fn prepared(&self) -> Vec<Vec<usize>> { self.state.prepared.lock().unwrap().clone() }

    fn release(&self, count: usize) { self.state.held_gate.add_permits(count); }
}

struct ManifestBatch {
    manifest: Arc<Manifest>,
    state: Arc<SourceState>,
    selected: HashSet<usize>,
}

impl PreparedPageBatch for ManifestBatch {
    fn download_page(&self, page_index: usize) -> AcquisitionFuture<'_, AcquiredPage> {
        Box::pin(async move {
            if !self.selected.contains(&page_index) {
                return Err(AcquisitionFailure { code: "oracle_page_outside_batch".to_owned(), retryable: false });
            }
            self.state.requested.lock().unwrap().push(page_index);
            if self.state.held_page == Some(page_index) {
                let permit = self.state.held_gate.acquire().await.map_err(|_| AcquisitionFailure {
                    code: "oracle_page_gate_closed".to_owned(), retryable: false,
                })?;
                permit.forget();
            }
            if page_index + 1 == self.manifest.pages.len() && self.state.fail_last.load(Ordering::SeqCst) {
                return Err(AcquisitionFailure { code: "oracle_last_page_failed".to_owned(), retryable: true });
            }
            let expected = &self.manifest.pages[page_index];
            Ok(AcquiredPage {
                bytes: expected.bytes.clone(), width: WIDTH, height: HEIGHT, sha256: Some(expected.digest.clone()),
            })
        })
    }
}

impl AcquisitionSource for ManifestSource {
    fn image_index<'a>(&'a self, episode_id: &'a str) -> AcquisitionFuture<'a, ImageIndex> {
        Box::pin(async move {
            self.state.index_calls.fetch_add(1, Ordering::SeqCst);
            if episode_id != self.manifest.episode_id {
                return Err(AcquisitionFailure { code: "oracle_episode_mismatch".to_owned(), retryable: false });
            }
            Ok(ImageIndex {
                episode_id: self.manifest.episode_id.clone(), revision: self.manifest.revision.clone(),
                images: self.manifest.pages.iter().enumerate().map(|(index, page)| PageDescriptor {
                    id: page.id.clone(), index, width: WIDTH, height: HEIGHT,
                }).collect(),
                paths: self.manifest.pages.iter().map(|page| format!("fixture-memory/{}", page.id)).collect(),
                ..Default::default()
            })
        })
    }

    fn download_page<'a>(&'a self, _index: &'a ImageIndex, _page_index: usize)
        -> AcquisitionFuture<'a, AcquiredPage>
    {
        Box::pin(async {
            Err(AcquisitionFailure { code: "oracle_legacy_path_used".to_owned(), retryable: false })
        })
    }

    fn page_batch_size(&self) -> usize { 20 }

    fn prepare_page_batch<'a>(&'a self, index: &'a ImageIndex, page_indices: &'a [usize])
        -> AcquisitionFuture<'a, Arc<dyn PreparedPageBatch>>
    {
        Box::pin(async move {
            assert_eq!(index.episode_id, self.manifest.episode_id);
            assert_eq!(index.revision, self.manifest.revision);
            assert!(!page_indices.is_empty() && page_indices.len() <= 20);
            assert!(page_indices.iter().all(|index| *index < self.manifest.pages.len()));
            let selected = page_indices.iter().copied().collect::<HashSet<_>>();
            assert_eq!(selected.len(), page_indices.len());
            self.state.prepared.lock().unwrap().push(page_indices.to_vec());
            Ok(Arc::new(ManifestBatch {
                manifest: self.manifest.clone(), state: self.state.clone(), selected,
            }) as Arc<dyn PreparedPageBatch>)
        })
    }
}

fn oracle_digest(bytes: &[u8]) -> String {
    let mut result = String::new();
    for byte in Sha256::digest(bytes) { result.push_str(&format!("{byte:02x}")); }
    result
}

fn request(manifest: &Manifest, retain: bool) -> DownloadRequest {
    DownloadRequest {
        comic_id: format!("comic-{}", manifest.episode_id), episode_id: manifest.episode_id.clone(),
        title: "Completeness fixture".to_owned(), comic_title: "Oracle fixture comic".to_owned(),
        expected_revision: Some(manifest.revision.clone()), retain, ..Default::default()
    }
}

fn stored_record(manager: &DownloadManager, task_id: &str) -> DownloadRecord {
    Store::open(manager.database_path()).unwrap().load_download(ACCOUNT, task_id).unwrap().unwrap()
}

fn collect_events(manager: &mut DownloadManager, events: &mut Vec<DownloadEvent>) {
    events.extend(manager.drain_events(512));
}

fn has_completion(events: &[DownloadEvent], task_id: &str) -> bool {
    events.iter().any(|event| event.task_id == task_id && matches!(event.kind, DownloadEventKind::Completed))
}

async fn wait_until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if predicate() { return; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("the manifest fixture did not reach its expected state");
}

async fn wait_for_completion(manager: &mut DownloadManager, task_id: &str, events: &mut Vec<DownloadEvent>) {
    wait_until(|| {
        collect_events(manager, events);
        has_completion(events, task_id) && stored_record(manager, task_id).status == DownloadStatus::Completed
    }).await;
}

fn assert_receipts(
    manager: &DownloadManager, manifest: &Manifest, selected: &[usize], retained: bool,
) -> Vec<PageReceipt> {
    let store = Store::open(manager.database_path()).unwrap();
    let receipts = store.page_receipts(ACCOUNT, &manifest.episode_id, &manifest.revision).unwrap();
    assert_eq!(receipts.len(), selected.len());
    assert_eq!(receipts.iter().map(|receipt| receipt.page_index).collect::<Vec<_>>(), selected);
    for (receipt, &page_index) in receipts.iter().zip(selected) {
        let expected = &manifest.pages[page_index];
        assert_eq!(receipt.episode_id, manifest.episode_id);
        assert_eq!(receipt.revision, manifest.revision);
        assert_eq!(receipt.page_id, expected.id);
        assert_eq!(receipt.sha256, expected.digest);
        assert_eq!(receipt.bytes, expected.bytes.len() as u64);
        assert_eq!((receipt.width, receipt.height), (WIDTH, HEIGHT));
        assert_eq!(receipt.retained, retained);
        assert_eq!(receipt.decode_validation_version, crate::image_integrity::VALIDATION_VERSION);
        let bytes = fs::read(manager.pages_root().join(&receipt.file_name)).unwrap();
        assert_eq!(oracle_digest(&bytes), expected.digest);
        let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (WIDTH, HEIGHT));
        assert_eq!(decoded.as_raw(), &expected.pixels);
    }
    receipts
}

fn assert_complete(manager: &DownloadManager, manifest: &Manifest, task_id: &str) -> Vec<PageReceipt> {
    let task = stored_record(manager, task_id);
    assert_eq!(task.status, DownloadStatus::Completed);
    assert_eq!(task.total_pages as usize, manifest.pages.len());
    assert_eq!(task.completed_pages, task.total_pages);
    assert_eq!(task.bytes, manifest.pages.iter().map(|page| page.bytes.len() as u64).sum::<u64>());
    let receipts = assert_receipts(manager, manifest, &(0..manifest.pages.len()).collect::<Vec<_>>(), true);
    assert!(Store::open(manager.database_path()).unwrap().pending_page_commits(ACCOUNT).unwrap().is_empty());
    receipts
}

#[tokio::test]
async fn complete_chapter_requires_the_last_manifest_page_and_its_pixels() {
    for page_count in [21, 41, 57] {
        let directory = tempfile::tempdir().unwrap();
        let manifest = Manifest::new(&format!("held-tail-{page_count}"), page_count);
        let source = ManifestSource::new(manifest.clone(), Some(page_count - 1), false);
        let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
            source.clone(), Handle::current()).unwrap();
        let task = manager.enqueue(request(&manifest, true)).unwrap();
        let mut events = Vec::new();
        wait_until(|| {
            collect_events(&mut manager, &mut events);
            source.requested().contains(&(page_count - 1))
                && stored_record(&manager, &task.id).completed_pages as usize == page_count - 1
        }).await;
        assert_eq!(stored_record(&manager, &task.id).status, DownloadStatus::Running);
        assert!(!has_completion(&events, &task.id));
        assert_receipts(&manager, &manifest, &(0..page_count - 1).collect::<Vec<_>>(), true);
        source.release(1);
        wait_for_completion(&mut manager, &task.id, &mut events).await;
        assert_complete(&manager, &manifest, &task.id);
        let mut requested = source.requested();
        requested.sort_unstable();
        assert_eq!(requested, (0..page_count).collect::<Vec<_>>());
    }
}

#[tokio::test]
async fn last_page_failure_stays_incomplete_and_retry_fetches_only_the_missing_tail() {
    for page_count in [21, 41, 57] {
        let directory = tempfile::tempdir().unwrap();
        let manifest = Manifest::new(&format!("failed-tail-{page_count}"), page_count);
        let source = ManifestSource::new(manifest.clone(), Some(page_count - 1), true);
        let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
            source.clone(), Handle::current()).unwrap();
        let task = manager.enqueue(request(&manifest, true)).unwrap();
        let mut events = Vec::new();
        wait_until(|| {
            collect_events(&mut manager, &mut events);
            source.requested().contains(&(page_count - 1))
                && stored_record(&manager, &task.id).completed_pages as usize == page_count - 1
        }).await;
        assert_eq!(stored_record(&manager, &task.id).status, DownloadStatus::Running);
        assert!(!has_completion(&events, &task.id));
        source.release(1);
        wait_until(|| {
            collect_events(&mut manager, &mut events);
            stored_record(&manager, &task.id).status == DownloadStatus::Failed
                && events.iter().any(|event| matches!(&event.kind, DownloadEventKind::Failed { code, .. }
                    if code == "oracle_last_page_failed"))
        }).await;
        assert!(!has_completion(&events, &task.id));
        assert_eq!(stored_record(&manager, &task.id).completed_pages as usize, page_count - 1);
        assert_receipts(&manager, &manifest, &(0..page_count - 1).collect::<Vec<_>>(), true);
        let requested_before = source.requested().len();
        let prepared_before = source.prepared().len();
        source.state.fail_last.store(false, Ordering::SeqCst);
        let retry = manager.retry(&task.id).unwrap();
        assert_eq!(retry.id, task.id);
        let mut retry_events = Vec::new();
        wait_until(|| source.requested().len() > requested_before).await;
        assert_eq!(&source.requested()[requested_before..], &[page_count - 1]);
        assert_eq!(&source.prepared()[prepared_before..], &[vec![page_count - 1]]);
        assert_eq!(stored_record(&manager, &task.id).status, DownloadStatus::Running);
        collect_events(&mut manager, &mut retry_events);
        assert!(!has_completion(&retry_events, &task.id));
        source.release(1);
        wait_for_completion(&mut manager, &task.id, &mut retry_events).await;
        assert_complete(&manager, &manifest, &task.id);
        assert_eq!(&source.requested()[requested_before..], &[page_count - 1]);
    }
}

#[tokio::test]
async fn a_running_retained_prefix_expands_to_a_whole_chapter_but_reader_subsets_remain_partial() {
    for page_count in [21, 41, 57] {
        let directory = tempfile::tempdir().unwrap();
        let manifest = Manifest::new(&format!("expanded-prefix-{page_count}"), page_count);
        let source = ManifestSource::new(manifest.clone(), Some(2), false);
        let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
            source.clone(), Handle::current()).unwrap();
        let mut prefix = request(&manifest, true);
        prefix.page_indices = Some(vec![0, 1, 2]);
        let partial = manager.enqueue(prefix).unwrap();
        wait_until(|| source.requested().contains(&2)
            && stored_record(&manager, &partial.id).completed_pages == 2).await;
        assert_eq!(stored_record(&manager, &partial.id).status, DownloadStatus::Running);
        assert_receipts(&manager, &manifest, &[0, 1], true);
        let full = manager.enqueue(request(&manifest, true)).unwrap();
        assert_eq!(full.id, partial.id);
        assert!(full.generation > partial.generation);
        assert_eq!(full.completed_pages, 2);
        source.release(4);
        let mut events = Vec::new();
        wait_for_completion(&mut manager, &full.id, &mut events).await;
        assert_complete(&manager, &manifest, &full.id);

        let reader_directory = tempfile::tempdir().unwrap();
        let reader_manifest = Manifest::new(&format!("reader-subset-{page_count}"), page_count);
        let reader_source = ManifestSource::new(reader_manifest.clone(), None, false);
        let mut reader_manager = DownloadManager::new(reader_directory.path(), ACCOUNT.to_owned(), 1,
            reader_source.clone(), Handle::current()).unwrap();
        let selected = vec![0, page_count / 2, page_count - 1];
        let mut reader_request = request(&reader_manifest, false);
        reader_request.reader_generation = Some(700 + page_count as u64);
        reader_request.page_indices = Some(selected.clone());
        let reader = reader_manager.enqueue(reader_request).unwrap();
        let mut reader_events = Vec::new();
        wait_until(|| {
            collect_events(&mut reader_manager, &mut reader_events);
            stored_record(&reader_manager, &reader.id).status == DownloadStatus::Paused
                && reader_events.iter().any(|event| matches!(event.kind, DownloadEventKind::PrefetchReady))
        }).await;
        let record = stored_record(&reader_manager, &reader.id);
        assert_eq!(record.total_pages as usize, page_count);
        assert_eq!(record.completed_pages as usize, selected.len());
        assert!(!has_completion(&reader_events, &reader.id));
        assert_receipts(&reader_manager, &reader_manifest, &selected, false);
        let mut requested = reader_source.requested();
        requested.sort_unstable();
        assert_eq!(requested, selected);
    }
}

#[derive(Clone, Copy)]
enum TailDamage { Missing, Truncated }

impl TailDamage {
    fn apply(self, path: &Path, expected: &ExpectedPage) {
        match self {
            Self::Missing => fs::remove_file(path).unwrap(),
            Self::Truncated => {
                // Preserve the signature and dimensions while removing the end of the image body.
                let remaining = expected.bytes.len() / 2;
                assert!(remaining > 33 && remaining < expected.bytes.len());
                fs::write(path, &expected.bytes[..remaining]).unwrap();
            },
        }
    }
}

#[tokio::test]
async fn reopening_a_completed_chapter_repairs_only_a_missing_or_truncated_final_page() {
    for page_count in [21, 41, 57] {
        for damage in [TailDamage::Missing, TailDamage::Truncated] {
            let directory = tempfile::tempdir().unwrap();
            let manifest = Manifest::new(&format!("recovered-tail-{page_count}"), page_count);
            let source = ManifestSource::new(manifest.clone(), None, false);
            let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
                source.clone(), Handle::current()).unwrap();
            let task = manager.enqueue(request(&manifest, true)).unwrap();
            let mut events = Vec::new();
            wait_for_completion(&mut manager, &task.id, &mut events).await;
            let original = assert_complete(&manager, &manifest, &task.id);
            let tail = original.last().unwrap();
            let tail_path = manager.pages_root().join(&tail.file_name);
            let requested_before = source.requested().len();
            let prepared_before = source.prepared().len();
            drop(manager);
            damage.apply(&tail_path, manifest.pages.last().unwrap());
            let mut restored = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 2,
                source.clone(), Handle::current()).unwrap();
            restored.recover().unwrap();
            assert_ne!(stored_record(&restored, &task.id).status, DownloadStatus::Completed);
            assert_eq!(stored_record(&restored, &task.id).completed_pages as usize, page_count - 1);
            assert_receipts(&restored, &manifest, &(0..page_count - 1).collect::<Vec<_>>(), true);
            assert_eq!(source.requested().len(), requested_before);
            let retry = restored.retry(&task.id).unwrap();
            assert_eq!(retry.id, task.id);
            let mut retry_events = Vec::new();
            wait_for_completion(&mut restored, &task.id, &mut retry_events).await;
            let repaired = assert_complete(&restored, &manifest, &task.id);
            assert_eq!(&source.requested()[requested_before..], &[page_count - 1]);
            assert_eq!(&source.prepared()[prepared_before..], &[vec![page_count - 1]]);
            for (before, after) in original.iter().zip(&repaired).take(page_count - 1) {
                assert_eq!(after.page_id, before.page_id);
                assert_eq!(after.sha256, before.sha256);
                assert_eq!(after.file_name, before.file_name);
            }
        }
    }
}

fn seed_legacy_object(
    manager: &DownloadManager, manifest: &Manifest, bytes: &[u8], commit: bool,
) -> (DownloadRecord, PageReceipt) {
    assert_eq!(manifest.pages.len(), 1);
    let request = request(manifest, true);
    let record = DownloadRecord {
        id: super::request_id(ACCOUNT, &request), comic_id: request.comic_id.clone(),
        episode_id: manifest.episode_id.clone(), revision: manifest.revision.clone(),
        title: request.title.clone(), comic_title: request.comic_title.clone(),
        status: DownloadStatus::Running, total_pages: 1, generation: 1, retained: true,
        created_at: 1, updated_at: 1, ..Default::default()
    };
    let index = ImageIndex {
        episode_id: manifest.episode_id.clone(), revision: manifest.revision.clone(),
        images: vec![PageDescriptor {
            id: manifest.pages[0].id.clone(), index: 0, width: WIDTH, height: HEIGHT,
        }],
        ..Default::default()
    };
    let digest = oracle_digest(bytes);
    let receipt = PageReceipt {
        episode_id: manifest.episode_id.clone(), revision: manifest.revision.clone(),
        page_id: manifest.pages[0].id.clone(), page_index: 0,
        file_name: format!("{digest}.png"), sha256: digest, bytes: bytes.len() as u64,
        width: WIDTH, height: HEIGHT, retained: true, verified_at: 1,
        decode_validation_version: 0,
    };
    let mut store = Store::open(manager.database_path()).unwrap();
    store.save_index(ACCOUNT, &index).unwrap();
    store.save_download(ACCOUNT, &record).unwrap();
    // Raw publication intentionally models a receipt and journal written by the old verifier.
    store.begin_page_commit_generation(ACCOUNT, &record.id, record.generation, &receipt).unwrap();
    fs::write(manager.pages_root().join(&receipt.file_name), bytes).unwrap();
    if commit {
        store.commit_page_generation(ACCOUNT, &record.id, record.generation, &receipt).unwrap();
    }
    (store.load_download(ACCOUNT, &record.id).unwrap().unwrap(), receipt)
}

fn mark_legacy_completed(manager: &DownloadManager, record: &DownloadRecord) {
    assert_eq!(record.total_pages, 1);
    assert_eq!(record.completed_pages, record.total_pages);
    // Migration fixtures need the old persisted state that the strict Completed writer now rejects.
    let connection = rusqlite::Connection::open(manager.database_path()).unwrap();
    let changed = connection.execute(
        "UPDATE downloads SET status='completed',payload=json_set(payload,'$.status','completed')
         WHERE account_key=?1 AND task_id=?2",
        rusqlite::params![ACCOUNT, record.id],
    ).unwrap();
    assert_eq!(changed, 1);
}

fn stored_history(manager: &DownloadManager, manifest: &Manifest) -> Option<(String, u64, u32)> {
    let connection = rusqlite::Connection::open(manager.database_path()).unwrap();
    let value: Option<(String, i64, i64)> = connection.query_row(
        "SELECT sha256,bytes,decode_validation_version FROM page_identity_history
         WHERE account_key=?1 AND episode_id=?2 AND revision=?3 AND page_id=?4",
        rusqlite::params![ACCOUNT, manifest.episode_id, manifest.revision, manifest.pages[0].id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional().unwrap();
    value.map(|(digest, bytes, version)| {
        assert!(bytes >= 0 && version >= 0);
        (digest, bytes as u64, version as u32)
    })
}

fn assert_no_acquisition(source: &ManifestSource) {
    assert_eq!(source.state.index_calls.load(Ordering::SeqCst), 0);
    assert!(source.requested().is_empty());
    assert!(source.prepared().is_empty());
}

#[derive(Clone, Copy)]
enum LegacyCacheAction { Enqueue, RecoverRunning, RecoverCompleted }

#[tokio::test]
async fn valid_legacy_receipts_gain_decode_proofs_without_any_acquisition() {
    for action in [LegacyCacheAction::Enqueue, LegacyCacheAction::RecoverRunning, LegacyCacheAction::RecoverCompleted] {
        let directory = tempfile::tempdir().unwrap();
        let manifest = Manifest::new("legacy-valid-cache", 1);
        let source = ManifestSource::new(manifest.clone(), None, false);
        let manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
            source.clone(), Handle::current()).unwrap();
        let (mut original, receipt) = seed_legacy_object(&manager, &manifest, &manifest.pages[0].bytes, true);
        assert_eq!(original.status, DownloadStatus::Running);
        assert_eq!(receipt.decode_validation_version, 0);
        assert_eq!(stored_history(&manager, &manifest), Some((receipt.sha256.clone(), receipt.bytes, 0)));
        match action {
            LegacyCacheAction::Enqueue => {
                original.status = DownloadStatus::Paused;
                Store::open(manager.database_path()).unwrap().save_download(ACCOUNT, &original).unwrap();
            },
            LegacyCacheAction::RecoverRunning => {},
            LegacyCacheAction::RecoverCompleted => mark_legacy_completed(&manager, &original),
        }
        drop(manager);
        let mut restored = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 2,
            source.clone(), Handle::current()).unwrap();
        let mut events = Vec::new();
        match action {
            LegacyCacheAction::Enqueue => {
                let task = restored.enqueue(request(&manifest, true)).unwrap();
                assert_eq!(task.id, original.id);
                wait_for_completion(&mut restored, &task.id, &mut events).await;
            },
            LegacyCacheAction::RecoverRunning => {
                restored.recover().unwrap();
                wait_for_completion(&mut restored, &original.id, &mut events).await;
            },
            LegacyCacheAction::RecoverCompleted => restored.recover().unwrap(),
        }
        assert_complete(&restored, &manifest, &original.id);
        assert_eq!(stored_history(&restored, &manifest), Some((receipt.sha256, receipt.bytes,
            crate::image_integrity::VALIDATION_VERSION)));
        assert_no_acquisition(&source);
    }
}

fn empty_pixel_stream_png() -> Vec<u8> {
    fn chunk(output: &mut Vec<u8>, kind: &[u8; 4], bytes: &[u8]) {
        output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        output.extend_from_slice(kind);
        output.extend_from_slice(bytes);
        let mut crc = crc32fast::Hasher::new();
        crc.update(kind);
        crc.update(bytes);
        output.extend_from_slice(&crc.finalize().to_be_bytes());
    }
    let mut output = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&WIDTH.to_be_bytes());
    header.extend_from_slice(&HEIGHT.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut output, b"IHDR", &header);
    // This is a complete empty zlib stream, with no scanlines for the declared dimensions.
    chunk(&mut output, b"IDAT", &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]);
    chunk(&mut output, b"IEND", &[]);
    output
}

#[tokio::test]
async fn invalid_legacy_encoding_can_replace_only_its_untrusted_historical_digest() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = Manifest::new("legacy-invalid-body", 1);
    let source = ManifestSource::new(manifest.clone(), None, false);
    let invalid = empty_pixel_stream_png();
    let dimensions = image::ImageReader::with_format(Cursor::new(&invalid), image::ImageFormat::Png)
        .into_dimensions().unwrap();
    assert_eq!(dimensions, (WIDTH, HEIGHT));
    assert_ne!(oracle_digest(&invalid), manifest.pages[0].digest);
    let manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    let (original, receipt) = seed_legacy_object(&manager, &manifest, &invalid, true);
    assert_eq!(stored_history(&manager, &manifest), Some((receipt.sha256.clone(), receipt.bytes, 0)));
    mark_legacy_completed(&manager, &original);
    drop(manager);
    let mut restored = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 2,
        source.clone(), Handle::current()).unwrap();
    restored.recover().unwrap();
    assert_eq!(stored_record(&restored, &original.id).status, DownloadStatus::Failed);
    assert!(stored_history(&restored, &manifest).is_none());
    assert!(Store::open(restored.database_path()).unwrap()
        .page_receipt(ACCOUNT, &manifest.episode_id, &manifest.revision, &receipt.page_id).unwrap().is_none());
    assert_no_acquisition(&source);
    restored.retry(&original.id).unwrap();
    let mut events = Vec::new();
    wait_for_completion(&mut restored, &original.id, &mut events).await;
    assert_complete(&restored, &manifest, &original.id);
    assert_eq!(source.requested(), vec![0]);
    assert_eq!(source.prepared(), vec![vec![0]]);
    assert_eq!(stored_history(&restored, &manifest), Some((manifest.pages[0].digest.clone(),
        manifest.pages[0].bytes.len() as u64, crate::image_integrity::VALIDATION_VERSION)));
}

#[tokio::test]
async fn valid_legacy_pending_journals_are_reproven_and_committed_without_acquisition() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = Manifest::new("legacy-pending-object", 1);
    let source = ManifestSource::new(manifest.clone(), None, false);
    let manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    let (original, receipt) = seed_legacy_object(&manager, &manifest, &manifest.pages[0].bytes, false);
    let store = Store::open(manager.database_path()).unwrap();
    assert!(store.page_receipt(ACCOUNT, &manifest.episode_id, &manifest.revision, &receipt.page_id).unwrap().is_none());
    let pending = store.pending_page_commits(ACCOUNT).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].generation, original.generation);
    assert_eq!(pending[0].receipt.decode_validation_version, 0);
    assert!(stored_history(&manager, &manifest).is_none());
    drop(store);
    drop(manager);
    let mut restored = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 2,
        source.clone(), Handle::current()).unwrap();
    restored.recover().unwrap();
    let mut events = Vec::new();
    wait_for_completion(&mut restored, &original.id, &mut events).await;
    assert_complete(&restored, &manifest, &original.id);
    assert_eq!(stored_history(&restored, &manifest), Some((receipt.sha256, receipt.bytes,
        crate::image_integrity::VALIDATION_VERSION)));
    assert_no_acquisition(&source);
}

#[tokio::test]
async fn disk_hash_damage_does_not_authorize_replacing_a_legacy_content_identity() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = Manifest::new("legacy-disk-damage", 1);
    let different_pixels = Manifest::new("replacement-pixel-template", 2);
    let mut replacement = different_pixels.pages[1].clone();
    replacement.id = manifest.pages[0].id.clone();
    let replacement_manifest = Arc::new(Manifest {
        episode_id: manifest.episode_id.clone(), revision: manifest.revision.clone(), pages: vec![replacement],
    });
    assert_ne!(replacement_manifest.pages[0].digest, manifest.pages[0].digest);
    let source = ManifestSource::new(replacement_manifest, None, false);
    let manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    let (original, receipt) = seed_legacy_object(&manager, &manifest, &manifest.pages[0].bytes, true);
    mark_legacy_completed(&manager, &original);
    let mut damaged = manifest.pages[0].bytes.clone();
    let position = damaged.len() / 2;
    damaged[position] ^= 0x40;
    assert_ne!(oracle_digest(&damaged), receipt.sha256);
    fs::write(manager.pages_root().join(&receipt.file_name), damaged).unwrap();
    drop(manager);
    let mut restored = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 2,
        source.clone(), Handle::current()).unwrap();
    restored.recover().unwrap();
    assert_eq!(stored_record(&restored, &original.id).status, DownloadStatus::Failed);
    assert_eq!(stored_history(&restored, &manifest), Some((receipt.sha256.clone(), receipt.bytes, 0)));
    assert_no_acquisition(&source);
    restored.retry(&original.id).unwrap();
    let mut events = Vec::new();
    wait_until(|| {
        collect_events(&mut restored, &mut events);
        source.requested() == vec![0]
            && stored_record(&restored, &original.id).status == DownloadStatus::Failed
            && events.iter().any(|event| matches!(event.kind, DownloadEventKind::Failed { .. }))
    }).await;
    assert!(!has_completion(&events, &original.id));
    assert_eq!(stored_record(&restored, &original.id).completed_pages, 0);
    assert_eq!(stored_history(&restored, &manifest), Some((receipt.sha256, receipt.bytes, 0)));
    assert!(Store::open(restored.database_path()).unwrap()
        .page_receipt(ACCOUNT, &manifest.episode_id, &manifest.revision, &manifest.pages[0].id).unwrap().is_none());
}

struct RevisionSource {
    versions: Vec<Arc<ManifestSource>>,
    current: AtomicUsize,
    readable: AtomicBool,
    checks: Mutex<Vec<(String, String)>>,
    check_gate: Semaphore,
    legacy_calls: AtomicUsize,
}

impl RevisionSource {
    fn new(manifests: Vec<Arc<Manifest>>) -> Arc<Self> {
        assert!(!manifests.is_empty());
        Arc::new(Self {
            versions: manifests.into_iter().map(|manifest| ManifestSource::new(manifest, None, false)).collect(),
            current: AtomicUsize::new(0), readable: AtomicBool::new(true),
            checks: Mutex::new(Vec::new()), check_gate: Semaphore::new(1), legacy_calls: AtomicUsize::new(0),
        })
    }

    fn select_revision(&self, position: usize) {
        assert!(position < self.versions.len());
        self.current.store(position, Ordering::SeqCst);
    }

    fn checks(&self) -> Vec<(String, String)> { self.checks.lock().unwrap().clone() }

    fn release_check(&self) { self.check_gate.add_permits(1); }

    fn acquisition_counts(&self) -> Vec<(usize, usize, usize)> {
        self.versions.iter().map(|source| (
            source.state.index_calls.load(Ordering::SeqCst), source.prepared().len(), source.requested().len(),
        )).collect()
    }
}

impl AcquisitionSource for RevisionSource {
    fn image_index<'a>(&'a self, _episode_id: &'a str) -> AcquisitionFuture<'a, ImageIndex> {
        Box::pin(async move {
            self.legacy_calls.fetch_add(1, Ordering::SeqCst);
            Err(AcquisitionFailure { code: "oracle_context_free_index_used".to_owned(), retryable: false })
        })
    }

    fn image_index_for<'a>(&'a self, comic_id: &'a str, episode_id: &'a str) -> AcquisitionFuture<'a, ImageIndex> {
        Box::pin(async move {
            self.checks.lock().unwrap().push((comic_id.to_owned(), episode_id.to_owned()));
            let permit = self.check_gate.acquire().await.map_err(|_| AcquisitionFailure {
                code: "oracle_source_check_gate_closed".to_owned(), retryable: false,
            })?;
            permit.forget();
            // Permission and version are read after the gate, so revocation may precede execution.
            if !self.readable.load(Ordering::SeqCst) {
                return Err(AcquisitionFailure { code: "oracle_permission_denied".to_owned(), retryable: false });
            }
            let source = self.versions[self.current.load(Ordering::SeqCst)].clone();
            if episode_id != source.manifest.episode_id || comic_id != format!("comic-{}", source.manifest.episode_id) {
                return Err(AcquisitionFailure { code: "oracle_source_context_mismatch".to_owned(), retryable: false });
            }
            source.image_index(episode_id).await
        })
    }

    fn download_page<'a>(&'a self, _index: &'a ImageIndex, _page_index: usize)
        -> AcquisitionFuture<'a, AcquiredPage>
    {
        Box::pin(async move {
            self.legacy_calls.fetch_add(1, Ordering::SeqCst);
            Err(AcquisitionFailure { code: "oracle_context_free_page_used".to_owned(), retryable: false })
        })
    }

    fn page_batch_size(&self) -> usize { 20 }

    fn prepare_page_batch<'a>(&'a self, index: &'a ImageIndex, page_indices: &'a [usize])
        -> AcquisitionFuture<'a, Arc<dyn PreparedPageBatch>>
    {
        Box::pin(async move {
            // Each prepared plan keeps the immutable revision that produced its source descriptor.
            let source = self.versions.iter().find(|source| source.manifest.revision == index.revision)
                .cloned().ok_or_else(|| AcquisitionFailure {
                    code: "oracle_unregistered_revision".to_owned(), retryable: false,
                })?;
            source.prepare_page_batch(index, page_indices).await
        })
    }
}

fn latest_request(manifest: &Manifest) -> DownloadRequest {
    let mut latest = request(manifest, true);
    latest.expected_revision = None;
    latest
}

fn assert_revision_unchanged(
    manager: &DownloadManager, manifest: &Manifest, index: &ImageIndex, receipts: &[PageReceipt],
) {
    let store = Store::open(manager.database_path()).unwrap();
    let current = store.load_index(ACCOUNT, &manifest.episode_id, Some(&manifest.revision)).unwrap().unwrap();
    assert_eq!(&current, index);
    let checked = assert_receipts(manager, manifest, &(0..manifest.pages.len()).collect::<Vec<_>>(), true);
    assert_eq!(checked.as_slice(), receipts);
}

async fn wait_for_permission_failure(
    manager: &mut DownloadManager, task_id: &str, events: &mut Vec<DownloadEvent>,
) {
    wait_until(|| {
        collect_events(manager, events);
        let task = stored_record(manager, task_id);
        task.status == DownloadStatus::Failed && task.error_code.as_deref() == Some("oracle_permission_denied")
            && events.iter().any(|event| matches!(&event.kind, DownloadEventKind::Failed { code, .. }
                if event.task_id == task_id && code == "oracle_permission_denied"))
    }).await;
    assert!(!has_completion(events, task_id));
    assert!(stored_record(manager, task_id).refresh_source);
}

#[tokio::test]
async fn explicit_fresh_acquisition_keeps_the_old_checkpoint_and_downloads_the_new_complete_revision() {
    for pinned_checkpoint in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let old = Manifest::versioned("revised-chapter", "revised-chapter-v1", 21, 0);
        let new = Manifest::versioned("revised-chapter", "revised-chapter-v2", 57, 128);
        assert_ne!(old.pages[0].digest, new.pages[0].digest);
        let source = RevisionSource::new(vec![old.clone(), new.clone()]);
        let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
            source.clone(), Handle::current()).unwrap();
        let mut initial = latest_request(&old);
        if pinned_checkpoint { initial.expected_revision = Some(old.revision.clone()); }
        let original = manager.enqueue(initial).unwrap();
        manager.prioritize(&original.id).unwrap();
        let mut original_events = Vec::new();
        wait_for_completion(&mut manager, &original.id, &mut original_events).await;
        let old_receipts = assert_complete(&manager, &old, &original.id);
        let old_record = stored_record(&manager, &original.id);
        let old_priority = old_record.priority;
        assert!(old_priority > 0);
        let old_index = Store::open(manager.database_path()).unwrap()
            .load_index(ACCOUNT, &old.episode_id, Some(&old.revision)).unwrap().unwrap();
        let old_activity = source.versions[0].requested();
        source.select_revision(1);
        source.release_check();
        let canonical = latest_request(&new);
        let canonical_id = super::request_id(ACCOUNT, &canonical);
        let fresh = manager.enqueue_fresh(canonical).unwrap();
        assert_eq!(fresh.id, canonical_id);
        let expected_records = if pinned_checkpoint { 2 } else { 1 };
        if pinned_checkpoint {
            assert_ne!(fresh.id, original.id);
            assert_eq!(stored_record(&manager, &original.id), old_record);
        } else {
            assert_eq!(fresh.id, original.id);
            assert!(fresh.generation > original.generation);
            assert_eq!(fresh.priority, old_priority);
        }
        assert!(fresh.refresh_source);
        assert_eq!(manager.records().unwrap().len(), expected_records);
        let mut fresh_events = Vec::new();
        wait_for_completion(&mut manager, &fresh.id, &mut fresh_events).await;
        assert_eq!(stored_record(&manager, &fresh.id).revision, new.revision);
        assert_complete(&manager, &new, &fresh.id);
        assert_eq!(manager.records().unwrap().len(), expected_records);
        if pinned_checkpoint {
            let preserved = stored_record(&manager, &original.id);
            assert_eq!(preserved, old_record);
            assert_eq!(preserved.generation, original.generation);
            assert_eq!(preserved.priority, old_priority);
            assert_eq!(preserved.revision, old.revision);
            assert_complete(&manager, &old, &original.id);
        }
        assert_revision_unchanged(&manager, &old, &old_index, &old_receipts);
        assert_eq!(source.versions[0].requested(), old_activity);
        let mut new_requests = source.versions[1].requested();
        new_requests.sort_unstable();
        assert_eq!(new_requests, (0..57).collect::<Vec<_>>());
        assert_eq!(source.checks(), vec![
            (format!("comic-{}", old.episode_id), old.episode_id.clone()),
            (format!("comic-{}", new.episode_id), new.episode_id.clone()),
        ]);
        assert_eq!(source.legacy_calls.load(Ordering::SeqCst), 0);
        let mut forbidden_prefix = old_index.clone();
        forbidden_prefix.images.pop();
        assert!(Store::open(manager.database_path()).unwrap().save_index(ACCOUNT, &forbidden_prefix).is_err());
        assert_revision_unchanged(&manager, &old, &old_index, &old_receipts);
    }
}

#[tokio::test]
async fn fresh_permission_failures_never_complete_from_cache_and_remain_fresh_after_retry_and_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = Manifest::versioned("permission-chapter", "permission-chapter-v1", 21, 0);
    let source = RevisionSource::new(vec![manifest.clone()]);
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    let original = manager.enqueue(latest_request(&manifest)).unwrap();
    let mut original_events = Vec::new();
    wait_for_completion(&mut manager, &original.id, &mut original_events).await;
    let old_receipts = assert_complete(&manager, &manifest, &original.id);
    let old_index = Store::open(manager.database_path()).unwrap()
        .load_index(ACCOUNT, &manifest.episode_id, Some(&manifest.revision)).unwrap().unwrap();
    let old_activity = source.acquisition_counts();
    let fresh = manager.enqueue_fresh(latest_request(&manifest)).unwrap();
    assert_eq!(fresh.id, original.id);
    assert!(fresh.generation > original.generation && fresh.refresh_source);
    let mut fresh_events = Vec::new();
    wait_until(|| {
        collect_events(&mut manager, &mut fresh_events);
        source.checks().len() == 2 || has_completion(&fresh_events, &fresh.id)
    }).await;
    assert_eq!(source.checks().len(), 2);
    assert_ne!(stored_record(&manager, &fresh.id).status, DownloadStatus::Completed);
    assert!(!has_completion(&fresh_events, &fresh.id));
    assert_eq!(source.acquisition_counts(), old_activity);
    source.readable.store(false, Ordering::SeqCst);
    source.release_check();
    wait_for_permission_failure(&mut manager, &fresh.id, &mut fresh_events).await;
    assert_eq!(source.acquisition_counts(), old_activity);
    assert_revision_unchanged(&manager, &manifest, &old_index, &old_receipts);
    assert!(Store::open(manager.database_path()).unwrap().pending_page_commits(ACCOUNT).unwrap().is_empty());

    let retry = manager.retry(&fresh.id).unwrap();
    assert!(retry.generation > fresh.generation && retry.refresh_source);
    let mut retry_events = Vec::new();
    wait_until(|| {
        collect_events(&mut manager, &mut retry_events);
        source.checks().len() == 3 || has_completion(&retry_events, &retry.id)
    }).await;
    assert_eq!(source.checks().len(), 3);
    assert_ne!(stored_record(&manager, &retry.id).status, DownloadStatus::Completed);
    source.release_check();
    wait_for_permission_failure(&mut manager, &retry.id, &mut retry_events).await;
    assert_eq!(source.acquisition_counts(), old_activity);

    // Leave another fresh permission check pending to model interruption before its execution.
    let interrupted = manager.retry(&retry.id).unwrap();
    assert!(interrupted.refresh_source);
    wait_until(|| source.checks().len() == 4).await;
    assert_ne!(stored_record(&manager, &interrupted.id).status, DownloadStatus::Completed);
    drop(manager);
    let mut restored = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 2,
        source.clone(), Handle::current()).unwrap();
    restored.recover().unwrap();
    assert!(stored_record(&restored, &interrupted.id).refresh_source);
    let mut recovery_events = Vec::new();
    wait_until(|| {
        collect_events(&mut restored, &mut recovery_events);
        source.checks().len() == 5 || has_completion(&recovery_events, &interrupted.id)
    }).await;
    assert_eq!(source.checks().len(), 5);
    assert_ne!(stored_record(&restored, &interrupted.id).status, DownloadStatus::Completed);
    source.release_check();
    wait_for_permission_failure(&mut restored, &interrupted.id, &mut recovery_events).await;
    assert_eq!(source.acquisition_counts(), old_activity);
    assert_eq!(source.legacy_calls.load(Ordering::SeqCst), 0);
    assert!(source.checks().iter().all(|(comic_id, episode_id)|
        comic_id == &format!("comic-{}", manifest.episode_id) && episode_id == &manifest.episode_id));
    assert_revision_unchanged(&restored, &manifest, &old_index, &old_receipts);
    assert!(Store::open(restored.database_path()).unwrap().pending_page_commits(ACCOUNT).unwrap().is_empty());
}

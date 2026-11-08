use super::*;
use crate::domain::PageDescriptor;
use std::{sync::atomic::AtomicUsize, time::Duration};
use tokio::sync::Semaphore;

const ACCOUNT: &str = "fixture-account";
const PRIVATE_MARKER: &str = "fixture-private-acquisition-key";

struct BatchFixture {
    state: Arc<FixtureState>,
    page_count: usize,
}

struct FixtureState {
    index_calls: AtomicUsize,
    preparations: Mutex<Vec<(String, Vec<usize>)>>,
    pages: Mutex<Vec<(String, usize)>>,
    active: AtomicUsize,
    maximum: AtomicUsize,
    live_batches: AtomicUsize,
    dropped_batches: AtomicUsize,
    cancelled_pages: AtomicUsize,
    prepare_gate: Semaphore,
    page_gate: Semaphore,
    bytes: Vec<Vec<u8>>,
}

impl BatchFixture {
    fn new(page_count: usize, preparations: usize, pages: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Arc::new(FixtureState {
                index_calls: AtomicUsize::new(0), preparations: Mutex::new(Vec::new()),
                pages: Mutex::new(Vec::new()), active: AtomicUsize::new(0), maximum: AtomicUsize::new(0),
                live_batches: AtomicUsize::new(0), dropped_batches: AtomicUsize::new(0),
                cancelled_pages: AtomicUsize::new(0),
                prepare_gate: Semaphore::new(preparations), page_gate: Semaphore::new(pages),
                bytes: (0..page_count).map(|page_index| {
                    let tone = (page_index + 16) as u8;
                    let image = image::RgbaImage::from_pixel(8, 12, image::Rgba([tone, tone, tone, 255]));
                    let mut output = Cursor::new(Vec::new());
                    image::DynamicImage::ImageRgba8(image).write_to(&mut output, ImageFormat::Png).unwrap();
                    output.into_inner()
                }).collect(),
            }),
            page_count,
        })
    }
}

struct Activity(Arc<FixtureState>);

impl Activity {
    fn start(state: &Arc<FixtureState>) -> Self {
        let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
        state.maximum.fetch_max(active, Ordering::SeqCst);
        Self(state.clone())
    }
}

impl Drop for Activity {
    fn drop(&mut self) { self.0.active.fetch_sub(1, Ordering::SeqCst); }
}

struct PageActivity {
    activity: Activity,
    completed: bool,
}

impl Drop for PageActivity {
    fn drop(&mut self) {
        if !self.completed { self.activity.0.cancelled_pages.fetch_add(1, Ordering::SeqCst); }
    }
}

struct FixtureBatch {
    state: Arc<FixtureState>,
    episode_id: String,
    page_indices: HashSet<usize>,
    private_key: String,
}

impl Drop for FixtureBatch {
    fn drop(&mut self) {
        self.state.live_batches.fetch_sub(1, Ordering::SeqCst);
        self.state.dropped_batches.fetch_add(1, Ordering::SeqCst);
    }
}

impl PreparedPageBatch for FixtureBatch {
    fn download_page(&self, page_index: usize) -> AcquisitionFuture<'_, AcquiredPage> {
        Box::pin(async move {
            if !self.page_indices.contains(&page_index) || self.private_key != PRIVATE_MARKER {
                return Err(AcquisitionFailure::new("fixture_batch_page_invalid", false));
            }
            let mut activity = PageActivity { activity: Activity::start(&self.state), completed: false };
            self.state.pages.lock().unwrap().push((self.episode_id.clone(), page_index));
            let permit = self.state.page_gate.acquire().await
                .map_err(|_| AcquisitionFailure::new("fixture_page_gate_closed", false))?;
            permit.forget();
            activity.completed = true;
            Ok(AcquiredPage { bytes: self.state.bytes[page_index].clone(), width: 8, height: 12, sha256: None })
        })
    }
}

impl AcquisitionSource for BatchFixture {
    fn image_index<'a>(&'a self, episode_id: &'a str) -> AcquisitionFuture<'a, ImageIndex> {
        Box::pin(async move {
            let _activity = Activity::start(&self.state);
            self.state.index_calls.fetch_add(1, Ordering::SeqCst);
            Ok(ImageIndex {
                episode_id: episode_id.to_owned(), revision: format!("{episode_id}-revision"),
                images: (0..self.page_count).map(|index| PageDescriptor {
                    id: format!("{episode_id}-page-{index}"), index, width: 8, height: 12,
                }).collect(),
                paths: (0..self.page_count).map(|index| format!("{PRIVATE_MARKER}-{index}")).collect(),
                extra: serde_json::json!({ "fixture_private_marker": PRIVATE_MARKER }),
            })
        })
    }

    fn download_page<'a>(&'a self, _index: &'a ImageIndex, _page_index: usize)
        -> AcquisitionFuture<'a, AcquiredPage>
    {
        Box::pin(async { Err(AcquisitionFailure::new("fixture_legacy_path_used", false)) })
    }

    fn page_batch_size(&self) -> usize { MAX_PAGE_BATCH }

    fn prepare_page_batch<'a>(&'a self, index: &'a ImageIndex, page_indices: &'a [usize])
        -> AcquisitionFuture<'a, Arc<dyn PreparedPageBatch>>
    {
        Box::pin(async move {
            let _activity = Activity::start(&self.state);
            assert!(!page_indices.is_empty() && page_indices.len() <= MAX_PAGE_BATCH);
            self.state.preparations.lock().unwrap().push((index.episode_id.clone(), page_indices.to_vec()));
            let permit = self.state.prepare_gate.acquire().await
                .map_err(|_| AcquisitionFailure::new("fixture_prepare_gate_closed", false))?;
            permit.forget();
            self.state.live_batches.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(FixtureBatch {
                state: self.state.clone(), episode_id: index.episode_id.clone(),
                page_indices: page_indices.iter().copied().collect(), private_key: PRIVATE_MARKER.to_owned(),
            }) as Arc<dyn PreparedPageBatch>)
        })
    }
}

fn request(episode_id: &str, retain: bool) -> DownloadRequest {
    DownloadRequest {
        comic_id: format!("comic-{episode_id}"), episode_id: episode_id.to_owned(),
        title: episode_id.to_owned(), comic_title: "Fixture comic".to_owned(), retain,
        ..Default::default()
    }
}

fn record(root: &Path, task_id: &str) -> DownloadRecord {
    Store::open(root.join("state.sqlite3")).unwrap().load_download(ACCOUNT, task_id).unwrap().unwrap()
}

async fn wait_until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if predicate() { return; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("the controlled batch did not reach its expected state");
}

#[tokio::test]
async fn forty_five_missing_pages_use_three_batches_and_publish_each_commit() {
    let directory = tempfile::tempdir().unwrap();
    let source = BatchFixture::new(45, 10, 0);
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    let task = manager.enqueue(request("chapter", true)).unwrap();
    wait_until(|| source.state.pages.lock().unwrap().len() == 2).await;
    assert_eq!(source.state.preparations.lock().unwrap()[0].1, (0..20).collect::<Vec<_>>());
    let mut events = manager.drain_events(100);
    source.state.page_gate.add_permits(1);
    wait_until(|| {
        events.extend(manager.drain_events(100));
        events.iter().any(|event| matches!(event.kind, DownloadEventKind::PageReady { completed_pages: 1, .. }))
    }).await;
    assert_eq!(source.state.preparations.lock().unwrap().len(), 1);
    assert_eq!(record(directory.path(), &task.id).status, DownloadStatus::Running);
    source.state.page_gate.add_permits(44);
    wait_until(|| {
        events.extend(manager.drain_events(100));
        events.iter().any(|event| matches!(event.kind, DownloadEventKind::Completed))
    }).await;
    let batches = source.state.preparations.lock().unwrap().clone();
    assert_eq!(batches.iter().map(|(_, pages)| pages.clone()).collect::<Vec<_>>(), vec![
        (0..20).collect::<Vec<_>>(), (20..40).collect::<Vec<_>>(), (40..45).collect::<Vec<_>>(),
    ]);
    let mut pages = Vec::new();
    let mut progress = Vec::new();
    for event in events {
        if let DownloadEventKind::PageReady { page_index, completed_pages, .. } = event.kind {
            pages.push(page_index); progress.push(completed_pages);
        }
    }
    pages.sort_unstable(); progress.sort_unstable();
    assert_eq!(pages, (0..45).collect::<Vec<_>>());
    assert_eq!(progress, (1..=45).collect::<Vec<_>>());
    assert_eq!(source.state.pages.lock().unwrap().len(), 45);
    assert_eq!(source.state.maximum.load(Ordering::SeqCst), 2);
    assert_eq!(source.state.live_batches.load(Ordering::SeqCst), 0);
    assert_eq!(source.state.dropped_batches.load(Ordering::SeqCst), 3);
    let completed = record(directory.path(), &task.id);
    assert_eq!(completed.completed_pages, 45);
    let store = Store::open(manager.database_path()).unwrap();
    assert!(store.pending_page_commits(ACCOUNT).unwrap().is_empty());
    let stored_index = store.load_index(ACCOUNT, "chapter", Some("chapter-revision")).unwrap().unwrap();
    assert!(stored_index.paths.is_empty() && stored_index.extra.is_null());
    for entry in fs::read_dir(directory.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            assert!(!String::from_utf8_lossy(&fs::read(path).unwrap()).contains(PRIVATE_MARKER));
        }
    }
}

#[tokio::test]
async fn selection_deduplicates_cached_pages_and_repairs_only_corrupt_receipts() {
    let directory = tempfile::tempdir().unwrap();
    let source = BatchFixture::new(6, 20, 20);
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    let mut subset = request("cached", false);
    subset.expected_revision = Some("cached-revision".to_owned());
    subset.reader_generation = Some(7);
    subset.page_indices = Some(vec![4, 1, 1]);
    let first = manager.enqueue(subset.clone()).unwrap();
    wait_until(|| record(directory.path(), &first.id).status == DownloadStatus::Paused).await;
    subset.page_indices = Some(vec![4, 3, 1, 3]);
    let second = manager.enqueue(subset.clone()).unwrap();
    wait_until(|| record(directory.path(), &second.id).status == DownloadStatus::Paused).await;
    let store = Store::open(manager.database_path()).unwrap();
    let receipt = store.page_receipt(ACCOUNT, "cached", "cached-revision", "cached-page-3").unwrap().unwrap();
    fs::write(manager.pages_root().join(&receipt.file_name), b"corrupt fixture image").unwrap();
    let third = manager.enqueue(subset).unwrap();
    wait_until(|| record(directory.path(), &third.id).status == DownloadStatus::Paused).await;
    assert!(verify_receipt_file(manager.pages_root(), &receipt).unwrap());
    let retained = manager.enqueue(request("cached", true)).unwrap();
    wait_until(|| record(directory.path(), &retained.id).status == DownloadStatus::Completed).await;
    assert_eq!(source.state.preparations.lock().unwrap().iter()
        .map(|(_, pages)| pages.clone()).collect::<Vec<_>>(), vec![vec![1, 4], vec![3], vec![3], vec![0, 2, 5]]);
    assert_eq!(source.state.pages.lock().unwrap().len(), 7);
    let receipts = store.page_receipts(ACCOUNT, "cached", "cached-revision").unwrap();
    assert_eq!(receipts.len(), 6);
    assert!(receipts.iter().all(|receipt| receipt.retained && verify_receipt_file(manager.pages_root(), receipt).unwrap()));
    let indices = source.state.index_calls.load(Ordering::SeqCst);
    let preparations = source.state.preparations.lock().unwrap().len();
    manager.retry(&retained.id).unwrap();
    wait_until(|| record(directory.path(), &retained.id).status == DownloadStatus::Completed).await;
    assert_eq!(source.state.index_calls.load(Ordering::SeqCst), indices);
    assert_eq!(source.state.preparations.lock().unwrap().len(), preparations);
}

#[tokio::test]
async fn metadata_and_images_share_capacity_across_chapters_and_reader_prefetch() {
    for limit in [1, 2, 4] {
        let directory = tempfile::tempdir().unwrap();
        let source = BatchFixture::new(5, 20, 0);
        let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
            source.clone(), Handle::current()).unwrap();
        manager.set_concurrency(limit).unwrap();
        let first = manager.enqueue(request("first", true)).unwrap();
        wait_until(|| source.state.pages.lock().unwrap().len() == limit).await;
        let second = manager.enqueue(request("second", true)).unwrap();
        let third = manager.enqueue(request("third", true)).unwrap();
        let mut reader = request("reader", false);
        reader.reader_generation = Some(9);
        reader.expected_revision = Some("reader-revision".to_owned());
        reader.page_indices = Some(vec![2, 0]);
        let reader = manager.enqueue(reader).unwrap();
        wait_until(|| manager.context.capacity.lock().unwrap().waiters.iter()
            .filter(|waiter| waiter.kind == SlotKind::Preparation).count() == 3).await;
        assert_eq!(source.state.index_calls.load(Ordering::SeqCst), 1, "limit {limit}");
        assert_eq!(source.state.preparations.lock().unwrap().len(), 1, "limit {limit}");
        source.state.page_gate.add_permits(20);
        wait_until(|| [&first.id, &second.id, &third.id].iter()
            .all(|id| record(directory.path(), id).status == DownloadStatus::Completed)
            && record(directory.path(), &reader.id).status == DownloadStatus::Paused).await;
        assert_eq!(source.state.maximum.load(Ordering::SeqCst), limit, "limit {limit}");
        assert_eq!(source.state.pages.lock().unwrap().len(), 17, "limit {limit}");
        assert_eq!(source.state.preparations.lock().unwrap().iter()
            .find(|(episode, _)| episode == "reader").unwrap().1, vec![0, 2]);
    }
}

#[tokio::test]
async fn cancellation_transport_pause_and_account_change_stop_preparation_before_cdn_or_commit() {
    for mode in ["cancel", "suspend", "account"] {
        let directory = tempfile::tempdir().unwrap();
        let source = BatchFixture::new(3, 0, 0);
        let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
            source.clone(), Handle::current()).unwrap();
        manager.set_concurrency(1).unwrap();
        let task = manager.enqueue(request("interrupted", true)).unwrap();
        wait_until(|| source.state.preparations.lock().unwrap().len() == 1).await;
        match mode {
            "cancel" => manager.cancel(&task.id).unwrap(),
            "suspend" => manager.pause_handle().suspend(),
            "account" => manager.invalidate_account(2).unwrap(),
            _ => unreachable!(),
        }
        wait_until(|| manager.context.capacity.lock().unwrap().active == 0
            && source.state.active.load(Ordering::SeqCst) == 0).await;
        assert!(manager.context.capacity.lock().unwrap().waiters.is_empty(), "{mode}");
        source.state.prepare_gate.add_permits(1);
        source.state.page_gate.add_permits(3);
        assert!(source.state.pages.lock().unwrap().is_empty(), "{mode}");
        assert_eq!(source.state.live_batches.load(Ordering::SeqCst), 0, "{mode}");
        let store = Store::open(manager.database_path()).unwrap();
        assert!(store.page_receipts(ACCOUNT, "interrupted", "interrupted-revision").unwrap().is_empty(), "{mode}");
        assert!(store.pending_page_commits(ACCOUNT).unwrap().is_empty(), "{mode}");
        assert!(manager.drain_events(100).iter().all(|event| !matches!(event.kind,
            DownloadEventKind::PageReady { .. } | DownloadEventKind::Failed { .. } | DownloadEventKind::Completed)), "{mode}");
    }
}

fn waiting_for(manager: &DownloadManager, task_id: &str) -> usize {
    let Some(control) = manager.control(task_id) else { return 0; };
    manager.context.capacity.lock().unwrap().waiters.iter()
        .filter(|waiter| waiter.control.upgrade().is_some_and(|candidate| Arc::ptr_eq(&candidate, &control))).count()
}

#[tokio::test]
async fn prepared_batches_handoff_page_tickets_without_changing_priority_fifo_or_reader_fairness() {
    let directory = tempfile::tempdir().unwrap();
    let source = BatchFixture::new(4, 20, 0);
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    manager.set_concurrency(1).unwrap();
    let first = manager.enqueue(request("first", true)).unwrap();
    wait_until(|| source.state.pages.lock().unwrap().len() == 1 && waiting_for(&manager, &first.id) == 3).await;
    let fifo = manager.enqueue(request("fifo", true)).unwrap();
    wait_until(|| waiting_for(&manager, &fifo.id) == 1).await;
    let preferred = manager.enqueue(request("preferred", true)).unwrap();
    wait_until(|| waiting_for(&manager, &preferred.id) == 1).await;
    let mut reader = request("reader", false);
    reader.reader_generation = Some(31);
    reader.expected_revision = Some("reader-revision".to_owned());
    reader.page_indices = Some(vec![0]);
    let reader = manager.enqueue(reader).unwrap();
    wait_until(|| waiting_for(&manager, &reader.id) == 1).await;
    let paused = manager.enqueue(request("paused", true)).unwrap();
    wait_until(|| waiting_for(&manager, &paused.id) == 1).await;
    manager.pause(&paused.id).unwrap();
    wait_until(|| waiting_for(&manager, &paused.id) == 0).await;
    manager.prioritize(&preferred.id).unwrap();
    assert_eq!(source.state.pages.lock().unwrap().len(), 1);
    for (count, episode) in [(2, "preferred"), (3, "reader"), (4, "preferred")] {
        source.state.page_gate.add_permits(1);
        wait_until(|| source.state.pages.lock().unwrap().len() == count).await;
        assert_eq!(source.state.pages.lock().unwrap()[count - 1].0, episode);
        assert_eq!(source.state.cancelled_pages.load(Ordering::SeqCst), 0);
    }
    source.state.page_gate.add_permits(20);
    wait_until(|| [&first.id, &fifo.id, &preferred.id].iter()
        .all(|id| record(directory.path(), id).status == DownloadStatus::Completed)).await;
    let pages = source.state.pages.lock().unwrap();
    let last_first = pages.iter().rposition(|(episode, _)| episode == "first").unwrap();
    let first_fifo = pages.iter().position(|(episode, _)| episode == "fifo").unwrap();
    assert!(last_first < first_fifo);
    assert!(!pages.iter().any(|(episode, _)| episode == "paused"));
    assert!(!source.state.preparations.lock().unwrap().iter().any(|(episode, _)| episode == "paused"));
    assert_eq!(source.state.cancelled_pages.load(Ordering::SeqCst), 0);
    assert_eq!(record(directory.path(), &first.id).generation, first.generation);
    assert_eq!(record(directory.path(), &preferred.id).generation, preferred.generation);
    assert_eq!(record(directory.path(), &paused.id).status, DownloadStatus::Paused);
}

#[tokio::test]
async fn a_cancelled_async_worker_keeps_capacity_charged_until_its_blocking_worker_exits() {
    let directory = tempfile::tempdir().unwrap();
    let source = BatchFixture::new(1, 10, 0);
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    manager.set_concurrency(1).unwrap();
    let context = manager.context.clone();
    let task = manager.enqueue(request("blocked", true)).unwrap();
    wait_until(|| source.state.pages.lock().unwrap().len() == 1).await;
    let control = manager.control(&task.id).unwrap();
    let other = manager.enqueue(request("waiting", true)).unwrap();
    wait_until(|| waiting_for(&manager, &other.id) == 1).await;
    let publication = context.publication.lock().unwrap();
    source.state.page_gate.add_permits(1);
    wait_until(|| source.state.active.load(Ordering::SeqCst) == 0).await;
    manager.pause_handle().suspend();
    wait_until(|| control.changed.receiver_count() == 0 && context.capacity.lock().unwrap().waiters.is_empty()).await;
    assert_eq!(context.capacity.lock().unwrap().active, 1);
    assert_eq!(source.state.index_calls.load(Ordering::SeqCst), 1);
    assert_eq!(source.state.pages.lock().unwrap().len(), 1);
    drop(publication);
    wait_until(|| context.capacity.lock().unwrap().active == 0).await;
    let store = Store::open(manager.database_path()).unwrap();
    assert!(store.page_receipts(ACCOUNT, "blocked", "blocked-revision").unwrap().is_empty());
    assert!(store.pending_page_commits(ACCOUNT).unwrap().is_empty());
}

#[tokio::test]
async fn later_batches_recheck_pages_committed_by_another_owner_while_waiting() {
    let directory = tempfile::tempdir().unwrap();
    let source = BatchFixture::new(25, 10, 0);
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    let task = manager.enqueue(request("shared", true)).unwrap();
    wait_until(|| source.state.pages.lock().unwrap().len() == 2).await;
    let mut store = Store::open(manager.database_path()).unwrap();
    let other = DownloadRecord {
        id: "fixture-other-owner".to_owned(), comic_id: "comic-shared".to_owned(),
        episode_id: "shared".to_owned(), revision: "shared-revision".to_owned(),
        status: DownloadStatus::Running, total_pages: 25, generation: 1, created_at: now(), updated_at: now(),
        ..Default::default()
    };
    store.save_download(ACCOUNT, &other).unwrap();
    for page_index in 20..25 {
        let bytes = &source.state.bytes[page_index];
        let sha256 = digest_hex(bytes);
        let receipt = PageReceipt {
            episode_id: "shared".to_owned(), revision: "shared-revision".to_owned(),
            page_id: format!("shared-page-{page_index}"), page_index,
            file_name: format!("{sha256}.png"), sha256, bytes: bytes.len() as u64,
            width: 8, height: 12, verified_at: now(), ..Default::default()
        };
        store.begin_page_commit_generation(ACCOUNT, &other.id, 1, &receipt).unwrap();
        write_object(manager.pages_root(), &receipt.file_name, bytes).unwrap();
        store.commit_page_generation(ACCOUNT, &other.id, 1, &receipt).unwrap();
    }
    source.state.page_gate.add_permits(20);
    let mut events = Vec::new();
    wait_until(|| {
        events.extend(manager.drain_events(100));
        events.iter().any(|event| event.task_id == task.id && matches!(event.kind, DownloadEventKind::Completed))
    }).await;
    assert_eq!(source.state.preparations.lock().unwrap().len(), 1);
    assert_eq!(source.state.pages.lock().unwrap().len(), 20);
    let mut ready = events.into_iter().filter_map(|event| match event.kind {
        DownloadEventKind::PageReady { page_index, .. } => Some(page_index),
        _ => None,
    }).collect::<Vec<_>>();
    ready.sort_unstable();
    assert_eq!(ready, (0..25).collect::<Vec<_>>());
    assert!(store.page_receipts(ACCOUNT, "shared", "shared-revision").unwrap().iter().all(|receipt| receipt.retained));
}

#[test]
fn page_admission_respects_explicit_priority_fifo_and_the_reader_burst_bound() {
    let retained = Arc::new(TaskControl::new(1, None, true, 0, 0));
    let preferred = Arc::new(TaskControl::new(1, None, true, 0, 1));
    let reader = Arc::new(TaskControl::new(1, Some(1), false, 0, 0));
    let waiter = |ticket, control: &Arc<TaskControl>, kind| SlotWaiter {
        ticket, control: Arc::downgrade(control), epoch: 1, generation: 1, kind,
    };
    let mut capacity = Capacity {
        limit: 1, active: 0, next_ticket: 6, retained_streak: 0,
        waiters: vec![waiter(1, &retained, SlotKind::Preparation), waiter(2, &retained, SlotKind::Page),
            waiter(3, &retained, SlotKind::Page), waiter(4, &preferred, SlotKind::Preparation),
            waiter(5, &reader, SlotKind::Preparation), waiter(6, &reader, SlotKind::Page)],
    };
    assert_eq!(capacity.next_waiter(), Some((4, true)));
    capacity.waiters.retain(|waiter| waiter.ticket != 4);
    assert_eq!(capacity.next_waiter(), Some((2, true)));
    capacity.waiters.retain(|waiter| waiter.ticket != 2);
    assert_eq!(capacity.next_waiter(), Some((3, true)));
    capacity.retained_streak = MAX_RETAINED_BURST;
    assert_eq!(capacity.next_waiter(), Some((6, false)));
    capacity.waiters.retain(|waiter| waiter.ticket != 6);
    assert_eq!(capacity.next_waiter(), Some((5, false)));
    capacity.retained_streak = 0;
    capacity.waiters.retain(|waiter| waiter.ticket != 3);
    assert_eq!(capacity.next_waiter(), Some((1, true)));
}

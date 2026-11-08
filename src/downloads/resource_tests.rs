use super::*;
use crate::domain::PageDescriptor;
use std::{sync::atomic::AtomicUsize, time::Duration};
use tokio::sync::Semaphore;

const ACCOUNT: &str = "resource-fixture-account";

struct ResourceSource {
    state: Arc<ResourceSourceState>,
    page_count: usize,
}

struct ResourceSourceState {
    indices: AtomicUsize,
    pages: Mutex<Vec<(String, usize)>>,
    gate: Semaphore,
    bytes: Vec<Vec<u8>>,
}

impl ResourceSource {
    fn new(page_count: usize, available: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Arc::new(ResourceSourceState {
                indices: AtomicUsize::new(0), pages: Mutex::new(Vec::new()), gate: Semaphore::new(available),
                bytes: (0..page_count).map(|position| {
                    let pixels = image::RgbaImage::from_pixel(8, 12,
                        image::Rgba([31 + position as u8, 53, 79, 255]));
                    let mut encoded = Cursor::new(Vec::new());
                    image::DynamicImage::ImageRgba8(pixels).write_to(&mut encoded, ImageFormat::Png).unwrap();
                    encoded.into_inner()
                }).collect(),
            }),
            page_count,
        })
    }

    fn requested(&self) -> Vec<(String, usize)> { self.state.pages.lock().unwrap().clone() }

    fn release(&self, count: usize) { self.state.gate.add_permits(count); }
}

struct ResourceBatch {
    state: Arc<ResourceSourceState>,
    episode_id: String,
    selected: HashSet<usize>,
}

impl PreparedPageBatch for ResourceBatch {
    fn download_page(&self, page_index: usize) -> AcquisitionFuture<'_, AcquiredPage> {
        Box::pin(async move {
            assert!(self.selected.contains(&page_index));
            self.state.pages.lock().unwrap().push((self.episode_id.clone(), page_index));
            let permit = self.state.gate.acquire().await
                .map_err(|_| AcquisitionFailure::new("resource_fixture_gate_closed", false))?;
            permit.forget();
            Ok(AcquiredPage { bytes: self.state.bytes[page_index].clone(), width: 8, height: 12, sha256: None })
        })
    }
}

impl AcquisitionSource for ResourceSource {
    fn image_index<'a>(&'a self, episode_id: &'a str) -> AcquisitionFuture<'a, ImageIndex> {
        Box::pin(async move {
            self.state.indices.fetch_add(1, Ordering::SeqCst);
            Ok(ImageIndex {
                episode_id: episode_id.to_owned(), revision: format!("{episode_id}-revision"),
                images: (0..self.page_count).map(|index| PageDescriptor {
                    id: format!("{episode_id}-page-{index}"), index, width: 8, height: 12,
                }).collect(),
                paths: (0..self.page_count).map(|index| format!("resource-memory/{episode_id}/{index}")).collect(),
                ..Default::default()
            })
        })
    }

    fn download_page<'a>(&'a self, _index: &'a ImageIndex, _page_index: usize)
        -> AcquisitionFuture<'a, AcquiredPage>
    {
        Box::pin(async { Err(AcquisitionFailure::new("resource_fixture_legacy_path_used", false)) })
    }

    fn page_batch_size(&self) -> usize { 20 }

    fn prepare_page_batch<'a>(&'a self, index: &'a ImageIndex, page_indices: &'a [usize])
        -> AcquisitionFuture<'a, Arc<dyn PreparedPageBatch>>
    {
        Box::pin(async move {
            assert!(!page_indices.is_empty() && page_indices.len() <= 20);
            Ok(Arc::new(ResourceBatch { state: self.state.clone(), episode_id: index.episode_id.clone(),
                selected: page_indices.iter().copied().collect() }) as Arc<dyn PreparedPageBatch>)
        })
    }
}

fn request(episode: &str, retain: bool) -> DownloadRequest {
    DownloadRequest {
        comic_id: format!("comic-{episode}"), episode_id: episode.to_owned(), title: "Resource fixture".to_owned(),
        comic_title: "Resource fixture comic".to_owned(), expected_revision: Some(format!("{episode}-revision")),
        retain, ..Default::default()
    }
}

fn record(manager: &DownloadManager, task_id: &str) -> DownloadRecord {
    Store::open(manager.database_path()).unwrap().load_download(ACCOUNT, task_id).unwrap().unwrap()
}

async fn wait_until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if predicate() { return; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("the resource fixture did not reach its expected state");
}

fn registry_sizes(manager: &DownloadManager) -> (usize, usize) {
    let registry = manager.context.registry.lock().unwrap();
    (registry.controls.len(), registry.partial_intents.len())
}

fn assert_no_completed(events: &[DownloadEvent]) {
    assert!(!events.iter().any(|event| matches!(event.kind, DownloadEventKind::Completed)));
}

#[tokio::test]
async fn sparse_retired_reader_ids_use_a_floor_and_keep_existing_lower_leases_active() {
    let directory = tempfile::tempdir().unwrap();
    let source = ResourceSource::new(2, 0);
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    let mut last = 0;
    for iteration in 0..10_000_u64 {
        let owner = 100 + iteration * 17;
        manager.pin_reader(owner, "lease-only", "lease-only-revision").unwrap();
        manager.cancel_reader(owner).unwrap();
        last = owner;
    }
    {
        let readers = manager.context.readers.lock().unwrap();
        assert_eq!(readers.retired_through, Some(last));
        assert!(readers.active.is_empty());
    }
    assert!(manager.context.reader_pins.lock().unwrap().is_empty());
    assert_eq!(registry_sizes(&manager), (0, 0));
    assert!(manager.reader_is_retired(100));
    assert!(manager.reader_is_retired(last));
    assert_eq!(source.state.indices.load(Ordering::SeqCst), 0);
    drop(manager);

    let directory = tempfile::tempdir().unwrap();
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 2,
        source, Handle::current()).unwrap();
    manager.pin_reader(7, "active-lower", "active-lower-revision").unwrap();
    manager.pin_reader(99, "active-upper", "active-upper-revision").unwrap();
    manager.cancel_reader(99).unwrap();
    assert!(manager.reader_is_retired(99));
    assert!(!manager.reader_is_retired(7));
    manager.pin_reader(7, "active-lower", "active-lower-revision").unwrap();
    let mut lower = request("active-lower", false);
    lower.reader_generation = Some(7);
    lower.page_indices = Some(vec![0]);
    let task = manager.enqueue(lower.clone()).unwrap();
    assert!(manager.control(&task.id).is_some());
    assert!(manager.pin_reader(88, "unknown-lower", "unknown-lower-revision").is_err());
    let mut unknown = lower.clone();
    unknown.reader_generation = Some(88);
    assert!(manager.enqueue(unknown).is_err());
    manager.cancel_reader(7).unwrap();
    assert!(manager.reader_is_retired(7));
    assert!(manager.enqueue(lower).is_err());
    assert!(manager.pin_reader(7, "active-lower", "active-lower-revision").is_err());
    manager.pin_reader(100, "new-owner", "new-owner-revision").unwrap();
    manager.cancel_reader(100).unwrap();
    assert_eq!(registry_sizes(&manager), (0, 0));
}

#[tokio::test]
async fn closing_a_reader_reaps_its_control_selection_and_row_without_deleting_shared_pages() {
    let directory = tempfile::tempdir().unwrap();
    let source = ResourceSource::new(3, 10);
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    let mut selection = request("reader-cache", false);
    selection.reader_generation = Some(41);
    selection.page_indices = Some(vec![2, 0, 2]);
    let task = manager.enqueue(selection.clone()).unwrap();
    wait_until(|| record(&manager, &task.id).status == DownloadStatus::Paused
        && manager.control(&task.id).is_some_and(|control| control.finished.load(Ordering::Acquire))).await;
    let control = manager.control(&task.id).unwrap();
    assert_eq!(control.selection.as_deref(), Some([0, 2].as_slice()));
    assert_eq!(registry_sizes(&manager), (1, 0));
    let before_retry = source.requested();
    let retry = manager.retry(&task.id).unwrap();
    assert!(retry.generation > task.generation);
    wait_until(|| record(&manager, &task.id).status == DownloadStatus::Paused
        && manager.control(&task.id).is_some_and(|control| control.finished.load(Ordering::Acquire))).await;
    assert_eq!(source.requested(), before_retry);
    let control = manager.control(&task.id).unwrap();
    let store = Store::open(manager.database_path()).unwrap();
    let index = store.load_index(ACCOUNT, "reader-cache", Some("reader-cache-revision")).unwrap().unwrap();
    let receipts = store.page_receipts(ACCOUNT, "reader-cache", "reader-cache-revision").unwrap();
    assert_eq!(receipts.iter().map(|receipt| receipt.page_index).collect::<Vec<_>>(), vec![0, 2]);
    let originals = receipts.iter().map(|receipt| fs::read(manager.pages_root().join(&receipt.file_name)).unwrap())
        .collect::<Vec<_>>();
    drop(store);
    manager.cancel_reader(41).unwrap();
    assert!(manager.control(&task.id).is_none());
    assert_eq!(registry_sizes(&manager), (0, 0));
    let store = Store::open(manager.database_path()).unwrap();
    assert!(store.load_download(ACCOUNT, &task.id).unwrap().is_none());
    assert_eq!(store.load_index(ACCOUNT, "reader-cache", Some("reader-cache-revision")).unwrap().unwrap(), index);
    assert_eq!(store.page_receipts(ACCOUNT, "reader-cache", "reader-cache-revision").unwrap(), receipts);
    for (receipt, bytes) in receipts.iter().zip(&originals) {
        let path = manager.local_page(&receipt.episode_id, &receipt.revision, &receipt.page_id).unwrap().unwrap();
        assert_eq!(&fs::read(path).unwrap(), bytes);
        assert_eq!(digest_hex(bytes), receipt.sha256);
    }
    assert!(manager.retry(&task.id).is_err());
    assert!(manager.enqueue(selection).is_err());
    manager.context.events.progress(&manager.context, &control, DownloadEvent {
        account_epoch: 1, task_id: task.id.clone(), generation: retry.generation, kind: DownloadEventKind::Started,
    });
    assert!(manager.drain_events(1_024).iter().all(|event| event.task_id != task.id));
}

#[tokio::test]
async fn retained_completion_releases_live_control_but_delivers_and_acknowledges_its_durable_event() {
    let directory = tempfile::tempdir().unwrap();
    let source = ResourceSource::new(2, 10);
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source, Handle::current()).unwrap();
    let task = manager.enqueue(request("retained-complete", true)).unwrap();
    wait_until(|| record(&manager, &task.id).status == DownloadStatus::Completed && manager.control(&task.id).is_none()).await;
    assert_eq!(registry_sizes(&manager), (0, 0));
    let store = Store::open(manager.database_path()).unwrap();
    let pending = store.pending_download_events(ACCOUNT, 10).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].task_id, task.id);
    assert_eq!(pending[0].payload, serde_json::json!({"kind": "completed"}));
    drop(store);
    let mut completed = 0;
    wait_until(|| {
        for event in manager.drain_events(2) {
            if event.task_id == task.id && matches!(event.kind, DownloadEventKind::Completed) { completed += 1; }
        }
        completed != 0
    }).await;
    assert_eq!(completed, 1);
    let store = Store::open(manager.database_path()).unwrap();
    assert!(store.pending_download_events(ACCOUNT, 10).unwrap().is_empty());
    assert!(store.load_download(ACCOUNT, &task.id).unwrap().unwrap().terminal_event.is_none());
    for _ in 0..4 { assert_no_completed(&manager.drain_events(32)); }
}

#[tokio::test]
async fn retained_partial_intents_keep_only_the_selection_and_preserve_retry_scope_after_reaping() {
    let directory = tempfile::tempdir().unwrap();
    let source = ResourceSource::new(3, 20);
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    let mut partial = request("retained-partial", true);
    partial.page_indices = Some(vec![2, 0, 2]);
    let task = manager.enqueue(partial).unwrap();
    wait_until(|| record(&manager, &task.id).status == DownloadStatus::Paused && manager.control(&task.id).is_none()).await;
    assert_eq!(registry_sizes(&manager), (0, 1));
    assert_eq!(manager.context.registry.lock().unwrap().partial_intents[&task.id].as_ref(), &[0, 2]);
    let receipt = Store::open(manager.database_path()).unwrap()
        .page_receipt(ACCOUNT, "retained-partial", "retained-partial-revision", "retained-partial-page-0")
        .unwrap().unwrap();
    fs::remove_file(manager.pages_root().join(&receipt.file_name)).unwrap();
    let requested_before = source.requested().len();
    manager.retry(&task.id).unwrap();
    wait_until(|| record(&manager, &task.id).status == DownloadStatus::Paused && manager.control(&task.id).is_none()).await;
    assert_eq!(&source.requested()[requested_before..], &[("retained-partial".to_owned(), 0)]);
    assert_eq!(registry_sizes(&manager), (0, 1));
    assert!(!source.requested().iter().any(|(_, position)| *position == 1));
    manager.enqueue(request("retained-partial", true)).unwrap();
    wait_until(|| record(&manager, &task.id).status == DownloadStatus::Completed && manager.control(&task.id).is_none()).await;
    assert_eq!(registry_sizes(&manager), (0, 0));
    assert_eq!(record(&manager, &task.id).completed_pages, 3);
}

#[tokio::test]
async fn progress_floods_stay_bounded_and_closed_contexts_reject_late_notifications() {
    let directory = tempfile::tempdir().unwrap();
    let source = ResourceSource::new(1, 0);
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    let task = manager.enqueue(request("progress-flood", true)).unwrap();
    wait_until(|| source.requested().len() == 1).await;
    let control = manager.control(&task.id).unwrap();
    // Exercise notification pressure independently of file throughput; no objects are published here.
    for position in 0..MAX_PROGRESS_EVENTS * 3 {
        manager.context.events.progress(&manager.context, &control, DownloadEvent {
            account_epoch: 1, task_id: task.id.clone(), generation: task.generation,
            kind: DownloadEventKind::PageReady {
                episode_id: task.episode_id.clone(), revision: "progress-flood-revision".to_owned(),
                page_id: format!("notification-{position}"), page_index: position,
                path: manager.pages_root().join(format!("not-published-{position}")), retained: true,
                completed_pages: 0, total_pages: 1,
            },
        });
    }
    {
        let mailbox = manager.context.events.state.lock().unwrap();
        assert!(mailbox.progress.len() <= 256);
        assert!(mailbox.progress_dirty);
    }
    source.release(1);
    wait_until(|| record(&manager, &task.id).status == DownloadStatus::Completed && manager.control(&task.id).is_none()).await;
    let mut delivered = false;
    wait_until(|| {
        delivered |= manager.drain_events(17).iter().any(|event|
            event.task_id == task.id && matches!(event.kind, DownloadEventKind::Completed));
        delivered
    }).await;
    let context = manager.context.clone();
    drop(manager);
    context.events.progress(&context, &control, DownloadEvent {
        account_epoch: 1, task_id: task.id.clone(), generation: task.generation, kind: DownloadEventKind::Started,
    });
    context.events.retry_terminal(TerminalRetry { task_id: task.id, generation: task.generation,
        payload: serde_json::json!({"kind": "completed"}) });
    let mailbox = context.events.state.lock().unwrap();
    assert!(mailbox.closed);
    assert!(mailbox.progress.is_empty());
    assert!(mailbox.terminal_retries.is_empty());
}

#[tokio::test]
async fn terminal_outbox_failures_use_bounded_retries_and_reconcile_every_durable_marker_once() {
    let directory = tempfile::tempdir().unwrap();
    let source = ResourceSource::new(1, 100);
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source, Handle::current()).unwrap();
    let connection = rusqlite::Connection::open(manager.database_path()).unwrap();
    // A persistent trigger covers worker connections while leaving download markers durable.
    connection.execute_batch(
        "CREATE TRIGGER resource_block_terminal_outbox
         BEFORE INSERT ON download_event_outbox
         BEGIN SELECT RAISE(ABORT, 'resource_fixture_outbox_blocked'); END;"
    ).unwrap();
    let mut expected = HashSet::new();
    for position in 0..81 {
        let task = manager.enqueue(request(&format!("terminal-{position:03}"), true)).unwrap();
        assert!(expected.insert(task.id));
    }
    assert!(expected.len() > MAX_TERMINAL_RETRIES && expected.len() > RECONCILE_PAGE_SIZE);
    wait_until(|| {
        let tasks = manager.records().unwrap();
        tasks.len() == expected.len() && tasks.iter().all(|task|
            task.status == DownloadStatus::Completed && manager.control(&task.id).is_none())
    }).await;
    {
        let mailbox = manager.context.events.state.lock().unwrap();
        assert!(mailbox.terminal_retries.len() <= 64);
        assert!(mailbox.reconcile_terminal);
        assert!(mailbox.progress.len() <= 256);
    }
    let store = Store::open(manager.database_path()).unwrap();
    assert!(store.pending_download_events(ACCOUNT, 1_024).unwrap().is_empty());
    let markers = store.terminal_downloads_page(ACCOUNT, None, 1_024).unwrap();
    assert_eq!(markers.len(), expected.len());
    assert!(markers.iter().all(|task| task.terminal_event == Some(serde_json::json!({"kind": "completed"}))));
    connection.execute_batch("DROP TRIGGER resource_block_terminal_outbox;").unwrap();
    let mut delivered = HashSet::new();
    wait_until(|| {
        for event in manager.drain_events(7) {
            if matches!(event.kind, DownloadEventKind::Completed) {
                assert!(expected.contains(&event.task_id));
                assert!(delivered.insert(event.task_id), "a terminal marker was delivered twice");
            }
        }
        delivered.len() == expected.len()
    }).await;
    assert_eq!(delivered, expected);
    assert!(store.pending_download_events(ACCOUNT, 1_024).unwrap().is_empty());
    assert!(store.terminal_downloads_page(ACCOUNT, None, 1_024).unwrap().is_empty());
    assert!(store.downloads(ACCOUNT).unwrap().iter().all(|task| task.terminal_event.is_none()));
    {
        let mut mailbox = manager.context.events.state.lock().unwrap();
        mailbox.reconcile_terminal = true;
        mailbox.terminal_cursor = None;
    }
    for _ in 0..20 { assert_no_completed(&manager.drain_events(7)); }
    assert!(store.pending_download_events(ACCOUNT, 1_024).unwrap().is_empty());
}

#[tokio::test]
async fn acknowledged_terminal_delivery_does_not_poll_sqlite_during_idle_ticks() {
    let directory = tempfile::tempdir().unwrap();
    let source = ResourceSource::new(1, 10);
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source, Handle::current()).unwrap();
    assert!(manager.drain_events(32).is_empty());
    let initial_polls = manager.context.events.terminal_polls.load(Ordering::SeqCst);
    assert_eq!(initial_polls, 1);
    for _ in 0..1_000 { assert!(manager.drain_events(32).is_empty()); }
    assert_eq!(manager.context.events.terminal_polls.load(Ordering::SeqCst), initial_polls);

    let task = manager.enqueue(request("idle-after-completion", true)).unwrap();
    wait_until(|| record(&manager, &task.id).status == DownloadStatus::Completed
        && manager.control(&task.id).is_none()).await;
    assert!(manager.drain_events(32).iter().any(|event|
        event.task_id == task.id && matches!(event.kind, DownloadEventKind::Completed)));
    let store = Store::open(manager.database_path()).unwrap();
    assert!(store.load_download(ACCOUNT, &task.id).unwrap().unwrap().terminal_event.is_none());
    assert!(store.pending_download_events(ACCOUNT, 32).unwrap().is_empty());
    let settled_polls = manager.context.events.terminal_polls.load(Ordering::SeqCst);
    for _ in 0..1_000 { assert!(manager.drain_events(32).is_empty()); }
    assert_eq!(manager.context.events.terminal_polls.load(Ordering::SeqCst), settled_polls);
}

#[test]
fn terminal_storage_failures_back_off_and_new_durable_events_wake_delivery() {
    let mailbox = EventMailbox::default();
    let mut now = Instant::now();
    for seconds in [1, 2, 4, 8, 16, 30, 30] {
        assert!(mailbox.terminal_poll_due(now));
        mailbox.terminal_poll_finished(false, now, mailbox.terminal_notice());
        assert!(!mailbox.terminal_poll_due(now + Duration::from_secs(seconds) - Duration::from_nanos(1)));
        now += Duration::from_secs(seconds);
        assert!(mailbox.terminal_poll_due(now));
    }
    mailbox.terminal_poll_finished(false, now, mailbox.terminal_notice());
    assert!(!mailbox.terminal_poll_due(now));
    mailbox.terminal_available();
    assert!(mailbox.terminal_poll_due(now));
    assert_eq!(mailbox.state.lock().unwrap().terminal_failures, 0);
}

#[test]
fn packet_cancellation_is_checked_before_decode_and_is_never_corrupt_evidence() {
    let checks = AtomicUsize::new(0);
    let check = || -> Result<()> {
        if checks.fetch_add(1, Ordering::SeqCst) == 0 { Ok(()) } else { Err(cancelled().into()) }
    };
    let failure = verify_packet(AcquiredPage { bytes: b"not an image".to_vec(),
        width: 0, height: 0, sha256: None }, Some(&check)).err().expect("The packet must be cancelled");
    assert_eq!(failure.code, "download_cancelled");
    assert!(!failure.retryable);
}

#[tokio::test]
async fn a_worker_finishing_during_reader_collection_keeps_the_next_collection_requested() {
    let directory = tempfile::tempdir().unwrap();
    let source = ResourceSource::new(1, 0);
    let manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source, Handle::current()).unwrap();
    let context = manager.context.clone();
    let publication = context.publication.clone();
    let held = publication.lock().unwrap();
    let collection = std::thread::spawn(move || {
        let result = manager.collect_reader_metadata();
        (manager, result)
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while context.reader_gc_needed.load(Ordering::Acquire) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!context.reader_gc_needed.load(Ordering::Acquire), "The collector must consume its own request before waiting");
    context.reader_gc_needed.store(true, Ordering::Release);
    drop(held);
    let (manager, result) = collection.join().unwrap();
    result.unwrap();
    assert!(context.reader_gc_needed.load(Ordering::Acquire), "A concurrent completion must not be overwritten");
    manager.collect_reader_metadata().unwrap();
    assert!(!context.reader_gc_needed.load(Ordering::Acquire));
}

#[tokio::test]
async fn periodic_unconfirmed_terminal_checks_do_not_starve_reader_index_replay() {
    let directory = tempfile::tempdir().unwrap();
    let source = ResourceSource::new(1, 0);
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    let mut selected = request("unconfirmed-replay", false);
    selected.reader_generation = Some(7);
    let task = manager.enqueue(selected).unwrap();
    wait_until(|| source.requested().len() == 1).await;
    {
        let mut mailbox = manager.context.events.state.lock().unwrap();
        mailbox.progress.clear();
        mailbox.progress_dirty = true;
        mailbox.unconfirmed_state = true;
        mailbox.reconcile_terminal = true;
    }
    let events = manager.drain_events(32);
    assert!(events.iter().any(|event| event.task_id == task.id
        && matches!(event.kind, DownloadEventKind::IndexReady { .. })));
    assert!(!manager.context.events.terminal_poll_due(Instant::now()));
    assert!(manager.context.events.storage_retry_due(Instant::now()));
    assert!(!manager.context.events.state.lock().unwrap().progress_dirty);
    manager.cancel_reader(7).unwrap();
}

#[test]
fn a_new_committed_notice_cannot_be_delayed_by_an_older_poll_completion() {
    let mailbox = EventMailbox::default();
    mailbox.state.lock().unwrap().unconfirmed_state = true;
    let notice = mailbox.terminal_notice();
    let now = Instant::now();
    mailbox.terminal_available();
    // Model an older seek clearing its dirty bit before a later database error.
    mailbox.state.lock().unwrap().reconcile_terminal = false;
    mailbox.terminal_poll_finished(true, now, notice);
    assert!(mailbox.terminal_poll_due(now));
    mailbox.state.lock().unwrap().reconcile_terminal = false;
    mailbox.terminal_poll_finished(false, now, notice);
    assert!(mailbox.terminal_poll_due(now));
    assert_eq!(mailbox.state.lock().unwrap().terminal_failures, 0);
}

#[tokio::test]
async fn healthy_terminal_reads_do_not_reset_an_unrecovered_index_replay_failure() {
    let directory = tempfile::tempdir().unwrap();
    let source = ResourceSource::new(1, 0);
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    let mut selected = request("broken-replay", false);
    selected.reader_generation = Some(11);
    let task = manager.enqueue(selected).unwrap();
    wait_until(|| source.requested().len() == 1).await;
    let connection = rusqlite::Connection::open(manager.database_path()).unwrap();
    let original: String = connection.query_row(
        "SELECT payload FROM image_indices WHERE account_key=?1 AND episode_id=?2",
        [ACCOUNT, "broken-replay"], |row| row.get(0)).unwrap();
    connection.execute(
        "UPDATE image_indices SET payload='invalid-json' WHERE account_key=?1 AND episode_id=?2",
        [ACCOUNT, "broken-replay"]).unwrap();
    {
        let mut mailbox = manager.context.events.state.lock().unwrap();
        mailbox.progress.clear();
        mailbox.progress_dirty = true;
        mailbox.reconcile_terminal = true;
    }
    for attempt in 1..=3 {
        // Advance the controlled retry boundary without waiting on wall time.
        manager.context.events.state.lock().unwrap().terminal_retry_at = None;
        assert!(manager.drain_events(32).is_empty());
        let mailbox = manager.context.events.state.lock().unwrap();
        assert_eq!(mailbox.terminal_failures, attempt);
        assert!(mailbox.terminal_retry_at.is_some_and(|deadline| deadline > Instant::now()));
        assert!(mailbox.progress_dirty);
    }
    connection.execute(
        "UPDATE image_indices SET payload=?1 WHERE account_key=?2 AND episode_id=?3",
        rusqlite::params![original, ACCOUNT, "broken-replay"]).unwrap();
    manager.context.events.state.lock().unwrap().terminal_retry_at = None;
    assert!(manager.drain_events(32).iter().any(|event| event.task_id == task.id
        && matches!(event.kind, DownloadEventKind::IndexReady { .. })));
    let mailbox = manager.context.events.state.lock().unwrap();
    assert_eq!(mailbox.terminal_failures, 0);
    assert!(!mailbox.progress_dirty);
    drop(mailbox);
    manager.cancel_reader(11).unwrap();
}

#[tokio::test]
async fn overflow_during_index_replay_cannot_be_cleared_or_skip_to_an_old_cursor() {
    let directory = tempfile::tempdir().unwrap();
    let source = ResourceSource::new(1, 0);
    let mut manager = DownloadManager::new(directory.path(), ACCOUNT.to_owned(), 1,
        source.clone(), Handle::current()).unwrap();
    let task = manager.enqueue(request("replay-overflow-race", true)).unwrap();
    wait_until(|| source.requested().len() == 1).await;
    let control = manager.control(&task.id).unwrap();
    let notice = {
        let mut mailbox = manager.context.events.state.lock().unwrap();
        mailbox.progress.clear();
        mailbox.progress_dirty = true;
        mailbox.replay = None;
        mailbox.progress_notice
    };
    for position in 0..=MAX_PROGRESS_EVENTS {
        manager.context.events.progress(&manager.context, &control, DownloadEvent {
            account_epoch: 1, task_id: task.id.clone(), generation: task.generation,
            kind: DownloadEventKind::PageReady {
                episode_id: task.episode_id.clone(), revision: "replay-overflow-race-revision".to_owned(),
                page_id: format!("not-published-{position}"), page_index: position,
                path: manager.pages_root().join(format!("not-published-{position}")), retained: true,
                completed_pages: 0, total_pages: 1,
            },
        });
    }
    assert_ne!(manager.context.events.state.lock().unwrap().progress_notice, notice);
    manager.context.events.finish_progress_replay(notice, None);
    {
        let mailbox = manager.context.events.state.lock().unwrap();
        assert!(mailbox.progress_dirty);
        assert!(mailbox.replay.is_none());
    }
    manager.context.events.finish_progress_replay(notice, Some(ProgressReplay {
        after_task: Some("old-cursor".to_owned()),
    }));
    let mailbox = manager.context.events.state.lock().unwrap();
    assert!(mailbox.progress_dirty);
    assert!(mailbox.replay.is_none());
}

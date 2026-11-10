//! Durable page acquisition with account, task-generation, and file-commit boundaries.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, File, OpenOptions},
    future::Future,
    io::{Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, ensure};
#[cfg(test)]
use image::ImageFormat;
#[cfg(test)]
use std::io::Cursor;
#[cfg(test)]
use std::sync::mpsc;
use sha2::{Digest, Sha256};
use tokio::{runtime::Handle, sync::{Notify, watch}, task::JoinSet};

use crate::{
    domain::ImageIndex,
    image_integrity::{DecodedImageProof, IntegrityFailureKind, InvalidEncodingEvidence, MAX_ENCODED_BYTES,
        MAX_SOURCE_PIXELS, VALIDATION_VERSION, validate_image_cancellable},
    source::work::{self, HEAVY_WORK_BYTES, WorkCancellation, WorkError},
    store::{DownloadRecord, DownloadStatus, PageReceipt, Store, PENDING_REVISION},
};

mod bili_source;
mod local_source;

const MAX_IMAGE_BYTES: u64 = MAX_ENCODED_BYTES;
const MAX_PAGES: usize = 10_000;
const MAX_PAGE_BATCH: usize = 20;
const MAX_PAGE_JOBS: usize = 4;
const MAX_RETAINED_BURST: u8 = 2;
const MAX_PROGRESS_EVENTS: usize = 256;
const MAX_TERMINAL_RETRIES: usize = 64;
const RECONCILE_PAGE_SIZE: usize = 64;
const MAX_REPLAY_SCAN: usize = 128;
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(1);

pub type AcquisitionFuture<'a, T> =
    Pin<Box<dyn Future<Output = std::result::Result<T, AcquisitionFailure>> + Send + 'a>>;

/// A bounded page packet. Its bytes are independently checked before persistence.
pub struct AcquiredPage {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("{code}")]
pub struct AcquisitionFailure {
    pub code: String,
    pub retryable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DownloadControlError {
    #[error("episode is in use by a reader")]
    ReaderInUse,
}

impl AcquisitionFailure {
    fn new(code: &str, retryable: bool) -> Self {
        Self { code: code.to_owned(), retryable }
    }
}

/// The real adapter and controlled fixtures share this acquisition boundary.
pub trait AcquisitionSource: Send + Sync + 'static {
    fn image_index<'a>(&'a self, episode_id: &'a str) -> AcquisitionFuture<'a, ImageIndex>;
    /// Sources may recheck episode access in its owning comic before exposing an index.
    fn image_index_for<'a>(&'a self, _comic_id: &'a str, episode_id: &'a str)
        -> AcquisitionFuture<'a, ImageIndex>
    {
        self.image_index(episode_id)
    }
    fn download_page<'a>(&'a self, index: &'a ImageIndex, page_index: usize)
        -> AcquisitionFuture<'a, AcquiredPage>;

    /// Sources that prepare several missing pages opt in with a size greater than one.
    fn page_batch_size(&self) -> usize { 1 }

    fn prepare_page_batch<'a>(&'a self, _index: &'a ImageIndex, _page_indices: &'a [usize])
        -> AcquisitionFuture<'a, Arc<dyn PreparedPageBatch>>
    {
        Box::pin(async { Err(AcquisitionFailure::new("download_batch_unsupported", false)) })
    }
}

/// A source-owned, short-lived acquisition plan that never enters manager or store state.
pub trait PreparedPageBatch: Send + Sync {
    fn download_page(&self, page_index: usize) -> AcquisitionFuture<'_, AcquiredPage>;
}

#[derive(Clone, Debug, Default)]
pub struct DownloadRequest {
    pub comic_id: String,
    pub episode_id: String,
    pub title: String,
    pub comic_title: String,
    pub expected_revision: Option<String>,
    pub retain: bool,
    pub reader_generation: Option<u64>,
    /// A reader may acquire just its visible and neighboring source pages.
    pub page_indices: Option<Vec<usize>>,
}

#[derive(Clone, Debug)]
pub struct DownloadEvent {
    pub account_epoch: u64,
    pub task_id: String,
    pub generation: u64,
    pub kind: DownloadEventKind,
}

#[derive(Clone, Debug)]
pub enum DownloadEventKind {
    Started,
    IndexReady { index: ImageIndex },
    PageReady {
        episode_id: String,
        revision: String,
        page_id: String,
        page_index: usize,
        path: PathBuf,
        retained: bool,
        completed_pages: u32,
        total_pages: u32,
    },
    Completed,
    PrefetchReady,
    Paused,
    Cancelled,
    Failed { code: String, retryable: bool },
}

struct Capacity {
    limit: usize,
    active: usize,
    next_ticket: u64,
    retained_streak: u8,
    waiters: Vec<SlotWaiter>,
}

struct SlotWaiter {
    ticket: u64,
    control: Weak<TaskControl>,
    epoch: u64,
    generation: u64,
    kind: SlotKind,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SlotKind { Preparation, Page }

impl Capacity {
    fn next_waiter(&self) -> Option<(u64, bool)> {
        let mut reader = None::<(u64, SlotKind)>;
        let mut retained = None::<(u64, u64, SlotKind)>;
        for waiter in &self.waiters {
            let Some(control) = waiter.control.upgrade() else { continue; };
            if control.retained {
                let priority = control.priority.load(Ordering::Acquire);
                if retained.is_none_or(|(ticket, value, kind)| priority > value || priority == value
                    && (waiter.kind > kind || waiter.kind == kind && waiter.ticket < ticket)) {
                    retained = Some((waiter.ticket, priority, waiter.kind));
                }
            } else if reader.is_none_or(|(ticket, kind)| waiter.kind > kind
                || waiter.kind == kind && waiter.ticket < ticket) {
                reader = Some((waiter.ticket, waiter.kind));
            }
        }
        match (reader, retained) {
            (Some((ticket, _)), Some(_)) if self.retained_streak >= MAX_RETAINED_BURST => Some((ticket, false)),
            (_, Some((ticket, _, _))) => Some((ticket, true)),
            (Some((ticket, _)), None) => Some((ticket, false)),
            (None, None) => None,
        }
    }
}

struct PauseState {
    suspended: AtomicBool,
    revision: AtomicU64,
    changed: watch::Sender<u64>,
    transition: Mutex<()>,
}

impl PauseState {
    fn new() -> Self {
        let (changed, _) = watch::channel(0);
        Self { suspended: AtomicBool::new(false), revision: AtomicU64::new(0), changed,
            transition: Mutex::new(()) }
    }
}

/// A transport latch remains usable while the manager is owned by a background job.
#[derive(Clone)]
pub struct DownloadPauseHandle {
    state: Arc<PauseState>,
}

impl DownloadPauseHandle {
    pub fn suspend(&self) { self.set_suspended(true); }
    pub fn resume(&self) { self.set_suspended(false); }
    pub fn is_suspended(&self) -> bool { self.state.suspended.load(Ordering::Acquire) }

    fn set_suspended(&self, suspended: bool) {
        let _transition = self.state.transition.lock().unwrap_or_else(|error| error.into_inner());
        if self.is_suspended() == suspended { return; }
        if suspended { self.state.suspended.store(true, Ordering::Release); }
        let revision = self.state.revision.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        if !suspended { self.state.suspended.store(false, Ordering::Release); }
        self.state.changed.send_replace(revision);
    }
}

struct ContextState {
    database: PathBuf,
    pages: PathBuf,
    account: String,
    epoch: AtomicU64,
    pause: Arc<PauseState>,
    source: Arc<dyn AcquisitionSource>,
    events: EventMailbox,
    registry: Mutex<TaskRegistry>,
    readers: Mutex<ReaderOwners>,
    active: AtomicBool,
    reader_gc_needed: AtomicBool,
    capacity: Mutex<Capacity>,
    capacity_changed: Notify,
    publication: Arc<Mutex<()>>,
    reader_pins: Mutex<HashMap<u64, HashSet<(String, String)>>>,
}

struct TaskControl {
    generation: AtomicU64,
    changed: watch::Sender<u64>,
    reader_generation: Option<u64>,
    retained: bool,
    pause_revision: u64,
    priority: AtomicU64,
    selection: Option<Arc<[usize]>>,
    lease: Option<Arc<ReaderLease>>,
    finished: AtomicBool,
}

impl TaskControl {
    #[cfg(test)]
    fn new(generation: u64, reader_generation: Option<u64>, retained: bool, pause_revision: u64, priority: u64) -> Self {
        let lease = (!retained).then(|| Arc::new(ReaderLease { active: AtomicBool::new(true) }));
        Self::scoped(generation, reader_generation, retained, pause_revision, priority, None, lease)
    }

    fn scoped(
        generation: u64, reader_generation: Option<u64>, retained: bool, pause_revision: u64, priority: u64,
        selection: Option<Arc<[usize]>>, lease: Option<Arc<ReaderLease>>,
    ) -> Self {
        let (changed, _) = watch::channel(generation);
        Self { generation: AtomicU64::new(generation), changed, reader_generation, retained, pause_revision,
            priority: AtomicU64::new(priority), selection, lease, finished: AtomicBool::new(false) }
    }

    fn change(&self, generation: u64) {
        self.generation.store(generation, Ordering::Release);
        self.changed.send_replace(generation);
    }

    fn reader_is_active(&self) -> bool {
        self.retained || self.lease.as_ref().is_some_and(|lease| lease.active.load(Ordering::Acquire))
    }
}

struct ReaderLease { active: AtomicBool }

#[derive(Default)]
struct ReaderOwners {
    retired_through: Option<u64>,
    active: HashMap<u64, Arc<ReaderLease>>,
}

impl ReaderOwners {
    fn register(&mut self, owner: u64) -> Result<Arc<ReaderLease>> {
        if let Some(lease) = self.active.get(&owner) { return Ok(lease.clone()); }
        ensure!(self.retired_through.is_none_or(|floor| owner > floor), "reader prefetch owner is closed");
        let lease = Arc::new(ReaderLease { active: AtomicBool::new(true) });
        self.active.insert(owner, lease.clone());
        Ok(lease)
    }

    fn retire(&mut self, owner: u64) {
        if let Some(lease) = self.active.remove(&owner) { lease.active.store(false, Ordering::Release); }
        self.retired_through = Some(self.retired_through.map_or(owner, |floor| floor.max(owner)));
    }

    fn is_retired(&self, owner: u64) -> bool {
        !self.active.contains_key(&owner) && self.retired_through.is_some_and(|floor| owner <= floor)
    }
}

#[derive(Default)]
struct TaskRegistry {
    controls: HashMap<String, Arc<TaskControl>>,
    // Keep only the selection that cannot be reconstructed from a retained checkpoint.
    // Reopening a manager retains the existing whole-chapter resume behavior.
    partial_intents: HashMap<String, Arc<[usize]>>,
}

struct WorkerCompletion {
    context: Arc<ContextState>,
    control: Arc<TaskControl>,
    task_id: String,
}

impl Drop for WorkerCompletion {
    fn drop(&mut self) {
        self.control.finished.store(true, Ordering::Release);
        if self.control.retained || !self.control.reader_is_active() {
            if let Ok(mut registry) = self.context.registry.lock() {
                if registry.controls.get(&self.task_id).is_some_and(|current| Arc::ptr_eq(current, &self.control)) {
                    registry.controls.remove(&self.task_id);
                }
            }
        }
        if !self.control.retained { self.context.reader_gc_needed.store(true, Ordering::Release); }
    }
}

struct BufferedProgress {
    event: DownloadEvent,
    control: Arc<TaskControl>,
}

#[derive(Clone)]
struct TerminalRetry {
    task_id: String,
    generation: u64,
    payload: serde_json::Value,
}

#[derive(Default)]
struct ProgressReplay {
    after_task: Option<String>,
}

#[derive(Default)]
struct EventMailboxState {
    progress: VecDeque<BufferedProgress>,
    terminal_retries: VecDeque<TerminalRetry>,
    reconcile_terminal: bool,
    terminal_retry_at: Option<Instant>,
    terminal_failures: u8,
    terminal_notice: u64,
    unconfirmed_state: bool,
    terminal_cursor: Option<String>,
    progress_dirty: bool,
    progress_notice: u64,
    replay: Option<ProgressReplay>,
    closed: bool,
}

struct EventMailbox {
    state: Mutex<EventMailboxState>,
    #[cfg(test)]
    terminal_polls: std::sync::atomic::AtomicUsize,
}

impl Default for EventMailbox {
    fn default() -> Self {
        Self {
            state: Mutex::new(EventMailboxState { reconcile_terminal: true, ..Default::default() }),
            #[cfg(test)]
            terminal_polls: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl EventMailbox {
    fn finish_progress_replay(&self, notice: u64, replay: Option<ProgressReplay>) {
        if let Ok(mut state) = self.state.lock() {
            if state.closed || state.progress_notice != notice { return; }
            state.progress_dirty = replay.is_some();
            state.replay = replay;
        }
    }

    fn storage_retry_due(&self, now: Instant) -> bool {
        self.state.lock().is_ok_and(|state| !state.closed
            && (state.terminal_failures == 0 || state.terminal_retry_at.is_none_or(|deadline| now >= deadline)))
    }

    fn terminal_notice(&self) -> u64 {
        self.state.lock().map_or(0, |state| state.terminal_notice)
    }

    fn terminal_poll_due(&self, now: Instant) -> bool {
        self.state.lock().is_ok_and(|state| !state.closed
            && (state.reconcile_terminal || !state.terminal_retries.is_empty())
            && state.terminal_retry_at.is_none_or(|deadline| now >= deadline))
    }

    fn terminal_available(&self) {
        if let Ok(mut state) = self.state.lock() {
            if state.closed { return; }
            state.reconcile_terminal = true;
            state.terminal_notice = state.terminal_notice.wrapping_add(1);
            state.terminal_retry_at = None;
            state.terminal_failures = 0;
        }
    }

    fn terminal_poll_finished(&self, succeeded: bool, now: Instant, notice: u64) {
        if let Ok(mut state) = self.state.lock() {
            if state.closed { return; }
            // A newer committed event owns its immediate wakeup. An older
            // in-flight poll must not put that event back behind a retry delay.
            if state.terminal_notice != notice {
                state.reconcile_terminal = true;
                return;
            }
            if succeeded {
                state.terminal_failures = 0;
                state.terminal_retry_at = state.unconfirmed_state.then_some(now + Duration::from_secs(30));
            } else {
                state.reconcile_terminal = true;
                state.terminal_failures = state.terminal_failures.saturating_add(1);
                let seconds = (1_u64 << state.terminal_failures.saturating_sub(1).min(5)).min(30);
                state.terminal_retry_at = Some(now + Duration::from_secs(seconds));
            }
        }
    }

    fn invalidate_task(&self, task_id: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.progress.retain(|pending| pending.event.task_id != task_id);
            state.terminal_retries.retain(|pending| pending.task_id != task_id);
            state.replay = None;
        }
    }

    fn progress(&self, context: &ContextState, control: &Arc<TaskControl>, event: DownloadEvent) {
        let Ok(mut state) = self.state.lock() else { return; };
        if state.closed || !is_current(context, control, event.account_epoch, event.generation) { return; }
        state.progress.retain(|pending| !(pending.event.task_id == event.task_id
            && (pending.event.generation != event.generation || same_progress(&pending.event.kind, &event.kind))));
        if state.progress.len() == MAX_PROGRESS_EVENTS {
            state.progress.pop_front();
            state.progress_dirty = true;
            state.progress_notice = state.progress_notice.wrapping_add(1);
            state.replay = None;
        }
        state.progress.push_back(BufferedProgress { event, control: control.clone() });
    }

    fn retry_terminal(&self, event: TerminalRetry) {
        if let Ok(mut state) = self.state.lock() {
            if state.closed { return; }
            state.terminal_retries.retain(|pending| pending.task_id != event.task_id);
            if state.terminal_retries.len() == MAX_TERMINAL_RETRIES { state.terminal_retries.pop_front(); }
            state.terminal_retries.push_back(event);
            state.reconcile_terminal = true;
        }
    }

    fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            state.progress.clear();
            state.terminal_retries.clear();
            state.replay = None;
        }
    }
}

fn same_progress(left: &DownloadEventKind, right: &DownloadEventKind) -> bool {
    match (left, right) {
        (DownloadEventKind::Started, DownloadEventKind::Started)
        | (DownloadEventKind::IndexReady { .. }, DownloadEventKind::IndexReady { .. }) => true,
        (DownloadEventKind::PageReady { page_index: left, .. }, DownloadEventKind::PageReady { page_index: right, .. }) => left == right,
        _ => false,
    }
}

/// A local-only account snapshot that can be cloned into a blocking render worker.
#[derive(Clone)]
pub struct LocalPageResolver {
    database: PathBuf,
    pages: PathBuf,
    account: String,
    connection: Arc<Mutex<Option<Store>>>,
}

impl LocalPageResolver {
    /// Resolve only receipt-backed bytes whose digest and container geometry still match.
    pub fn local_page(&self, episode_id: &str, revision: &str, page_id: &str) -> Result<Option<PathBuf>> {
        let publication = publication_lock();
        let _publication = publication.lock().map_err(|_| anyhow!("page publication lock failed"))?;
        let store = Store::open(&self.database)?;
        let Some(receipt) = store.page_receipt(&self.account, episode_id, revision, page_id)? else {
            return Ok(None);
        };
        let Some(receipt) = revalidate_receipt_locked(&store, &self.account, &self.pages, &receipt)? else {
            store.invalidate_page(&self.account, episode_id, revision, page_id)?;
            return Ok(None);
        };
        Ok(Some(self.pages.join(&receipt.file_name)))
    }
}

/// Application-owned downloads outlive reader views; reader prefetch has its own owner.
pub struct DownloadManager {
    context: Arc<ContextState>,
    handle: Handle,
}

impl DownloadManager {
    pub fn new(
        data_dir: &Path,
        account_key: String,
        account_epoch: u64,
        source: Arc<dyn AcquisitionSource>,
        handle: Handle,
    ) -> Result<Self> {
        ensure!(!account_key.trim().is_empty(), "download account namespace is empty");
        fs::create_dir_all(data_dir).context("create application data directory")?;
        let root = data_dir.canonicalize().context("resolve application data directory")?;
        let pages = root.join("accounts").join(digest_hex(account_key.as_bytes())).join("pages");
        fs::create_dir_all(&pages).context("create account page directory")?;
        let pages = pages.canonicalize().context("resolve account page directory")?;
        ensure!(pages.starts_with(&root), "account page directory escapes application data");
        let database = root.join("state.sqlite3");
        Store::open(&database)?;
        Ok(Self {
            context: Arc::new(ContextState {
                database, pages, account: account_key, epoch: AtomicU64::new(account_epoch),
                pause: Arc::new(PauseState::new()),
                source, events: EventMailbox::default(), registry: Mutex::new(TaskRegistry::default()),
                readers: Mutex::new(ReaderOwners::default()), active: AtomicBool::new(true),
                reader_gc_needed: AtomicBool::new(true), capacity: Mutex::new(Capacity { limit: 2, active: 0,
                    next_ticket: 0, retained_streak: 0, waiters: Vec::new() }),
                capacity_changed: Notify::new(), publication: publication_lock(),
                reader_pins: Mutex::new(HashMap::new()),
            }),
            handle,
        })
    }

    pub fn database_path(&self) -> &Path { &self.context.database }
    pub fn pages_root(&self) -> &Path { &self.context.pages }
    pub fn account_epoch(&self) -> u64 { self.context.epoch.load(Ordering::Acquire) }
    pub fn pause_handle(&self) -> DownloadPauseHandle { DownloadPauseHandle { state: self.context.pause.clone() } }
    pub fn page_resolver(&self) -> LocalPageResolver {
        LocalPageResolver {
            database: self.context.database.clone(), pages: self.context.pages.clone(),
            account: self.context.account.clone(), connection: Arc::new(Mutex::new(None)),
        }
    }

    fn control(&self, task_id: &str) -> Option<Arc<TaskControl>> {
        self.context.registry.lock().ok()?.controls.get(task_id).cloned()
    }

    fn reader_lease(&self, owner: u64) -> Result<Arc<ReaderLease>> {
        ensure!(self.context.active.load(Ordering::Acquire), "download context is retired");
        self.context.readers.lock().map_err(|_| anyhow!("reader owner lock failed"))?.register(owner)
    }

    fn reader_is_retired(&self, owner: u64) -> bool {
        self.context.readers.lock().map_or(true, |readers| readers.is_retired(owner))
    }

    fn collect_reader_metadata(&self) -> Result<()> {
        if !self.context.reader_gc_needed.swap(false, Ordering::AcqRel) { return Ok(()); }
        let collected = (|| -> Result<()> {
            let protected = self.context.registry.lock().map_err(|_| anyhow!("download registry lock failed"))?
                .controls.iter().filter(|(_, control)| !control.retained && control.reader_is_active())
                .map(|(id, _)| id.clone()).collect::<Vec<_>>();
            let _publication = self.context.publication.lock().map_err(|_| anyhow!("page publication lock failed"))?;
            Store::open(&self.context.database)?.prune_reader_downloads(&self.context.account, &protected)?;
            Ok(())
        })();
        if collected.is_err() { self.context.reader_gc_needed.store(true, Ordering::Release); }
        collected
    }

    /// The new limit applies immediately to new acquisitions; active requests finish normally.
    pub fn set_concurrency(&self, count: usize) -> Result<()> {
        ensure!((1..=4).contains(&count), "download concurrency must be between one and four");
        self.context.capacity.lock().map_err(|_| anyhow!("download capacity lock failed"))?.limit = count;
        self.context.capacity_changed.notify_waiters();
        Ok(())
    }

    /// Reorder waiting retained pages without changing task generation, status, or in-flight work.
    pub fn prioritize(&mut self, task_id: &str) -> Result<()> {
        let priority = Store::open(&self.context.database)?
            .prioritize_download(&self.context.account, task_id)?;
        if let Some(control) = self.control(task_id) {
            control.priority.fetch_max(priority, Ordering::AcqRel);
        }
        self.context.capacity_changed.notify_waiters();
        Ok(())
    }

    pub fn enqueue(&mut self, request: DownloadRequest) -> Result<DownloadRecord> {
        self.enqueue_inner(request, false)
    }

    /// An explicit whole-chapter acquisition always rechecks access and the current source index.
    pub fn enqueue_fresh(&mut self, request: DownloadRequest) -> Result<DownloadRecord> {
        ensure!(request.retain && request.page_indices.is_none() && request.expected_revision.is_none(),
            "a fresh download requires an unpinned whole retained chapter");
        self.enqueue_inner(request, true)
    }

    fn enqueue_inner(&mut self, request: DownloadRequest, fresh: bool) -> Result<DownloadRecord> {
        ensure!(self.context.active.load(Ordering::Acquire), "download context is retired");
        let pause_revision = self.context.pause.revision.load(Ordering::Acquire);
        ensure!(pause_is_current(&self.context, pause_revision), "downloads are suspended");
        validate_request(&request)?;
        if !request.retain {
            if let Some(reader) = request.reader_generation {
                self.reader_lease(reader)?;
                if let Some(revision) = request.expected_revision.as_deref() {
                    self.pin_reader(reader, &request.episode_id, revision)?;
                }
            }
        }
        let id = request_id(&self.context.account, &request);
        let publication = self.context.publication.clone();
        let _publication = publication.lock().map_err(|_| anyhow!("page publication lock failed"))?;
        ensure!(pause_is_current(&self.context, pause_revision), "downloads are suspended");
        let store = Store::open(&self.context.database)?;
        let previous = store.load_download(&self.context.account, &id)?;
        if let Some(record) = previous.as_ref() {
            if matches!(record.status, DownloadStatus::Queued | DownloadStatus::Running) {
                let covered = self.control(&id).is_some_and(|existing| !existing.finished.load(Ordering::Acquire) && {
                    match (existing.selection.as_deref(), request.page_indices.as_deref()) {
                        (None, _) => true,
                        (Some(current), Some(requested)) => requested.iter().all(|index| current.binary_search(index).is_ok()),
                        (Some(_), None) => false,
                    }
                });
                if covered && !fresh { return Ok(record.clone()); }
            }
        }
        let timestamp = now();
        let mut record = previous.unwrap_or_else(|| DownloadRecord {
            id: id.clone(), comic_id: request.comic_id.clone(), episode_id: request.episode_id.clone(),
            revision: request.expected_revision.clone().unwrap_or_else(|| PENDING_REVISION.to_owned()),
            title: request.title.clone(), comic_title: request.comic_title.clone(),
            retained: request.retain, created_at: timestamp, ..Default::default()
        });
        record.generation = record.generation.checked_add(1).context("download generation overflow")?;
        record.status = DownloadStatus::Queued;
        record.error_code = None;
        record.terminal_event = None;
        record.updated_at = timestamp;
        record.retained |= request.retain;
        record.refresh_source |= fresh;
        ensure!(!record.refresh_source || request.retain && request.page_indices.is_none()
            && request.expected_revision.is_none(), "a pending source refresh requires a whole retained chapter");
        store.save_download(&self.context.account, &record)?;
        self.context.events.invalidate_task(&id);
        self.spawn(record.clone(), request, pause_revision);
        Ok(record)
    }

    pub fn pause(&mut self, task_id: &str) -> Result<()> {
        self.change_state(task_id, DownloadStatus::Paused, DownloadEventKind::Paused)
    }

    pub fn cancel(&mut self, task_id: &str) -> Result<()> {
        self.change_state(task_id, DownloadStatus::Cancelled, DownloadEventKind::Cancelled)
    }

    pub fn resume(&mut self, task_id: &str) -> Result<DownloadRecord> { self.restart(task_id) }
    pub fn retry(&mut self, task_id: &str) -> Result<DownloadRecord> { self.restart(task_id) }

    pub fn pause_all(&mut self) -> Result<()> {
        let records = self.records()?;
        for record in records {
            if matches!(record.status, DownloadStatus::Queued | DownloadStatus::Running) {
                self.pause(&record.id)?;
            }
        }
        Ok(())
    }

    /// New owner IDs must exceed all retired IDs; already active lower owners retain their leases.
    pub fn pin_reader(&self, reader_generation: u64, episode_id: &str, revision: &str) -> Result<()> {
        ensure!(!episode_id.is_empty() && !revision.is_empty(), "reader source identity is empty");
        self.reader_lease(reader_generation)?;
        self.context.reader_pins.lock().map_err(|_| anyhow!("reader pin lock failed"))?
            .entry(reader_generation).or_default().insert((episode_id.to_owned(), revision.to_owned()));
        Ok(())
    }

    /// Cancelling a reader cannot cancel an explicit retained download of the same episode.
    pub fn cancel_reader(&mut self, reader_generation: u64) -> Result<()> {
        let controls = self.context.registry.lock().map_err(|_| anyhow!("download registry lock failed"))?
            .controls.iter().filter(|(_, control)| {
            !control.retained && control.reader_generation == Some(reader_generation)
        }).map(|(id, control)| (id.clone(), control.clone())).collect::<Vec<_>>();
        self.context.readers.lock().map_err(|_| anyhow!("reader owner lock failed"))?.retire(reader_generation);
        for (_, control) in &controls {
            control.changed.send_replace(control.generation.load(Ordering::Acquire));
        }
        self.context.capacity_changed.notify_waiters();
        let mut failure = None;
        if !controls.is_empty() { match Store::open(&self.context.database) {
            Ok(store) => for (id, _) in &controls {
                match store.load_download(&self.context.account, id) {
                    Ok(Some(record)) if record.status != DownloadStatus::Cancelled => {
                        if let Err(error) = self.cancel(id) { failure.get_or_insert(error); }
                    },
                    Ok(_) => {},
                    Err(error) => { failure.get_or_insert(error); },
                }
            },
            Err(error) => { failure = Some(error); },
        } }
        {
            let mut registry = self.context.registry.lock().map_err(|_| anyhow!("download registry lock failed"))?;
            for (id, _) in &controls {
                registry.controls.remove(id);
                registry.partial_intents.remove(id);
                self.context.events.invalidate_task(id);
            }
        }
        self.context.reader_pins.lock().map_err(|_| anyhow!("reader pin lock failed"))?
            .remove(&reader_generation);
        if !controls.is_empty() { self.context.reader_gc_needed.store(true, Ordering::Release); }
        if let Err(error) = self.collect_reader_metadata() { failure.get_or_insert(error); }
        failure.map_or(Ok(()), Err)
    }

    /// Reconcile reader leases after the manager returns from a background operation.
    pub fn synchronize_reader_owner(&mut self, current: Option<(u64, String, String)>) -> Result<()> {
        if let Some((owner, episode, revision)) = current.as_ref() {
            ensure!(!episode.is_empty() && !revision.is_empty(), "reader source identity is empty");
            self.reader_lease(*owner)?;
        }
        let current_owner = current.as_ref().map(|(owner, _, _)| *owner);
        let mut previous = self.context.readers.lock().map_err(|_| anyhow!("reader owner lock failed"))?
            .active.keys().copied().filter(|owner| Some(*owner) != current_owner)
            .collect::<HashSet<_>>();
        {
            let pins = self.context.reader_pins.lock().map_err(|_| anyhow!("reader pin lock failed"))?;
            previous.extend(pins.keys().copied().filter(|owner| Some(*owner) != current_owner));
        }
        for owner in previous { self.cancel_reader(owner)?; }
        for record in self.records()? {
            if record.retained || record.status == DownloadStatus::Cancelled { continue; }
            let owned = current.as_ref().is_some_and(|(owner, episode, revision)| {
                record.episode_id == *episode && record.revision == *revision
                    && self.control(&record.id).is_some_and(|control| {
                        !control.retained && control.reader_generation == Some(*owner)
                            && control.generation.load(Ordering::Acquire) == record.generation
                    })
            });
            if !owned { self.cancel(&record.id)?; }
        }
        let mut pins = self.context.reader_pins.lock().map_err(|_| anyhow!("reader pin lock failed"))?;
        pins.clear();
        if let Some((owner, episode, revision)) = current {
            pins.entry(owner).or_default().insert((episode, revision));
        }
        drop(pins);
        self.context.reader_gc_needed.store(true, Ordering::Release);
        self.collect_reader_metadata()?;
        Ok(())
    }

    pub fn records(&self) -> Result<Vec<DownloadRecord>> {
        Store::open(&self.context.database)?.downloads(&self.context.account)
    }

    pub fn drain_events(&mut self, maximum: usize) -> Vec<DownloadEvent> {
        let maximum = maximum.min(1024);
        if maximum == 0 || !self.context.active.load(Ordering::Acquire) { return Vec::new(); }
        let notice = self.context.events.terminal_notice();
        let retry_due = self.context.events.storage_retry_due(Instant::now());
        let mut storage_attempted = false;
        let mut storage_succeeded = true;
        if retry_due && self.context.reader_gc_needed.load(Ordering::Acquire) {
            storage_attempted = true;
            storage_succeeded = self.collect_reader_metadata().is_ok();
        }
        let mut output = Vec::new();
        if retry_due && storage_succeeded && self.context.events.terminal_poll_due(Instant::now()) {
            storage_attempted = true;
            #[cfg(test)]
            self.context.events.terminal_polls.fetch_add(1, Ordering::SeqCst);
            let succeeded = if let Ok(store) = Store::open(&self.context.database) {
                let mut succeeded = self.flush_terminal_retries(&store);
                succeeded &= self.reconcile_terminal_events(&store);
                match store.pending_download_events(&self.context.account, maximum) {
                    Ok(pending) => {
                        if pending.len() == maximum {
                            if let Ok(mut state) = self.context.events.state.lock() { state.reconcile_terminal = true; }
                        }
                        for pending in pending {
                            let current = match store.load_download(&self.context.account, &pending.task_id) {
                                Ok(current) => current,
                                Err(_) => { succeeded = false; continue; },
                            };
                            let valid = current.as_ref().is_some_and(|record| {
                                record.generation == pending.generation
                                    && record.terminal_event.as_ref() == Some(&pending.payload)
                                    && (record.retained || self.control(&record.id).is_some_and(|control| {
                                        control.reader_is_active() && control.generation.load(Ordering::Acquire) == record.generation
                                    }))
                            });
                            if valid {
                                if let Some(kind) = terminal_kind(&pending.payload) {
                                    output.push(DownloadEvent { account_epoch: self.account_epoch(),
                                        task_id: pending.task_id, generation: pending.generation, kind });
                                }
                            }
                            // A failed acknowledgement leaves the durable event available for another poll.
                            if store.acknowledge_download_event(&self.context.account, pending.id).is_err() {
                                succeeded = false;
                            }
                        }
                    },
                    Err(_) => succeeded = false,
                }
                succeeded
            } else { false };
            storage_succeeded &= succeeded;
        }
        if let Ok(mut state) = self.context.events.state.lock() {
            while output.len() < maximum {
                let Some(pending) = state.progress.pop_front() else { break; };
                if is_current(&self.context, &pending.control, pending.event.account_epoch, pending.event.generation) {
                    output.push(pending.event);
                }
            }
        }
        if retry_due && storage_succeeded && output.len() < maximum {
            if let Some(succeeded) = self.replay_reader_indices(maximum, &mut output) {
                storage_attempted = true;
                storage_succeeded &= succeeded;
            }
        }
        let pending_local_work = self.context.reader_gc_needed.load(Ordering::Acquire)
            || self.context.events.state.lock().is_ok_and(|state| state.progress_dirty);
        if storage_attempted && (!storage_succeeded || !pending_local_work) {
            self.context.events.terminal_poll_finished(storage_succeeded, Instant::now(), notice);
        }
        output
    }

    fn flush_terminal_retries(&self, store: &Store) -> bool {
        let pending = self.context.events.state.lock().map(|mut state| {
            std::mem::take(&mut state.terminal_retries)
        }).unwrap_or_default();
        let mut succeeded = true;
        for pending in pending {
            let current = store.load_download(&self.context.account, &pending.task_id);
            match current {
                Ok(Some(record)) if record.generation == pending.generation
                    && record.terminal_event.as_ref() == Some(&pending.payload) => {
                    if store.record_download_event(&self.context.account, &pending.task_id,
                        pending.generation, &pending.payload).is_err() {
                        succeeded = false;
                        self.context.events.retry_terminal(pending);
                    }
                },
                Ok(Some(mut record)) if record.generation == pending.generation && record.terminal_event.is_none()
                    && matches!(record.status, DownloadStatus::Queued | DownloadStatus::Running)
                    && pending.payload.get("kind").and_then(serde_json::Value::as_str) == Some("failed") => {
                    let repaired = (|| -> Result<()> {
                        let _publication = self.context.publication.lock().map_err(|_| anyhow!("page publication lock failed"))?;
                        let current = store.load_download(&self.context.account, &record.id)?;
                        ensure!(current.as_ref().is_some_and(|current| current.generation == record.generation
                            && current.terminal_event.is_none() && matches!(current.status, DownloadStatus::Queued | DownloadStatus::Running)),
                            "the failed download state was superseded");
                        record.status = DownloadStatus::Failed;
                        record.error_code = pending.payload.get("code").and_then(serde_json::Value::as_str).map(str::to_owned);
                        record.terminal_event = Some(pending.payload.clone());
                        record.updated_at = now();
                        store.save_download(&self.context.account, &record)?;
                        store.record_download_event(&self.context.account, &record.id, record.generation, &pending.payload)
                    })();
                    if repaired.is_err() { succeeded = false; self.context.events.retry_terminal(pending); }
                },
                Ok(_) => {},
                Err(_) => { succeeded = false; self.context.events.retry_terminal(pending); },
            }
        }
        succeeded
    }

    fn reconcile_terminal_events(&self, store: &Store) -> bool {
        let cursor = self.context.events.state.lock().ok().and_then(|state| state.terminal_cursor.clone());
        let records = match store.terminal_downloads_page(&self.context.account, cursor.as_deref(), RECONCILE_PAGE_SIZE) {
            Ok(records) => records,
            Err(_) => {
                if let Ok(mut state) = self.context.events.state.lock() { state.reconcile_terminal = true; }
                return false;
            },
        };
        let mut failed = false;
        for record in &records {
            if let Some(payload) = record.terminal_event.as_ref() {
                if store.record_download_event(&self.context.account, &record.id, record.generation, payload).is_err() {
                    failed = true;
                }
            }
        }
        if let Ok(mut state) = self.context.events.state.lock() {
            if failed {
                state.reconcile_terminal = true;
            } else if records.len() == RECONCILE_PAGE_SIZE {
                state.terminal_cursor = records.last().map(|record| record.id.clone());
                state.reconcile_terminal = true;
            } else {
                state.terminal_cursor = None;
                state.reconcile_terminal = state.unconfirmed_state;
            }
        }
        !failed
    }

    fn replay_reader_indices(&self, maximum: usize, output: &mut Vec<DownloadEvent>) -> Option<bool> {
        let (mut replay, notice) = {
            let Ok(mut state) = self.context.events.state.lock() else { return Some(false); };
            if !state.progress_dirty { return None; }
            (state.replay.take().unwrap_or_default(), state.progress_notice)
        };
        let store = match Store::open(&self.context.database) {
            Ok(store) => store,
            Err(_) => return Some(false),
        };
        let mut scanned = 0;
        while output.len() < maximum && scanned < MAX_REPLAY_SCAN {
            let next = self.context.registry.lock().ok().and_then(|registry| registry.controls.iter()
                .filter(|(id, control)| !control.retained && control.reader_is_active()
                    && replay.after_task.as_ref().is_none_or(|after| id.as_str() > after.as_str()))
                .min_by(|(left, _), (right, _)| left.cmp(right)).map(|(id, control)| (id.clone(), control.clone())));
            let Some((id, control)) = next else {
                self.context.events.finish_progress_replay(notice, None);
                return Some(true);
            };
            replay.after_task = Some(id.clone());
            scanned += 1;
            let record = match store.load_download(&self.context.account, &id) {
                Ok(Some(record)) => record,
                Ok(None) => continue,
                Err(_) => return Some(false),
            };
            if record.generation != control.generation.load(Ordering::Acquire) { continue; }
            match store.load_index(&self.context.account, &record.episode_id, Some(&record.revision)) {
                Ok(Some(index)) => output.push(DownloadEvent { account_epoch: self.account_epoch(), task_id: id,
                    generation: record.generation, kind: DownloadEventKind::IndexReady { index } }),
                Ok(None) => {},
                Err(_) => return Some(false),
            }
        }
        self.context.events.finish_progress_replay(notice, Some(replay));
        Some(true)
    }

    /// Reconcile journals first, then resume only work that was running at interruption.
    pub fn recover(&mut self) -> Result<()> {
        let pause_revision = self.context.pause.revision.load(Ordering::Acquire);
        let may_restart = pause_is_current(&self.context, pause_revision);
        {
            let _publication = self.context.publication.lock()
                .map_err(|_| anyhow!("page publication lock failed"))?;
            cleanup_temporary_objects(&self.context.pages)?;
        }
        let mut store = Store::open(&self.context.database)?;
        let mut records = {
            let _publication = self.context.publication.lock()
                .map_err(|_| anyhow!("page publication lock failed"))?;
            for pending in store.pending_page_commits(&self.context.account)? {
                let current = store.load_download(&self.context.account, &pending.task_id)?;
                let eligible = current.as_ref().is_some_and(|task| {
                    task.generation == pending.generation && task.status == DownloadStatus::Running
                        && !task.refresh_source && task.episode_id == pending.receipt.episode_id
                        && task.revision == pending.receipt.revision
                });
                if !eligible {
                    if let ReceiptFileInspection::Unavailable(failure) =
                        inspect_receipt_file(&self.context.pages, &pending.receipt, false)? {
                        mark_unverifiable_records(&store, &self.context.account, &pending.receipt, &failure, true)?;
                        continue;
                    }
                }
                if eligible {
                    let checked = match revalidate_receipt_with_proof(&store, &self.context.account,
                        &self.context.pages, &pending.receipt, true) {
                        Err(error) if is_validation_unavailable(&error) => continue,
                        result => result?,
                    };
                    if let Some((receipt, proof)) = checked {
                        let proof = proof.context("recovered page has no decode proof")?;
                        store.begin_verified_page_commit_generation(&self.context.account, &pending.task_id,
                            pending.generation, &receipt, &proof)?;
                        store.commit_verified_page_generation(&self.context.account, &pending.task_id,
                            pending.generation, &receipt, &proof)?;
                        continue;
                    }
                }
                store.discard_page_commit(&self.context.account, &pending.task_id, &pending.receipt.page_id)?;
            }
            let snapshot = store.downloads(&self.context.account)?;
            for record in &snapshot {
                for receipt in store.page_receipts(&self.context.account, &record.episode_id, &record.revision)? {
                    match revalidate_receipt_locked(&store, &self.context.account, &self.context.pages, &receipt) {
                        Ok(Some(_)) => {},
                        Ok(None) => store.invalidate_page(&self.context.account, &receipt.episode_id,
                            &receipt.revision, &receipt.page_id)?,
                        Err(error) if is_validation_unavailable(&error) => {},
                        Err(error) => return Err(error),
                    }
                }
            }
            for record in snapshot.iter().filter(|record| record.status == DownloadStatus::Completed) {
                if let Err(error) = reconcile_completed_record(&self.context, &store, record) {
                    if !is_validation_unavailable(&error) { return Err(error); }
                }
            }
            cleanup_unreferenced_objects(&self.context.pages, &store, &self.context.account)?;
            store.downloads(&self.context.account)?
        };
        drop(store);
        records.sort_by_key(|record| std::cmp::Reverse(if record.retained
            && matches!(record.status, DownloadStatus::Queued | DownloadStatus::Running) {
                record.priority
            } else { 0 }));
        for record in records {
            if !record.retained && matches!(record.status, DownloadStatus::Queued | DownloadStatus::Running) {
                self.change_state(&record.id, DownloadStatus::Cancelled, DownloadEventKind::Cancelled)?;
            } else if record.retained && matches!(record.status, DownloadStatus::Queued | DownloadStatus::Running)
                && may_restart && pause_is_current(&self.context, pause_revision) {
                if let Err(error) = self.restart_at_revision(&record.id, pause_revision) {
                    if pause_is_current(&self.context, pause_revision) { return Err(error); }
                }
            }
        }
        self.context.reader_gc_needed.store(true, Ordering::Release);
        self.collect_reader_metadata()?;
        if let Ok(mut state) = self.context.events.state.lock() {
            state.unconfirmed_state = false;
            state.reconcile_terminal = true;
            state.terminal_cursor = None;
        }
        self.context.events.terminal_available();
        Ok(())
    }

    /// Returns only an account-owned, receipt-backed object whose content still matches its digest.
    pub fn local_page(&self, episode_id: &str, revision: &str, page_id: &str) -> Result<Option<PathBuf>> {
        self.page_resolver().local_page(episode_id, revision, page_id)
    }

    /// Removal is explicit and refuses to delete pages still owned by an open reader.
    pub fn remove_episode(&mut self, episode_id: &str, revision: &str) -> Result<()> {
        let pinned = self.context.reader_pins.lock().map_err(|_| anyhow!("reader pin lock failed"))?
            .values().any(|pins| pins.contains(&(episode_id.to_owned(), revision.to_owned())));
        if pinned { return Err(DownloadControlError::ReaderInUse.into()); }
        let mut store = Store::open(&self.context.database)?;
        let records = store.downloads(&self.context.account)?;
        for record in records.iter().filter(|record| record.episode_id == episode_id
            && (record.revision == revision || record.revision == PENDING_REVISION)) {
            self.cancel(&record.id)?;
        }
        let publication = self.context.publication.clone();
        let _publication = publication.lock().map_err(|_| anyhow!("page publication lock failed"))?;
        let files = store.page_receipts(&self.context.account, episode_id, revision)?;
        store.remove_episode(&self.context.account, episode_id, revision)?;
        {
            let mut registry = self.context.registry.lock().map_err(|_| anyhow!("download registry lock failed"))?;
            for record in records.iter().filter(|record| record.episode_id == episode_id
                && (record.revision == revision || record.revision == PENDING_REVISION)) {
                registry.controls.remove(&record.id);
                registry.partial_intents.remove(&record.id);
                self.context.events.invalidate_task(&record.id);
            }
        }
        for receipt in files {
            let shared = store.page_object_referenced(&self.context.account, &receipt.file_name)?;
            if !shared {
                let path = receipt_path(&self.context.pages, &receipt.file_name)?;
                match fs::remove_file(path) {
                    Ok(()) => {},
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                    Err(error) => return Err(error).context("remove cached page object"),
                }
            }
        }
        Ok(())
    }

    /// Invalidates all old-account callbacks while retaining their durable local checkpoints.
    pub fn invalidate_account(&mut self, new_epoch: u64) -> Result<()> {
        self.pause_all()?;
        self.context.active.store(false, Ordering::Release);
        self.context.epoch.store(new_epoch, Ordering::Release);
        if let Ok(mut registry) = self.context.registry.lock() {
            for control in registry.controls.values() {
                control.change(control.generation.load(Ordering::Acquire).saturating_add(1));
            }
            registry.controls.clear();
            registry.partial_intents.clear();
        }
        if let Ok(mut readers) = self.context.readers.lock() {
            for lease in readers.active.values() { lease.active.store(false, Ordering::Release); }
            readers.active.clear();
        }
        self.context.events.close();
        self.context.capacity_changed.notify_waiters();
        Ok(())
    }

    fn restart(&mut self, task_id: &str) -> Result<DownloadRecord> {
        let pause_revision = self.context.pause.revision.load(Ordering::Acquire);
        self.restart_at_revision(task_id, pause_revision)
    }

    fn restart_at_revision(&mut self, task_id: &str, pause_revision: u64) -> Result<DownloadRecord> {
        ensure!(self.context.active.load(Ordering::Acquire), "download context is retired");
        ensure!(pause_is_current(&self.context, pause_revision), "downloads are suspended");
        let publication = self.context.publication.clone();
        let _publication = publication.lock().map_err(|_| anyhow!("page publication lock failed"))?;
        ensure!(pause_is_current(&self.context, pause_revision), "downloads are suspended");
        let store = Store::open(&self.context.database)?;
        let mut record = store.load_download(&self.context.account, task_id)?.context("download task does not exist")?;
        let control = self.control(task_id);
        let selection = {
            let registry = self.context.registry.lock().map_err(|_| anyhow!("download registry lock failed"))?;
            control.as_ref().and_then(|control| control.selection.clone())
                .or_else(|| registry.partial_intents.get(task_id).cloned())
        };
        let mut request = DownloadRequest {
            comic_id: record.comic_id.clone(), episode_id: record.episode_id.clone(),
            title: record.title.clone(), comic_title: record.comic_title.clone(), retain: record.retained,
            expected_revision: (!record.refresh_source && record.revision != PENDING_REVISION)
                .then(|| record.revision.clone()),
            reader_generation: control.as_ref().and_then(|control| control.reader_generation),
            page_indices: selection.as_ref().map(|selection| selection.to_vec()),
        };
        if record.refresh_source {
            ensure!(record.retained, "a pending source refresh requires a retained task");
            request.expected_revision = None;
            request.page_indices = None;
        }
        if !record.retained {
            ensure!(request.reader_generation.is_some_and(|owner| {
                !self.reader_is_retired(owner)
                    && control.as_ref().is_some_and(|control|
                        !control.retained && control.reader_generation == Some(owner) && control.reader_is_active())
            }), "reader prefetch owner is no longer active");
        }
        validate_request(&request)?;
        record.generation = record.generation.checked_add(1).context("download generation overflow")?;
        record.status = DownloadStatus::Queued;
        record.error_code = None;
        record.terminal_event = None;
        record.updated_at = now();
        store.save_download(&self.context.account, &record)?;
        self.context.events.invalidate_task(task_id);
        self.spawn(record.clone(), request, pause_revision);
        Ok(record)
    }

    fn change_state(&mut self, task_id: &str, status: DownloadStatus, event: DownloadEventKind) -> Result<()> {
        let _publication = self.context.publication.lock()
            .map_err(|_| anyhow!("page publication lock failed"))?;
        let store = Store::open(&self.context.database)?;
        let mut record = store.load_download(&self.context.account, task_id)?.context("download task does not exist")?;
        if record.status == DownloadStatus::Completed && status == DownloadStatus::Paused && !record.refresh_source { return Ok(()); }
        record.generation = record.generation.checked_add(1).context("download generation overflow")?;
        record.status = status;
        record.error_code = None;
        record.terminal_event = terminal_payload(&event);
        record.updated_at = now();
        store.save_download(&self.context.account, &record)?;
        if let Some(control) = self.control(task_id) { control.change(record.generation); }
        self.context.events.invalidate_task(task_id);
        persist_terminal(&self.context, &record, &event);
        self.context.capacity_changed.notify_waiters();
        Ok(())
    }

    fn spawn(&mut self, record: DownloadRecord, request: DownloadRequest, pause_revision: u64) {
        let lease = if request.retain { None } else {
            request.reader_generation.and_then(|owner| self.reader_lease(owner).ok())
        };
        let selection = request.page_indices.as_ref().map(|selection| {
            let mut selected = selection.clone(); selected.sort_unstable(); selected.dedup();
            Arc::<[usize]>::from(selected)
        });
        let control = Arc::new(TaskControl::scoped(record.generation, request.reader_generation,
            record.retained, pause_revision, record.priority, selection.clone(), lease));
        {
            let mut registry = self.context.registry.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(previous) = registry.controls.insert(record.id.clone(), control.clone()) {
                previous.change(record.generation);
            }
            if record.retained {
                if let Some(selection) = selection { registry.partial_intents.insert(record.id.clone(), selection); }
                else { registry.partial_intents.remove(&record.id); }
            }
        }
        let context = self.context.clone();
        let epoch = self.account_epoch();
        self.handle.spawn(async move {
            let _completion = WorkerCompletion { context: context.clone(), control: control.clone(), task_id: record.id.clone() };
            let result = run_episode(context.clone(), control.clone(), epoch, record.clone(), request).await;
            if let Err(failure) = result {
                if failure.code == "download_cancelled" || !is_current(&context, &control, epoch, record.generation) { return; }
                let mut completed = false;
                let (update_context, update_control, update_record, update_failure) =
                    (context.clone(), control.clone(), record.clone(), failure.clone());
                let updated = run_download_work(context.clone(), control.clone(), epoch, record.generation, move |work| {
                    let check = || check_work(&update_context, &update_control, epoch, update_record.generation, work);
                    check().map_err(storage_failure)?;
                    let _publication = update_context.publication.lock()
                        .map_err(|_| AcquisitionFailure::new("page_publication_failed", false))?;
                    check().map_err(storage_failure)?;
                    let store = Store::open(&update_context.database).map_err(storage_failure)?;
                    let Some(mut current) = store.load_download(&update_context.account, &update_record.id)
                        .map_err(storage_failure)? else { return Err(cancelled()); };
                    if current.generation != update_record.generation { return Err(cancelled()); }
                    let completed = current.status == DownloadStatus::Completed
                        && verify_record_with_check(&update_context, &current, Some(&check)).map_err(storage_failure)?;
                    if !completed {
                        current.status = DownloadStatus::Failed;
                        current.error_code = Some(update_failure.code.clone());
                        current.terminal_event = terminal_payload(&DownloadEventKind::Failed {
                            code: update_failure.code, retryable: update_failure.retryable,
                        });
                        current.updated_at = now();
                        check().map_err(storage_failure)?;
                        store.save_download(&update_context.account, &current).map_err(storage_failure)?;
                    }
                    Ok(completed)
                }).await;
                match updated {
                    Ok(value) => completed = value,
                    Err(error) if error.code == "download_cancelled" => return,
                    Err(_) => {
                        if let Ok(mut state) = context.events.state.lock() {
                            state.unconfirmed_state = true;
                            state.reconcile_terminal = true;
                        }
                    },
                }
                if completed {
                    publish(&context, &control, epoch, &record, DownloadEventKind::Completed);
                    return;
                }
                publish(&context, &control, epoch, &record, DownloadEventKind::Failed {
                    code: failure.code, retryable: failure.retryable,
                });
            }
        });
    }
}

impl Drop for DownloadManager {
    fn drop(&mut self) {
        self.context.active.store(false, Ordering::Release);
        if let Ok(mut registry) = self.context.registry.lock() {
            for control in registry.controls.values() {
                control.change(control.generation.load(Ordering::Acquire).saturating_add(1));
            }
            registry.controls.clear();
            registry.partial_intents.clear();
        }
        if let Ok(mut readers) = self.context.readers.lock() {
            for lease in readers.active.values() { lease.active.store(false, Ordering::Release); }
            readers.active.clear();
        }
        self.context.events.close();
        self.context.capacity_changed.notify_waiters();
    }
}

type ValidationCheck<'a> = dyn Fn() -> Result<()> + 'a;

fn check_validation(check: Option<&ValidationCheck<'_>>) -> Result<()> {
    if let Some(check) = check { check()?; }
    Ok(())
}

fn check_work(
    context: &ContextState, control: &TaskControl, epoch: u64, generation: u64, work: &WorkCancellation,
) -> Result<()> {
    if work.is_cancelled() { return Err(cancelled().into()); }
    current(context, control, epoch, generation).map_err(anyhow::Error::new)
}

async fn run_download_work<T, F>(
    context: Arc<ContextState>, control: Arc<TaskControl>, epoch: u64, generation: u64, job: F,
) -> std::result::Result<T, AcquisitionFailure>
where
    T: Send + 'static,
    F: FnOnce(&WorkCancellation) -> std::result::Result<T, AcquisitionFailure> + Send + 'static,
{
    let mut signal = control.changed.subscribe();
    let mut suspension = context.pause.changed.subscribe();
    current(&context, &control, epoch, generation)?;
    tokio::select! {
        biased;
        _ = suspension.changed() => Err(cancelled()),
        _ = signal.changed() => Err(cancelled()),
        result = work::run_blocking(HEAVY_WORK_BYTES, job) => {
            result.map_err(|error| match error {
                WorkError::Cancelled => cancelled(),
                WorkError::BudgetExceeded => AcquisitionFailure::new("download_work_memory_limit", false),
                WorkError::BudgetUnavailable => AcquisitionFailure::new("download_work_budget_unavailable", true),
                WorkError::WorkerFailed => AcquisitionFailure::new("download_work_failed", true),
            })?
        },
    }
}

async fn run_episode(
    context: Arc<ContextState>, control: Arc<TaskControl>, epoch: u64,
    mut record: DownloadRecord, request: DownloadRequest,
) -> std::result::Result<(), AcquisitionFailure> {
    current(&context, &control, epoch, record.generation)?;
    let mut preparation = Some(acquire_slot(context.clone(), control.clone(), epoch,
        record.generation, SlotKind::Preparation).await?);
    if !record.refresh_source && record.revision != PENDING_REVISION && request.page_indices.is_none() {
        let (verify_context, verify_control, mut cached) = (context.clone(), control.clone(), record.clone());
        let slot = preparation.take().expect("cached verification owns preparation capacity");
        let generation = record.generation;
        let (completed, slot) = run_download_work(context.clone(), control.clone(), epoch, generation, move |work| {
            let check = || check_work(&verify_context, &verify_control, epoch, generation, work);
            let completed = complete_cached_record(&verify_context, &verify_control, epoch, &mut cached, Some(&check))?;
            Ok((completed, slot))
        }).await?;
        preparation = Some(slot);
        if completed {
            current(&context, &control, epoch, record.generation)?;
            publish(&context, &control, epoch, &record, DownloadEventKind::Completed);
            return Ok(());
        }
    }
    let mut signal = control.changed.subscribe();
    let mut suspension = context.pause.changed.subscribe();
    current(&context, &control, epoch, record.generation)?;
    let index = tokio::select! {
        biased;
        _ = suspension.changed() => return Err(cancelled()),
        _ = signal.changed() => return Err(cancelled()),
        result = context.source.image_index_for(&record.comic_id, &record.episode_id) => result?,
    };
    current(&context, &control, epoch, record.generation)?;
    validate_index(&index, &record.episode_id)?;
    if !record.refresh_source && record.revision != PENDING_REVISION && record.revision != index.revision {
        return Err(AcquisitionFailure::new("index_revision_changed", false));
    }
    let selected = match request.page_indices.as_ref() {
        Some(indices) => {
            let mut indices = indices.clone();
            indices.sort_unstable(); indices.dedup();
            if indices.is_empty() || indices.iter().any(|position| *position >= index.images.len()) {
                return Err(AcquisitionFailure::new("download_page_selection_invalid", false));
            }
            indices
        },
        None => (0..index.images.len()).collect::<Vec<_>>(),
    };
    record.revision = index.revision.clone();
    record.total_pages = index.images.len() as u32;
    record.status = DownloadStatus::Running;
    record.updated_at = now();
    let (page_context, page_control, mut prepared_record, page_index) =
        (context.clone(), control.clone(), record.clone(), index.clone());
    let slot = preparation.take().expect("page selection owns preparation capacity");
    let generation = record.generation;
    let (selection, slot) = run_download_work(context.clone(), control.clone(), epoch, generation, move |work| {
        let check = || check_work(&page_context, &page_control, epoch, generation, work);
        let (missing, cached) = prepare_episode_pages(&page_context, &page_control, epoch,
            &mut prepared_record, &page_index, &selected, Some(&check))?;
        Ok(((prepared_record, missing, cached), slot))
    }).await?;
    preparation = Some(slot);
    let (prepared_record, missing, cached) = selection;
    record = prepared_record;
    if !record.retained {
        if let Some(owner) = request.reader_generation {
            let mut pins = context.reader_pins.lock()
                .map_err(|_| AcquisitionFailure::new("reader_pin_failed", false))?;
            current(&context, &control, epoch, record.generation)?;
            pins.entry(owner).or_default().insert((record.episode_id.clone(), record.revision.clone()));
        }
    }
    publish(&context, &control, epoch, &record, DownloadEventKind::Started);
    let public_index = ImageIndex { episode_id: index.episode_id.clone(), revision: index.revision.clone(),
        images: index.images.clone(), paths: Vec::new(), extra: serde_json::Value::Null };
    publish(&context, &control, epoch, &record, DownloadEventKind::IndexReady { index: public_index });
    for (page_index, receipt) in cached {
        current(&context, &control, epoch, record.generation)?;
        let counts = Store::open(&context.database).map_err(storage_failure)?
            .load_download(&context.account, &record.id).map_err(storage_failure)?;
        publish(&context, &control, epoch, &record, DownloadEventKind::PageReady {
            episode_id: record.episode_id.clone(), revision: record.revision.clone(),
            page_id: receipt.page_id.clone(), page_index, path: context.pages.join(&receipt.file_name),
            retained: receipt.retained,
            completed_pages: counts.as_ref().map_or(0, |task| task.completed_pages),
            total_pages: record.total_pages,
        });
    }
    let index = Arc::new(index);
    let batch_size = context.source.page_batch_size().clamp(1, MAX_PAGE_BATCH);
    let batched = batch_size > 1;
    let chunk_size = if batched { batch_size } else { missing.len().max(1) };
    for page_indices in missing.chunks(chunk_size) {
        current(&context, &control, epoch, record.generation)?;
        if batched && preparation.is_none() {
            preparation = Some(acquire_slot(context.clone(), control.clone(), epoch,
                record.generation, SlotKind::Preparation).await?);
        }
        // Another reader or retained task may have committed pages while this chapter waited.
        let (check_context, check_control, check_record, check_index, check_pages) =
            (context.clone(), control.clone(), record.clone(), index.clone(), page_indices.to_vec());
        let slot = preparation.take();
        let generation = record.generation;
        let (checked, slot) = run_download_work(context.clone(), control.clone(), epoch, generation, move |work| {
            let check = || check_work(&check_context, &check_control, epoch, generation, work);
            let checked = recheck_missing_pages(&check_context, &check_control, epoch,
                &check_record, &check_index, &check_pages, Some(&check))?;
            Ok((checked, slot))
        }).await?;
        preparation = slot;
        let page_indices = checked;
        if page_indices.is_empty() {
            drop(preparation.take());
            continue;
        }
        let batch = if batched {
            current(&context, &control, epoch, record.generation)?;
            let prepared = tokio::select! {
                biased;
                _ = suspension.changed() => return Err(cancelled()),
                _ = signal.changed() => return Err(cancelled()),
                result = context.source.prepare_page_batch(&index, &page_indices) => result?,
            };
            current(&context, &control, epoch, record.generation)?;
            Some(prepared)
        } else {
            None
        };
        run_page_batch(context.clone(), control.clone(), epoch, &record, index.clone(),
            &page_indices, batch, preparation.take()).await?;
    }
    drop(preparation);
    current(&context, &control, epoch, record.generation)?;
    let slot = acquire_slot(context.clone(), control.clone(), epoch, record.generation, SlotKind::Page).await?;
    let (finish_context, finish_control, finish_record) = (context.clone(), control.clone(), record.clone());
    let partial = request.page_indices.is_some();
    let generation = record.generation;
    let kind = run_download_work(context.clone(), control.clone(), epoch, generation, move |work| {
        let _slot = slot;
        let check = || check_work(&finish_context, &finish_control, epoch, generation, work);
        finish_episode(&finish_context, &finish_control, epoch, &finish_record, &index, partial, Some(&check))
    }).await?;
    current(&context, &control, epoch, record.generation)?;
    publish(&context, &control, epoch, &record, kind);
    Ok(())
}

fn complete_cached_record(
    context: &ContextState, control: &TaskControl, epoch: u64, record: &mut DownloadRecord,
    check: Option<&ValidationCheck<'_>>,
) -> std::result::Result<bool, AcquisitionFailure> {
    if record.refresh_source { return Ok(false); }
    let _publication = context.publication.lock()
        .map_err(|_| AcquisitionFailure::new("page_publication_failed", false))?;
    current(context, control, epoch, record.generation)?;
    let store = Store::open(&context.database).map_err(storage_failure)?;
    let Some(index) = store.load_index(&context.account, &record.episode_id, Some(&record.revision))
        .map_err(storage_failure)? else { return Ok(false); };
    if !verify_record_with_check(context, record, check).map_err(storage_failure)? { return Ok(false); }
    let receipts = store.page_receipts(&context.account, &record.episode_id, &record.revision)
        .map_err(storage_failure)?;
    let receipts = receipts.iter().filter(|receipt| index.images.get(receipt.page_index)
        .is_some_and(|page| page.id == receipt.page_id)).collect::<Vec<_>>();
    if record.retained {
        for receipt in &receipts {
            check_validation(check).map_err(storage_failure)?;
            store.retain_page(&context.account, &record.episode_id, &record.revision, &receipt.page_id)
                .map_err(storage_failure)?;
        }
    }
    record.status = DownloadStatus::Completed;
    record.terminal_event = terminal_payload(&DownloadEventKind::Completed);
    record.total_pages = index.images.len() as u32;
    record.completed_pages = record.total_pages;
    record.bytes = receipts.iter().map(|receipt| receipt.bytes).sum();
    record.updated_at = now();
    current(context, control, epoch, record.generation)?;
    store.save_download(&context.account, record).map_err(storage_failure)?;
    Ok(true)
}

fn reconcile_completed_record(context: &ContextState, store: &Store, record: &DownloadRecord) -> Result<()> {
    let Some(mut updated) = store.load_download(&context.account, &record.id)? else { return Ok(()); };
    if updated.status != DownloadStatus::Completed { return Ok(()); }
    if updated.refresh_source {
        updated.status = DownloadStatus::Queued;
        updated.terminal_event = None;
        updated.error_code = None;
        updated.updated_at = now();
        return store.save_download(&context.account, &updated);
    }
    let index = store.load_index(&context.account, &updated.episode_id, Some(&updated.revision))?;
    let complete = verify_record(context, &updated)?;
    let receipts = store.page_receipts(&context.account, &updated.episode_id, &updated.revision)?;
    let valid = receipts.iter().filter(|receipt| index.as_ref().and_then(|index| index.images.get(receipt.page_index))
        .is_some_and(|page| page.id == receipt.page_id)).collect::<Vec<_>>();
    updated.total_pages = index.as_ref().map_or(updated.total_pages, |index| index.images.len() as u32);
    updated.completed_pages = valid.len() as u32;
    updated.bytes = valid.iter().map(|receipt| receipt.bytes).sum();
    updated.updated_at = now();
    if complete {
        if updated.retained {
            for receipt in valid {
                store.retain_page(&context.account, &updated.episode_id, &updated.revision, &receipt.page_id)?;
            }
        }
        updated.error_code = None;
    } else {
        updated.status = DownloadStatus::Failed;
        updated.error_code = Some("local_page_missing".to_owned());
        updated.terminal_event = terminal_payload(&DownloadEventKind::Failed { code: "local_page_missing".to_owned(), retryable: true });
    }
    store.save_download(&context.account, &updated)
}

type CachedPages = Vec<(usize, PageReceipt)>;

fn prepare_episode_pages(
    context: &ContextState, control: &TaskControl, epoch: u64, record: &mut DownloadRecord,
    index: &ImageIndex, selected: &[usize],
    check: Option<&ValidationCheck<'_>>,
) -> std::result::Result<(Vec<usize>, CachedPages), AcquisitionFailure> {
    let _publication = context.publication.lock()
        .map_err(|_| AcquisitionFailure::new("page_publication_failed", false))?;
    current(context, control, epoch, record.generation)?;
    let store = Store::open(&context.database).map_err(storage_failure)?;
    let mut available = HashMap::new();
    for receipt in store.page_receipts(&context.account, &record.episode_id, &record.revision).map_err(storage_failure)? {
        check_validation(check).map_err(storage_failure)?;
        current(context, control, epoch, record.generation)?;
        if !index.images.get(receipt.page_index).is_some_and(|page| page.id == receipt.page_id) {
            store.invalidate_page(&context.account, &record.episode_id, &record.revision, &receipt.page_id)
                .map_err(storage_failure)?;
        } else if let Some(receipt) = revalidate_receipt_locked_with_check(&store, &context.account, &context.pages, &receipt, check)
            .map_err(storage_failure)? {
            available.insert(receipt.page_index, receipt);
        } else {
            store.invalidate_page(&context.account, &record.episode_id, &record.revision, &receipt.page_id)
                .map_err(storage_failure)?;
        }
    }
    current(context, control, epoch, record.generation)?;
    store.save_index(&context.account, index).map_err(storage_failure)?;
    record.completed_pages = available.len() as u32;
    record.bytes = available.values().map(|receipt| receipt.bytes).sum();
    let mut missing = Vec::new();
    let mut cached = Vec::new();
    for &page_index in selected {
        check_validation(check).map_err(storage_failure)?;
        current(context, control, epoch, record.generation)?;
        if let Some(receipt) = available.get_mut(&page_index) {
            if record.retained && !receipt.retained {
                store.retain_page(&context.account, &record.episode_id, &record.revision, &receipt.page_id)
                    .map_err(storage_failure)?;
                receipt.retained = true;
            }
            cached.push((page_index, receipt.clone()));
        } else {
            missing.push(page_index);
        }
    }
    current(context, control, epoch, record.generation)?;
    record.refresh_source = false;
    record.terminal_event = None;
    store.save_download(&context.account, record).map_err(storage_failure)?;
    Ok((missing, cached))
}

fn finish_episode(
    context: &ContextState, control: &TaskControl, epoch: u64, record: &DownloadRecord,
    index: &ImageIndex, partial: bool,
    check: Option<&ValidationCheck<'_>>,
) -> std::result::Result<DownloadEventKind, AcquisitionFailure> {
    let _publication = context.publication.lock()
        .map_err(|_| AcquisitionFailure::new("page_publication_failed", false))?;
    current(context, control, epoch, record.generation)?;
    let store = Store::open(&context.database).map_err(storage_failure)?;
    let Some(mut updated) = store.load_download(&context.account, &record.id).map_err(storage_failure)? else {
        return Err(AcquisitionFailure::new("download_task_missing", false));
    };
    if updated.generation != record.generation || updated.status != DownloadStatus::Running {
        return Err(cancelled());
    }
    if updated.refresh_source { return Err(AcquisitionFailure::new("download_source_refresh_pending", true)); }
    let complete = verify_record_with_check(context, &updated, check).map_err(storage_failure)?;
    let receipts = store.page_receipts(&context.account, &record.episode_id, &record.revision)
        .map_err(storage_failure)?;
    updated.total_pages = index.images.len() as u32;
    updated.completed_pages = receipts.iter().filter(|receipt| index.images.get(receipt.page_index)
        .is_some_and(|page| page.id == receipt.page_id)).count() as u32;
    updated.bytes = receipts.iter().filter(|receipt| index.images.get(receipt.page_index)
        .is_some_and(|page| page.id == receipt.page_id)).map(|receipt| receipt.bytes).sum();
    let kind = if complete {
        if updated.retained {
            for receipt in &receipts {
                check_validation(check).map_err(storage_failure)?;
                store.retain_page(&context.account, &updated.episode_id, &updated.revision, &receipt.page_id)
                    .map_err(storage_failure)?;
            }
        }
        updated.status = DownloadStatus::Completed;
        DownloadEventKind::Completed
    } else if partial {
        updated.status = DownloadStatus::Paused;
        DownloadEventKind::PrefetchReady
    } else {
        return Err(AcquisitionFailure::new("download_completion_unconfirmed", true));
    };
    updated.updated_at = now();
    updated.terminal_event = terminal_payload(&kind);
    current(context, control, epoch, record.generation)?;
    store.save_download(&context.account, &updated).map_err(storage_failure)?;
    Ok(kind)
}

fn recheck_missing_pages(
    context: &ContextState, control: &Arc<TaskControl>, epoch: u64, record: &DownloadRecord,
    index: &ImageIndex, page_indices: &[usize],
    check: Option<&ValidationCheck<'_>>,
) -> std::result::Result<Vec<usize>, AcquisitionFailure> {
    let _publication = context.publication.lock()
        .map_err(|_| AcquisitionFailure::new("page_publication_failed", false))?;
    current(context, control, epoch, record.generation)?;
    let store = Store::open(&context.database).map_err(storage_failure)?;
    let mut missing = Vec::new();
    let mut cached = Vec::new();
    let mut invalidated = false;
    for &page_index in page_indices {
        check_validation(check).map_err(storage_failure)?;
        current(context, control, epoch, record.generation)?;
        let page = &index.images[page_index];
        if let Some(receipt) = store.page_receipt(&context.account, &record.episode_id,
            &record.revision, &page.id).map_err(storage_failure)?
        {
            let verified = if receipt.page_index == page_index {
                revalidate_receipt_locked_with_check(&store, &context.account, &context.pages, &receipt, check).map_err(storage_failure)?
            } else { None };
            if let Some(mut receipt) = verified {
                if record.retained && !receipt.retained {
                    store.retain_page(&context.account, &record.episode_id, &record.revision, &page.id)
                        .map_err(storage_failure)?;
                    receipt.retained = true;
                }
                cached.push((page_index, receipt));
                continue;
            }
            store.invalidate_page(&context.account, &record.episode_id, &record.revision, &page.id)
                .map_err(storage_failure)?;
            invalidated = true;
        }
        missing.push(page_index);
    }
    if cached.is_empty() && !invalidated { return Ok(missing); }
    current(context, control, epoch, record.generation)?;
    let Some(mut updated) = store.load_download(&context.account, &record.id).map_err(storage_failure)? else {
        return Err(AcquisitionFailure::new("download_task_missing", false));
    };
    if updated.generation != record.generation { return Err(cancelled()); }
    let receipts = store.page_receipts(&context.account, &record.episode_id, &record.revision)
        .map_err(storage_failure)?;
    updated.completed_pages = receipts.iter().filter(|receipt| index.images.get(receipt.page_index)
        .is_some_and(|page| page.id == receipt.page_id)).count() as u32;
    updated.bytes = receipts.iter().filter(|receipt| index.images.get(receipt.page_index)
        .is_some_and(|page| page.id == receipt.page_id)).map(|receipt| receipt.bytes).sum();
    updated.status = DownloadStatus::Running;
    updated.error_code = None;
    updated.terminal_event = None;
    updated.updated_at = now();
    current(context, control, epoch, record.generation)?;
    store.save_download(&context.account, &updated).map_err(storage_failure)?;
    for (page_index, receipt) in cached {
        publish(context, control, epoch, record, DownloadEventKind::PageReady {
            episode_id: record.episode_id.clone(), revision: record.revision.clone(),
            page_id: receipt.page_id, page_index, path: context.pages.join(receipt.file_name),
            retained: receipt.retained, completed_pages: updated.completed_pages,
            total_pages: record.total_pages,
        });
    }
    Ok(missing)
}

async fn run_page_batch(
    context: Arc<ContextState>, control: Arc<TaskControl>, epoch: u64,
    record: &DownloadRecord, index: Arc<ImageIndex>, page_indices: &[usize],
    batch: Option<Arc<dyn PreparedPageBatch>>, preparation: Option<Slot>,
) -> std::result::Result<(), AcquisitionFailure> {
    let mut signal = control.changed.subscribe();
    let mut suspension = context.pause.changed.subscribe();
    current(&context, &control, epoch, record.generation)?;
    let mut next = 0;
    let mut initial = Vec::new();
    while next < page_indices.len() && initial.len() < MAX_PAGE_JOBS {
        let waiting = queue_slot(context.clone(), &control, epoch, record.generation, SlotKind::Page)?;
        initial.push((page_indices[next], waiting));
        next += 1;
    }
    // Publish the page tickets before releasing preparation, without waiting for a second permit.
    // This closes the handoff gap in which another chapter could otherwise prepare ahead of them.
    drop(preparation);
    current(&context, &control, epoch, record.generation)?;
    let mut jobs = JoinSet::new();
    for (page_index, waiting) in initial {
        spawn_page_job(&mut jobs, &context, &control, epoch, record, &index, &batch, page_index, waiting);
    }
    while !jobs.is_empty() {
        current(&context, &control, epoch, record.generation)?;
        let outcome = tokio::select! {
            biased;
            _ = suspension.changed() => { jobs.abort_all(); return Err(cancelled()); },
            _ = signal.changed() => { jobs.abort_all(); return Err(cancelled()); },
            result = jobs.join_next() => result,
        };
        match outcome {
            Some(Ok(Ok(()))) => {},
            Some(Ok(Err(error))) => { jobs.abort_all(); return Err(error); },
            Some(Err(_)) => { jobs.abort_all(); return Err(AcquisitionFailure::new("download_worker_failed", true)); },
            None => {},
        }
        while next < page_indices.len() && jobs.len() < MAX_PAGE_JOBS {
            current(&context, &control, epoch, record.generation)?;
            let waiting = queue_slot(context.clone(), &control, epoch, record.generation, SlotKind::Page)?;
            let page_index = page_indices[next]; next += 1;
            spawn_page_job(&mut jobs, &context, &control, epoch, record, &index, &batch, page_index, waiting);
        }
    }
    Ok(())
}

fn spawn_page_job(
    jobs: &mut JoinSet<std::result::Result<(), AcquisitionFailure>>,
    context: &Arc<ContextState>, control: &Arc<TaskControl>, epoch: u64,
    record: &DownloadRecord, index: &Arc<ImageIndex>, batch: &Option<Arc<dyn PreparedPageBatch>>,
    page_index: usize, waiting: WaitingSlot,
) {
    let (context, control, record, index, batch) =
        (context.clone(), control.clone(), record.clone(), index.clone(), batch.clone());
    jobs.spawn(async move {
        acquire_page(context, control, epoch, record, index, page_index, batch, waiting).await
    });
}

async fn acquire_page(
    context: Arc<ContextState>, control: Arc<TaskControl>, epoch: u64,
    record: DownloadRecord, index: Arc<ImageIndex>, page_index: usize,
    batch: Option<Arc<dyn PreparedPageBatch>>, waiting: WaitingSlot,
) -> std::result::Result<(), AcquisitionFailure> {
    let slot = wait_for_slot(waiting, control.clone(), epoch, record.generation).await?;
    current(&context, &control, epoch, record.generation)?;
    let mut signal = control.changed.subscribe();
    let mut suspension = context.pause.changed.subscribe();
    current(&context, &control, epoch, record.generation)?;
    let image = tokio::select! {
        biased;
        _ = suspension.changed() => return Err(cancelled()),
        _ = signal.changed() => return Err(cancelled()),
        result = async {
            match batch.as_ref() {
                Some(prepared) => prepared.download_page(page_index).await,
                None => context.source.download_page(&index, page_index).await,
            }
        } => result?,
    };
    current(&context, &control, epoch, record.generation)?;
    let commit_context = context.clone();
    let commit_control = control.clone();
    let commit_record = record.clone();
    let generation = record.generation;
    let completed = run_download_work(context.clone(), control.clone(), epoch, generation, move |work| {
        // Keep capacity charged even if cancellation drops the async join handle during validation.
        let _slot = slot;
        let check = || check_work(&commit_context, &commit_control, epoch, generation, work);
        check().map_err(storage_failure)?;
        current(&commit_context, &commit_control, epoch, commit_record.generation)?;
        let prepared = verify_packet(image, Some(&check))?;
        let page = &index.images[page_index];
        if (page.width != 0 && page.width != prepared.width) || (page.height != 0 && page.height != prepared.height) {
            return Err(AcquisitionFailure::new("page_geometry_changed", false));
        }
        let receipt = PageReceipt {
            episode_id: commit_record.episode_id.clone(), revision: commit_record.revision.clone(), page_id: page.id.clone(),
            page_index, file_name: format!("{}.{}", prepared.sha256, prepared.extension), sha256: prepared.sha256,
            bytes: prepared.bytes.len() as u64, width: prepared.width, height: prepared.height,
            retained: commit_record.retained, verified_at: now(),
            decode_validation_version: VALIDATION_VERSION,
        };
        let _publication = commit_context.publication.lock()
            .map_err(|_| AcquisitionFailure::new("page_publication_failed", false))?;
        current(&commit_context, &commit_control, epoch, commit_record.generation)?;
        let mut store = Store::open(&commit_context.database).map_err(storage_failure)?;
        if let Some(existing) = store.page_receipt(&commit_context.account, &receipt.episode_id,
            &receipt.revision, &receipt.page_id).map_err(storage_failure)?
        {
            if let Some(mut existing) = revalidate_receipt_locked_with_check(&store, &commit_context.account,
                &commit_context.pages, &existing, Some(&check))
                .map_err(storage_failure)? {
                if existing.sha256 != receipt.sha256 || existing.bytes != receipt.bytes {
                    return Err(AcquisitionFailure::new("page_content_changed", false));
                }
                if commit_record.retained && !existing.retained {
                    store.retain_page(&commit_context.account, &receipt.episode_id,
                        &receipt.revision, &receipt.page_id).map_err(storage_failure)?;
                    existing.retained = true;
                }
                let updated = store.load_download(&commit_context.account, &commit_record.id)
                    .map_err(storage_failure)?;
                return Ok((existing, updated));
            }
        }
        store.begin_verified_page_commit_generation(&commit_context.account, &commit_record.id,
            commit_record.generation, &receipt, &prepared.proof).map_err(storage_failure)?;
        write_object_with_check(&commit_context.pages, &receipt.file_name, &prepared.bytes, Some(&check)).map_err(storage_failure)?;
        check().map_err(storage_failure)?;
        current(&commit_context, &commit_control, epoch, commit_record.generation)?;
        store.commit_verified_page_generation(&commit_context.account, &commit_record.id,
            commit_record.generation, &receipt, &prepared.proof).map_err(storage_failure)?;
        let updated = store.load_download(&commit_context.account, &commit_record.id).map_err(storage_failure)?;
        Ok::<_, AcquisitionFailure>((receipt, updated))
    }).await?;
    current(&context, &control, epoch, record.generation)?;
    publish(&context, &control, epoch, &record, DownloadEventKind::PageReady {
        episode_id: record.episode_id.clone(), revision: record.revision.clone(),
        page_id: completed.0.page_id.clone(), page_index,
        path: context.pages.join(&completed.0.file_name), retained: completed.0.retained,
        completed_pages: completed.1.as_ref().map_or(0, |task| task.completed_pages),
        total_pages: record.total_pages,
    });
    Ok(())
}

struct Slot(Arc<ContextState>);

impl Drop for Slot {
    fn drop(&mut self) {
        if let Ok(mut capacity) = self.0.capacity.lock() { capacity.active = capacity.active.saturating_sub(1); }
        self.0.capacity_changed.notify_waiters();
    }
}

struct WaitingSlot {
    context: Arc<ContextState>,
    ticket: u64,
}

impl Drop for WaitingSlot {
    fn drop(&mut self) {
        let removed = if let Ok(mut capacity) = self.context.capacity.lock() {
            let previous = capacity.waiters.len();
            capacity.waiters.retain(|waiter| waiter.ticket != self.ticket);
            capacity.waiters.len() != previous
        } else { false };
        if removed { self.context.capacity_changed.notify_waiters(); }
    }
}

async fn acquire_slot(
    context: Arc<ContextState>, control: Arc<TaskControl>, epoch: u64, generation: u64,
    kind: SlotKind,
) -> std::result::Result<Slot, AcquisitionFailure> {
    let waiting = queue_slot(context, &control, epoch, generation, kind)?;
    wait_for_slot(waiting, control, epoch, generation).await
}

fn queue_slot(
    context: Arc<ContextState>, control: &Arc<TaskControl>, epoch: u64, generation: u64,
    kind: SlotKind,
) -> std::result::Result<WaitingSlot, AcquisitionFailure> {
    current(&context, &control, epoch, generation)?;
    let ticket = {
        let mut capacity = context.capacity.lock()
            .map_err(|_| AcquisitionFailure::new("download_capacity_failed", false))?;
        capacity.next_ticket = capacity.next_ticket.checked_add(1)
            .ok_or_else(|| AcquisitionFailure::new("download_wait_queue_exhausted", false))?;
        let ticket = capacity.next_ticket;
        capacity.waiters.push(SlotWaiter { ticket, control: Arc::downgrade(control), epoch, generation, kind });
        ticket
    };
    context.capacity_changed.notify_waiters();
    Ok(WaitingSlot { context, ticket })
}

async fn wait_for_slot(
    waiting: WaitingSlot, control: Arc<TaskControl>, epoch: u64, generation: u64,
) -> std::result::Result<Slot, AcquisitionFailure> {
    let context = waiting.context.clone();
    let ticket = waiting.ticket;
    let _waiting = waiting;
    let mut signal = control.changed.subscribe();
    let mut suspension = context.pause.changed.subscribe();
    loop {
        current(&context, &control, epoch, generation)?;
        let wake = context.capacity_changed.notified();
        tokio::pin!(wake);
        wake.as_mut().enable();
        let granted = {
            let mut capacity = context.capacity.lock()
                .map_err(|_| AcquisitionFailure::new("download_capacity_failed", false))?;
            capacity.waiters.retain(|waiter| waiter.control.upgrade()
                .is_some_and(|control| is_current(&context, &control, waiter.epoch, waiter.generation)));
            if capacity.active < capacity.limit && capacity.next_waiter().is_some_and(|(winner, _)| winner == ticket) {
                capacity.waiters.retain(|waiter| waiter.ticket != ticket);
                capacity.active += 1;
                capacity.retained_streak = if control.retained {
                    capacity.retained_streak.saturating_add(1).min(MAX_RETAINED_BURST)
                } else { 0 };
                true
            } else { false }
        };
        if granted {
            context.capacity_changed.notify_waiters();
            return Ok(Slot(context.clone()));
        }
        tokio::select! {
            biased;
            _ = suspension.changed() => return Err(cancelled()),
            _ = signal.changed() => return Err(cancelled()),
            _ = &mut wake => {},
        }
    }
}

struct PreparedPage {
    bytes: Vec<u8>, width: u32, height: u32, sha256: String, extension: &'static str,
    proof: DecodedImageProof,
}

fn verify_packet(image: AcquiredPage, check: Option<&ValidationCheck<'_>>) -> std::result::Result<PreparedPage, AcquisitionFailure> {
    check_validation(check).map_err(storage_failure)?;
    let cancelled_before_allocation = || check.is_some_and(|check| check().is_err());
    let validated = validate_image_cancellable(&image.bytes, Some(MAX_SOURCE_PIXELS), &cancelled_before_allocation)
        .map_err(|error| AcquisitionFailure::new(error.code, false))?.ok_or_else(cancelled)?;
    check_validation(check).map_err(storage_failure)?;
    let (width, height) = (validated.width, validated.height);
    if (image.width != 0 && image.width != width) || (image.height != 0 && image.height != height) {
        return Err(AcquisitionFailure::new("page_geometry_mismatch", false));
    }
    let sha256 = validated.sha256.clone();
    if image.sha256.as_ref().is_some_and(|expected| !expected.eq_ignore_ascii_case(&sha256)) {
        return Err(AcquisitionFailure::new("page_digest_mismatch", false));
    }
    Ok(PreparedPage { bytes: image.bytes, width, height, sha256,
        extension: validated.extension, proof: validated })
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("{code}")]
pub(crate) struct ReceiptValidationUnavailable {
    pub(crate) kind: IntegrityFailureKind,
    pub(crate) code: &'static str,
}

pub(crate) fn is_validation_unavailable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<ReceiptValidationUnavailable>().is_some_and(|failure| {
        matches!(failure.kind, IntegrityFailureKind::Unsupported | IntegrityFailureKind::ResourceLimit)
    })
}

enum ReceiptFileInspection {
    Verified(Option<DecodedImageProof>),
    InvalidEncoding(InvalidEncodingEvidence),
    Unavailable(ReceiptValidationUnavailable),
    Rejected,
}

fn inspect_receipt_file(root: &Path, receipt: &PageReceipt, require_proof: bool) -> Result<ReceiptFileInspection> {
    inspect_receipt_file_with_check(root, receipt, require_proof, None)
}

fn inspect_receipt_file_with_check(
    root: &Path, receipt: &PageReceipt, require_proof: bool, check: Option<&ValidationCheck<'_>>,
) -> Result<ReceiptFileInspection> {
    check_validation(check)?;
    let path = receipt_path(root, &receipt.file_name)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(ReceiptFileInspection::Rejected),
        Err(error) => return Err(error).context("inspect page object"),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != receipt.bytes
        || receipt.bytes == 0 || receipt.width == 0 || receipt.height == 0 {
        return Ok(ReceiptFileInspection::Rejected);
    }
    if receipt.bytes > MAX_IMAGE_BYTES {
        return Ok(ReceiptFileInspection::Unavailable(ReceiptValidationUnavailable {
            kind: IntegrityFailureKind::ResourceLimit, code: "image_size_limit",
        }));
    }
    if u64::from(receipt.width) * u64::from(receipt.height) > MAX_SOURCE_PIXELS {
        return Ok(ReceiptFileInspection::Unavailable(ReceiptValidationUnavailable {
            kind: IntegrityFailureKind::ResourceLimit, code: "image_pixel_limit",
        }));
    }
    let mut file = File::open(&path).context("open page object")?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    check_validation(check)?;
    let mut encoded = (require_proof || receipt.decode_validation_version != VALIDATION_VERSION)
        .then(|| Vec::with_capacity(receipt.bytes as usize));
    let mut count = 0u64;
    loop {
        check_validation(check)?;
        let read = file.read(&mut buffer)?;
        if read == 0 { break; }
        count += read as u64;
        if count > MAX_IMAGE_BYTES { return Ok(ReceiptFileInspection::Rejected); }
        hasher.update(&buffer[..read]);
        if let Some(encoded) = encoded.as_mut() { encoded.extend_from_slice(&buffer[..read]); }
    }
    check_validation(check)?;
    if count != receipt.bytes || !hex_bytes(&hasher.finalize()).eq_ignore_ascii_case(&receipt.sha256) {
        return Ok(ReceiptFileInspection::Rejected);
    }
    let Some(encoded) = encoded else { return Ok(ReceiptFileInspection::Verified(None)); };
    let cancelled_before_allocation = || check.is_some_and(|check| check().is_err());
    let validated = validate_image_cancellable(&encoded, Some(MAX_SOURCE_PIXELS), &cancelled_before_allocation);
    check_validation(check)?;
    match validated {
        Ok(Some(proof)) if proof.width == receipt.width && proof.height == receipt.height => {
            Ok(ReceiptFileInspection::Verified(Some(proof)))
        },
        Ok(None) => Err(cancelled().into()),
        Ok(_) => Ok(ReceiptFileInspection::Rejected),
        Err(error) => Ok(match error.kind {
            IntegrityFailureKind::Unsupported | IntegrityFailureKind::ResourceLimit => {
                ReceiptFileInspection::Unavailable(ReceiptValidationUnavailable { kind: error.kind, code: error.code })
            },
            IntegrityFailureKind::Corrupt => error.encoding_evidence().cloned()
                .map_or(ReceiptFileInspection::Rejected, ReceiptFileInspection::InvalidEncoding),
        }),
    }
}

/// Verify exact bytes without changing their persisted validation state.
#[cfg(test)]
pub(crate) fn verify_receipt_file(root: &Path, receipt: &PageReceipt) -> Result<bool> {
    Ok(matches!(inspect_receipt_file(root, receipt, false)?, ReceiptFileInspection::Verified(_)))
}

/// Rehash every object; only legacy receipts need a complete decode and a proof upgrade.
pub(crate) fn revalidate_receipt(
    store: &Store, account: &str, root: &Path, receipt: &PageReceipt,
) -> Result<Option<PageReceipt>> {
    let publication = publication_lock();
    let _publication = publication.lock().map_err(|_| anyhow!("page publication lock failed"))?;
    revalidate_receipt_locked(store, account, root, receipt)
}

fn revalidate_receipt_locked(
    store: &Store, account: &str, root: &Path, receipt: &PageReceipt,
) -> Result<Option<PageReceipt>> {
    revalidate_receipt_locked_with_check(store, account, root, receipt, None)
}

fn revalidate_receipt_locked_with_check(
    store: &Store, account: &str, root: &Path, receipt: &PageReceipt, check: Option<&ValidationCheck<'_>>,
) -> Result<Option<PageReceipt>> {
    Ok(revalidate_receipt_checked(store, account, root, receipt, false, check)?
        .map(|(receipt, _)| receipt))
}

fn revalidate_receipt_with_proof(
    store: &Store, account: &str, root: &Path, receipt: &PageReceipt, require_proof: bool,
) -> Result<Option<(PageReceipt, Option<DecodedImageProof>)>> {
    revalidate_receipt_checked(store, account, root, receipt, require_proof, None)
}

fn revalidate_receipt_checked(
    store: &Store, account: &str, root: &Path, receipt: &PageReceipt, require_proof: bool,
    check: Option<&ValidationCheck<'_>>,
) -> Result<Option<(PageReceipt, Option<DecodedImageProof>)>> {
    match inspect_receipt_file_with_check(root, receipt, require_proof, check)? {
        ReceiptFileInspection::Verified(Some(proof)) => {
            if store.page_receipt(account, &receipt.episode_id, &receipt.revision, &receipt.page_id)?.is_none() {
                if !require_proof { return Ok(None); }
                let mut upgraded = receipt.clone();
                upgraded.decode_validation_version = proof.validation_version();
                return Ok(Some((upgraded, Some(proof))));
            }
            if let Some(upgraded) = store.promote_page_validation(account, receipt, &proof, now())? {
                return Ok(Some((upgraded, Some(proof))));
            }
            let current = store.page_receipt(account, &receipt.episode_id, &receipt.revision, &receipt.page_id)?;
            if let Some(current) = current.filter(|current| same_receipt_identity(current, receipt)
                && current.decode_validation_version == VALIDATION_VERSION) {
                if matches!(inspect_receipt_file_with_check(root, &current, false, check)?, ReceiptFileInspection::Verified(_)) {
                    return Ok(Some((current, Some(proof))));
                }
            }
            Err(anyhow!("page receipt changed during validation"))
        },
        ReceiptFileInspection::Verified(None) => {
            let current = store.page_receipt(account, &receipt.episode_id, &receipt.revision, &receipt.page_id)?;
            match current {
                Some(current) if same_receipt_identity(&current, receipt)
                    && current.decode_validation_version == VALIDATION_VERSION => Ok(Some((current, None))),
                None => Ok(None),
                Some(_) => Err(anyhow!("page receipt changed during validation")),
            }
        },
        ReceiptFileInspection::InvalidEncoding(evidence) => {
            if receipt.decode_validation_version == 0 {
                let rejected = store.reject_legacy_invalid_page(account, receipt, &evidence)?;
                if !rejected && store.page_receipt(account, &receipt.episode_id, &receipt.revision, &receipt.page_id)?.is_some() {
                    return Err(anyhow!("page receipt changed during validation"));
                }
            }
            Ok(None)
        },
        ReceiptFileInspection::Unavailable(failure) => {
            if mark_unverifiable_records(store, account, receipt, &failure, require_proof)? {
                Err(failure.into())
            } else {
                Ok(None)
            }
        },
        ReceiptFileInspection::Rejected => Ok(None),
    }
}

fn mark_unverifiable_records(
    store: &Store, account: &str, receipt: &PageReceipt, failure: &ReceiptValidationUnavailable,
    include_pending: bool,
) -> Result<bool> {
    let current = store.page_receipt(account, &receipt.episode_id, &receipt.revision, &receipt.page_id)?;
    let committed = current.as_ref().is_some_and(|current| same_receipt_identity(current, receipt));
    let pending = if include_pending {
        store.pending_page_commits(account)?.into_iter()
            .filter(|pending| same_receipt_identity(&pending.receipt, receipt)).collect::<Vec<_>>()
    } else { Vec::new() };
    if !committed && pending.is_empty() { return Ok(false); }
    for mut record in store.downloads(account)? {
        if record.episode_id != receipt.episode_id || record.revision != receipt.revision || record.refresh_source
            || !matches!(record.status, DownloadStatus::Completed | DownloadStatus::Queued | DownloadStatus::Running)
            || !committed && !pending.iter().any(|pending| pending.task_id == record.id && pending.generation == record.generation) {
            continue;
        }
        record.status = DownloadStatus::Failed;
        record.error_code = Some(failure.code.to_owned());
        record.terminal_event = terminal_payload(&DownloadEventKind::Failed { code: failure.code.to_owned(), retryable: false });
        record.updated_at = now();
        store.save_download(account, &record)?;
    }
    Ok(true)
}

fn same_receipt_identity(left: &PageReceipt, right: &PageReceipt) -> bool {
    left.episode_id == right.episode_id && left.revision == right.revision && left.page_id == right.page_id
        && left.page_index == right.page_index && left.file_name == right.file_name
        && left.sha256.eq_ignore_ascii_case(&right.sha256) && left.bytes == right.bytes
        && left.width == right.width && left.height == right.height
}

fn verify_record(context: &ContextState, record: &DownloadRecord) -> Result<bool> {
    verify_record_with_check(context, record, None)
}

fn verify_record_with_check(
    context: &ContextState, record: &DownloadRecord, check: Option<&ValidationCheck<'_>>,
) -> Result<bool> {
    check_validation(check)?;
    if record.revision == PENDING_REVISION { return Ok(false); }
    let store = Store::open(&context.database)?;
    let Some(index) = store.load_index(&context.account, &record.episode_id, Some(&record.revision))? else { return Ok(false); };
    if index.images.is_empty() { return Ok(false); }
    for descriptor in &index.images {
        check_validation(check)?;
        let Some(receipt) = store.page_receipt(&context.account, &record.episode_id, &record.revision, &descriptor.id)? else { return Ok(false); };
        if receipt.page_index != descriptor.index || receipt.width != descriptor.width || receipt.height != descriptor.height
            || revalidate_receipt_locked_with_check(&store, &context.account, &context.pages, &receipt, check)?.is_none() { return Ok(false); }
    }
    Ok(true)
}

#[cfg(test)]
fn write_object(root: &Path, filename: &str, bytes: &[u8]) -> Result<()> {
    write_object_with_check(root, filename, bytes, None)
}

fn write_object_with_check(root: &Path, filename: &str, bytes: &[u8], check: Option<&ValidationCheck<'_>>) -> Result<()> {
    check_validation(check)?;
    let destination = receipt_path(root, filename)?;
    if destination.exists() {
        let existing = fs::symlink_metadata(&destination)?;
        ensure!(existing.is_file() && !existing.file_type().is_symlink(), "page destination is not a regular file");
        if existing.len() == bytes.len() as u64 {
            let expected = filename.split('.').next().unwrap_or_default();
            let mut file = File::open(&destination)?;
            let mut hasher = Sha256::new();
            let mut buffer = [0u8; 64 * 1024];
            loop {
                check_validation(check)?;
                let read = file.read(&mut buffer)?; if read == 0 { break; } hasher.update(&buffer[..read]);
            }
            if hex_bytes(&hasher.finalize()) == expected {
                file.sync_all().context("sync existing page object")?;
                #[cfg(unix)] { File::open(root)?.sync_all().context("sync page directory")?; }
                return Ok(());
            }
        }
        fs::remove_file(&destination).context("remove corrupt page object")?;
    }
    let temporary = root.join(format!(".page-{}-{}-{}.tmp", std::process::id(),
        NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed), now()));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let mut file = options.open(&temporary).context("create temporary page object")?;
        for chunk in bytes.chunks(64 * 1024) {
            check_validation(check)?;
            file.write_all(chunk).context("write temporary page object")?;
        }
        file.sync_all().context("sync temporary page object")?;
        drop(file);
        check_validation(check)?;
        match fs::rename(&temporary, &destination) {
            Ok(()) => {},
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                ensure!(fs::metadata(&destination)?.len() == bytes.len() as u64, "concurrently committed page object differs");
                let mut existing = Vec::new();
                File::open(&destination)?.take(MAX_IMAGE_BYTES + 1).read_to_end(&mut existing)
                    .context("read concurrently committed object")?;
                ensure!(existing == bytes, "concurrently committed page object differs");
                fs::remove_file(&temporary)?;
            },
            Err(error) => return Err(error).context("publish page object"),
        }
        #[cfg(unix)] { File::open(root)?.sync_all().context("sync page directory")?; }
        Ok(())
    })();
    if result.is_err() { let _ = fs::remove_file(&temporary); }
    result
}

fn receipt_path(root: &Path, filename: &str) -> Result<PathBuf> {
    let mut parts = filename.split('.');
    let digest = parts.next().unwrap_or_default();
    let extension = parts.next().unwrap_or_default();
    ensure!(digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        && matches!(extension, "jpg" | "png" | "gif" | "webp") && parts.next().is_none(),
        "page object filename is invalid");
    Ok(root.join(filename))
}

fn cleanup_temporary_objects(root: &Path) -> Result<()> {
    let mut removed = false;
    for entry in fs::read_dir(root).context("read account page directory")? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue; };
        let Some(identity) = name.strip_prefix(".page-").and_then(|name| name.strip_suffix(".tmp")) else { continue; };
        let parts = identity.split('-').collect::<Vec<_>>();
        if parts.len() != 3 || parts.iter().any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit())) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_file() && !metadata.file_type().is_symlink() {
            fs::remove_file(entry.path()).context("remove interrupted temporary page object")?;
            removed = true;
        }
    }
    #[cfg(unix)]
    if removed { File::open(root)?.sync_all().context("sync recovered page directory")?; }
    #[cfg(not(unix))]
    let _ = removed;
    Ok(())
}

fn cleanup_unreferenced_objects(root: &Path, store: &Store, account: &str) -> Result<u64> {
    let mut removed = 0u64;
    let mut changed = false;
    for entry in fs::read_dir(root).context("read account page objects")? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue; };
        if receipt_path(root, name).is_err() { continue; }
        let metadata = fs::symlink_metadata(entry.path()).context("inspect account page object")?;
        if !metadata.is_file() || metadata.file_type().is_symlink() { continue; }
        if !store.page_object_referenced(account, name)? {
            fs::remove_file(entry.path()).context("remove unreferenced account page object")?;
            removed = removed.saturating_add(metadata.len());
            changed = true;
        }
    }
    #[cfg(unix)]
    if changed { File::open(root)?.sync_all().context("sync account page cleanup")?; }
    #[cfg(not(unix))]
    let _ = changed;
    Ok(removed)
}

fn validate_request(request: &DownloadRequest) -> Result<()> {
    ensure!(!request.comic_id.is_empty() && !request.episode_id.is_empty(), "download source identity is empty");
    ensure!(request.comic_id.len() <= 256 && request.episode_id.len() <= 256, "download source identity is too long");
    ensure!(request.retain || request.reader_generation.is_some(), "prefetch requires a reader owner");
    ensure!(request.expected_revision.as_ref().is_none_or(|value| !value.is_empty() && value.len() <= 256),
        "download revision is invalid");
    ensure!(request.page_indices.as_ref().is_none_or(|pages| !pages.is_empty() && pages.len() <= MAX_PAGES),
        "download page selection is invalid");
    Ok(())
}

fn validate_index(index: &ImageIndex, episode_id: &str) -> std::result::Result<(), AcquisitionFailure> {
    if index.episode_id != episode_id || index.revision.is_empty() || index.revision == PENDING_REVISION
        || index.images.is_empty() || index.images.len() > MAX_PAGES || index.paths.len() != index.images.len() {
        return Err(AcquisitionFailure::new("image_index_invalid", false));
    }
    let mut identities = HashSet::new();
    for (position, page) in index.images.iter().enumerate() {
        if page.index != position || page.id.is_empty() || !identities.insert(page.id.as_str()) {
            return Err(AcquisitionFailure::new("image_index_identity_invalid", false));
        }
    }
    Ok(())
}

fn request_id(account: &str, request: &DownloadRequest) -> String {
    let owner = if request.retain { "retained".to_owned() } else {
        format!("reader:{}", request.reader_generation.unwrap_or_default())
    };
    digest_hex(format!("download-v1\0{account}\0{}\0{}\0{}\0{owner}", request.comic_id,
        request.episode_id, request.expected_revision.as_deref().unwrap_or(PENDING_REVISION)).as_bytes())
}

fn is_current(context: &ContextState, control: &TaskControl, epoch: u64, generation: u64) -> bool {
    context.active.load(Ordering::Acquire) && control.reader_is_active()
        && context.epoch.load(Ordering::Acquire) == epoch && control.generation.load(Ordering::Acquire) == generation
        && pause_is_current(context, control.pause_revision)
}

fn pause_is_current(context: &ContextState, revision: u64) -> bool {
    !context.pause.suspended.load(Ordering::Acquire)
        && context.pause.revision.load(Ordering::Acquire) == revision
}

fn current(context: &ContextState, control: &TaskControl, epoch: u64, generation: u64)
    -> std::result::Result<(), AcquisitionFailure>
{
    if is_current(context, control, epoch, generation) { Ok(()) } else { Err(cancelled()) }
}

fn publish(context: &ContextState, control: &Arc<TaskControl>, epoch: u64, record: &DownloadRecord, kind: DownloadEventKind) {
    if is_current(context, control, epoch, record.generation) {
        if terminal_payload(&kind).is_some() { persist_terminal(context, record, &kind); }
        else {
            context.events.progress(context, control, DownloadEvent {
                account_epoch: epoch, task_id: record.id.clone(), generation: record.generation, kind,
            });
        }
    }
}

fn terminal_payload(kind: &DownloadEventKind) -> Option<serde_json::Value> {
    Some(match kind {
        DownloadEventKind::Completed => serde_json::json!({"kind":"completed"}),
        DownloadEventKind::PrefetchReady => serde_json::json!({"kind":"prefetch_ready"}),
        DownloadEventKind::Paused => serde_json::json!({"kind":"paused"}),
        DownloadEventKind::Cancelled => serde_json::json!({"kind":"cancelled"}),
        DownloadEventKind::Failed { code, retryable } => {
            let code = if !code.is_empty() && code.len() <= 256
                && code.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')) {
                code.as_str()
            } else { "download_acquisition_failed" };
            serde_json::json!({"kind":"failed","code":code,"retryable":retryable})
        },
        _ => return None,
    })
}

fn terminal_kind(payload: &serde_json::Value) -> Option<DownloadEventKind> {
    Some(match payload.get("kind")?.as_str()? {
        "completed" => DownloadEventKind::Completed,
        "prefetch_ready" => DownloadEventKind::PrefetchReady,
        "paused" => DownloadEventKind::Paused,
        "cancelled" => DownloadEventKind::Cancelled,
        "failed" => DownloadEventKind::Failed { code: payload.get("code")?.as_str()?.to_owned(),
            retryable: payload.get("retryable")?.as_bool()? },
        _ => return None,
    })
}

fn persist_terminal(context: &ContextState, record: &DownloadRecord, kind: &DownloadEventKind) {
    if !context.active.load(Ordering::Acquire) { return; }
    let Some(payload) = terminal_payload(kind) else { return; };
    let recorded = Store::open(&context.database).and_then(|store| {
        store.record_download_event(&context.account, &record.id, record.generation, &payload)
    });
    if recorded.is_err() {
        context.events.retry_terminal(TerminalRetry { task_id: record.id.clone(), generation: record.generation, payload });
    } else {
        context.events.terminal_available();
    }
}

fn cancelled() -> AcquisitionFailure { AcquisitionFailure::new("download_cancelled", false) }
fn storage_failure(error: anyhow::Error) -> AcquisitionFailure {
    if let Some(failure) = error.downcast_ref::<AcquisitionFailure>() { return failure.clone(); }
    match error.downcast_ref::<ReceiptValidationUnavailable>() {
        Some(failure) => AcquisitionFailure::new(failure.code, false),
        None => AcquisitionFailure::new("download_storage_failed", true),
    }
}
fn now() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs().min(i64::MAX as u64) as i64 }
fn digest_hex(bytes: &[u8]) -> String { hex_bytes(&Sha256::digest(bytes)) }
fn publication_lock() -> Arc<Mutex<()>> {
    static PUBLICATION: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
    PUBLICATION.get_or_init(|| Arc::new(Mutex::new(()))).clone()
}
fn hex_bytes(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes { output.push(DIGITS[(byte >> 4) as usize] as char); output.push(DIGITS[(byte & 15) as usize] as char); }
    output
}

#[cfg(test)]
#[path = "downloads/batch_scheduler_tests.rs"]
mod batch_scheduler_tests;

#[cfg(test)]
#[path = "downloads/completeness_tests.rs"]
mod completeness_tests;

#[cfg(test)]
#[path = "downloads/preservation_tests.rs"]
mod preservation_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::PageDescriptor;
    use std::{sync::atomic::AtomicUsize, time::Duration};
    use tokio::sync::Semaphore;

    struct FixtureSource {
        index: Mutex<ImageIndex>,
        bytes: Vec<u8>,
        requested: AtomicUsize,
        gate: Arc<Semaphore>,
    }

    impl FixtureSource {
        fn new(available_pages: usize) -> Arc<Self> {
            Arc::new(Self {
                index: Mutex::new(ImageIndex {
                    episode_id: "456".to_owned(), revision: "revision-one".to_owned(),
                    images: (0..2).map(|index| PageDescriptor {
                        id: format!("page-{index}"), index, width: 8, height: 12,
                    }).collect(),
                    paths: vec!["fixture-page-one".to_owned(), "fixture-page-two".to_owned()],
                    ..Default::default()
                }),
                bytes: fixture_png(34), requested: AtomicUsize::new(0),
                gate: Arc::new(Semaphore::new(available_pages)),
            })
        }
    }

    fn fixture_png(tone: u8) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(8, 12, image::Rgba([tone, tone, tone, 255]));
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image).write_to(&mut output, ImageFormat::Png).unwrap();
        output.into_inner()
    }

    struct OrderedFixtureSource {
        requests: Mutex<Vec<(String, usize)>>,
        cancelled: Arc<AtomicUsize>,
        gate: Arc<Semaphore>,
        bytes: Vec<u8>,
    }

    struct OrderedRequest {
        cancelled: Arc<AtomicUsize>,
        completed: bool,
    }

    impl Drop for OrderedRequest {
        fn drop(&mut self) {
            if !self.completed { self.cancelled.fetch_add(1, Ordering::SeqCst); }
        }
    }

    impl AcquisitionSource for OrderedFixtureSource {
        fn image_index<'a>(&'a self, episode_id: &'a str) -> AcquisitionFuture<'a, ImageIndex> {
            Box::pin(async move {
                Ok(ImageIndex {
                    episode_id: episode_id.to_owned(), revision: format!("{episode_id}-revision"),
                    images: (0..4).map(|index| PageDescriptor { id: format!("{episode_id}-page-{index}"),
                        index, width: 8, height: 12 }).collect(),
                    paths: (0..4).map(|index| format!("fixture-{episode_id}-{index}")).collect(),
                    ..Default::default()
                })
            })
        }

        fn download_page<'a>(&'a self, index: &'a ImageIndex, page_index: usize)
            -> AcquisitionFuture<'a, AcquiredPage>
        {
            Box::pin(async move {
                self.requests.lock().unwrap().push((index.episode_id.clone(), page_index));
                let mut activity = OrderedRequest { cancelled: self.cancelled.clone(), completed: false };
                let permit = self.gate.clone().acquire_owned().await
                    .map_err(|_| AcquisitionFailure::new("fixture_gate_closed", false))?;
                permit.forget();
                activity.completed = true;
                Ok(AcquiredPage { bytes: self.bytes.clone(), width: 8, height: 12, sha256: None })
            })
        }
    }

    fn queued_for(manager: &DownloadManager, task_id: &str) -> usize {
        let Some(control) = manager.control(task_id) else { return 0; };
        manager.context.capacity.lock().unwrap().waiters.iter().filter(|waiter| {
            waiter.control.upgrade().is_some_and(|candidate| Arc::ptr_eq(&candidate, &control))
        }).count()
    }

    impl AcquisitionSource for FixtureSource {
        fn image_index<'a>(&'a self, _episode_id: &'a str) -> AcquisitionFuture<'a, ImageIndex> {
            Box::pin(async move { Ok(self.index.lock().unwrap().clone()) })
        }

        fn download_page<'a>(&'a self, _index: &'a ImageIndex, _page_index: usize)
            -> AcquisitionFuture<'a, AcquiredPage>
        {
            Box::pin(async move {
                self.requested.fetch_add(1, Ordering::SeqCst);
                let permit = self.gate.clone().acquire_owned().await
                    .map_err(|_| AcquisitionFailure::new("fixture_gate_closed", false))?;
                permit.forget();
                Ok(AcquiredPage { bytes: self.bytes.clone(), width: 8, height: 12, sha256: None })
            })
        }
    }

    fn request(retain: bool) -> DownloadRequest {
        DownloadRequest {
            comic_id: "123".to_owned(), episode_id: "456".to_owned(),
            title: "Fixture episode".to_owned(), comic_title: "Fixture comic".to_owned(),
            retain, ..Default::default()
        }
    }

    async fn wait_until(mut predicate: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if predicate() { return; }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }).await.expect("the controlled download did not reach its expected state");
    }

    fn record(root: &Path, id: &str) -> DownloadRecord {
        Store::open(root.join("state.sqlite3")).unwrap().load_download("fixture-account", id).unwrap().unwrap()
    }

    #[tokio::test]
    async fn interruption_resume_keeps_verified_pages_and_private_descriptors() {
        let directory = tempfile::tempdir().unwrap();
        let source = FixtureSource::new(1);
        let mut manager = DownloadManager::new(directory.path(), "fixture-account".to_owned(), 1,
            source.clone(), Handle::current()).unwrap();
        let task = manager.enqueue(request(true)).unwrap();
        wait_until(|| record(directory.path(), &task.id).completed_pages == 1).await;
        manager.pause(&task.id).unwrap();
        let requested_before_resume = source.requested.load(Ordering::SeqCst);
        drop(manager);
        source.gate.add_permits(10);
        let mut restored = DownloadManager::new(directory.path(), "fixture-account".to_owned(), 2,
            source.clone(), Handle::current()).unwrap();
        restored.recover().unwrap();
        assert_eq!(record(directory.path(), &task.id).status, DownloadStatus::Paused);
        restored.resume(&task.id).unwrap();
        wait_until(|| record(directory.path(), &task.id).status == DownloadStatus::Completed).await;
        assert_eq!(source.requested.load(Ordering::SeqCst), requested_before_resume + 1);
        let completed = record(directory.path(), &task.id);
        assert_eq!(completed.completed_pages, 2);
        assert_eq!(completed.bytes, 2 * source.bytes.len() as u64);
        let stored_index = Store::open(directory.path().join("state.sqlite3")).unwrap()
            .load_index("fixture-account", "456", Some("revision-one")).unwrap().unwrap();
        assert!(stored_index.paths.is_empty());
        let resolver = restored.page_resolver().clone();
        let resolved = resolver.local_page("456", "revision-one", "page-0").unwrap();
        assert!(resolved.is_some());
        assert_eq!(resolved, restored.local_page("456", "revision-one", "page-0").unwrap());
    }

    #[tokio::test]
    async fn changed_source_revision_does_not_replace_an_existing_checkpoint() {
        let directory = tempfile::tempdir().unwrap();
        let source = FixtureSource::new(1);
        let mut manager = DownloadManager::new(directory.path(), "fixture-account".to_owned(), 1,
            source.clone(), Handle::current()).unwrap();
        let task = manager.enqueue(request(true)).unwrap();
        wait_until(|| record(directory.path(), &task.id).completed_pages == 1).await;
        manager.pause(&task.id).unwrap();
        source.index.lock().unwrap().revision = "revision-two".to_owned();
        manager.retry(&task.id).unwrap();
        wait_until(|| record(directory.path(), &task.id).status == DownloadStatus::Failed).await;
        let failed = record(directory.path(), &task.id);
        assert_eq!(failed.error_code.as_deref(), Some("index_revision_changed"));
        assert_eq!(failed.revision, "revision-one");
        let store = Store::open(directory.path().join("state.sqlite3")).unwrap();
        assert_eq!(store.page_receipts("fixture-account", "456", "revision-one").unwrap().len(), 1);
        assert!(store.load_index("fixture-account", "456", Some("revision-two")).unwrap().is_none());
    }

    #[tokio::test]
    async fn closing_reader_cancels_only_its_prefetch_owner() {
        let directory = tempfile::tempdir().unwrap();
        let source = FixtureSource::new(0);
        let mut manager = DownloadManager::new(directory.path(), "fixture-account".to_owned(), 1,
            source.clone(), Handle::current()).unwrap();
        let mut prefetch = request(false);
        prefetch.reader_generation = Some(77);
        prefetch.expected_revision = Some("revision-one".to_owned());
        prefetch.page_indices = Some(vec![0]);
        let reader_task = manager.enqueue(prefetch).unwrap();
        let retained_task = manager.enqueue(request(true)).unwrap();
        wait_until(|| source.requested.load(Ordering::SeqCst) >= 2).await;
        manager.cancel_reader(77).unwrap();
        source.gate.add_permits(10);
        wait_until(|| record(directory.path(), &retained_task.id).status == DownloadStatus::Completed).await;
        assert!(Store::open(manager.database_path()).unwrap().load_download("fixture-account", &reader_task.id).unwrap().is_none());
        let store = Store::open(directory.path().join("state.sqlite3")).unwrap();
        let receipts = store.page_receipts("fixture-account", "456", "revision-one").unwrap();
        assert_eq!(receipts.len(), 2);
        assert!(receipts.iter().all(|receipt| receipt.retained));
    }

    #[tokio::test]
    async fn manager_return_synchronizes_reader_lease_without_reviving_closed_prefetch() {
        let directory = tempfile::tempdir().unwrap();
        let source = FixtureSource::new(0);
        let mut manager = DownloadManager::new(directory.path(), "fixture-account".to_owned(), 1,
            source.clone(), Handle::current()).unwrap();
        let mut prefetch = request(false);
        prefetch.reader_generation = Some(77);
        prefetch.expected_revision = Some("revision-one".to_owned());
        prefetch.page_indices = Some(vec![0]);
        let closed_request = prefetch.clone();
        let previous = manager.enqueue(prefetch).unwrap();
        let retained = manager.enqueue(request(true)).unwrap();
        wait_until(|| source.requested.load(Ordering::SeqCst) >= 2).await;
        manager.synchronize_reader_owner(Some((88, "456".to_owned(), "revision-one".to_owned()))).unwrap();
        assert!(Store::open(manager.database_path()).unwrap().load_download("fixture-account", &previous.id).unwrap().is_none());
        assert!(manager.retry(&previous.id).is_err());
        assert!(manager.enqueue(closed_request).is_err());
        assert!(matches!(record(directory.path(), &retained.id).status, DownloadStatus::Queued | DownloadStatus::Running));
        assert!(manager.remove_episode("456", "revision-one").is_err());
        let mut current_request = request(false);
        current_request.reader_generation = Some(88);
        current_request.expected_revision = Some("revision-one".to_owned());
        current_request.page_indices = Some(vec![0]);
        let current_task = manager.enqueue(current_request).unwrap();
        source.gate.add_permits(10);
        wait_until(|| record(directory.path(), &retained.id).status == DownloadStatus::Completed).await;
        manager.synchronize_reader_owner(None).unwrap();
        assert!(manager.context.reader_pins.lock().unwrap().is_empty());
        assert!(manager.resume(&current_task.id).is_err());
        assert_eq!(record(directory.path(), &retained.id).status, DownloadStatus::Completed);
        let unknown = DownloadRecord {
            id: "unknown-prefetch".to_owned(), comic_id: "123".to_owned(), episode_id: "456".to_owned(),
            revision: "revision-one".to_owned(), status: DownloadStatus::Paused, total_pages: 2,
            generation: 1, created_at: now(), updated_at: now(), ..Default::default()
        };
        Store::open(manager.database_path()).unwrap().save_download("fixture-account", &unknown).unwrap();
        assert!(manager.resume(&unknown.id).is_err());
        manager.synchronize_reader_owner(None).unwrap();
        assert!(Store::open(manager.database_path()).unwrap().load_download("fixture-account", &unknown.id).unwrap().is_none());
    }

    #[tokio::test]
    async fn transport_pause_handle_stays_latched_while_manager_is_borrowed() {
        let directory = tempfile::tempdir().unwrap();
        let source = FixtureSource::new(0);
        let mut manager = DownloadManager::new(directory.path(), "fixture-account".to_owned(), 1,
            source.clone(), Handle::current()).unwrap();
        let mut prefetch = request(false);
        prefetch.reader_generation = Some(77);
        prefetch.expected_revision = Some("revision-one".to_owned());
        prefetch.page_indices = Some(vec![0]);
        let reader_task = manager.enqueue(prefetch).unwrap();
        let retained_task = manager.enqueue(request(true)).unwrap();
        wait_until(|| source.requested.load(Ordering::SeqCst) >= 2).await;
        let handle = manager.pause_handle();
        let context = manager.context.clone();
        let retained_id = retained_task.id.clone();
        let (inspect_sender, inspect_receiver) = mpsc::channel();
        let (observed_sender, observed_receiver) = tokio::sync::oneshot::channel();
        let (return_sender, return_receiver) = mpsc::channel();
        let borrowed = tokio::task::spawn_blocking(move || {
            inspect_receiver.recv().unwrap();
            let recovered = manager.recover().is_ok();
            let enqueue_blocked = manager.enqueue(request(true)).is_err();
            let restart_blocked = manager.resume(&retained_id).is_err();
            observed_sender.send((recovered, enqueue_blocked, restart_blocked)).unwrap();
            return_receiver.recv().unwrap();
            manager
        });
        let requests_before_suspend = source.requested.load(Ordering::SeqCst);
        handle.suspend();
        source.gate.add_permits(10);
        wait_until(|| context.capacity.lock().unwrap().active == 0).await;
        inspect_sender.send(()).unwrap();
        let (recovered, enqueue_blocked, restart_blocked) = observed_receiver.await.unwrap();
        assert!(recovered && enqueue_blocked && restart_blocked);
        assert!(handle.is_suspended());
        assert_eq!(source.requested.load(Ordering::SeqCst), requests_before_suspend);
        assert_ne!(record(directory.path(), &reader_task.id).status, DownloadStatus::Failed);
        assert_ne!(record(directory.path(), &retained_task.id).status, DownloadStatus::Failed);
        assert!(context.reader_pins.lock().unwrap().contains_key(&77));

        // The application owns the durable pause snapshot even while the manager is borrowed.
        let store = Store::open(directory.path().join("state.sqlite3")).unwrap();
        let mut snapshot = store.load_download("fixture-account", &retained_task.id).unwrap().unwrap();
        snapshot.generation += 1;
        snapshot.status = DownloadStatus::Paused;
        store.save_download("fixture-account", &snapshot).unwrap();
        handle.resume();
        assert!(!handle.is_suspended());
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(source.requested.load(Ordering::SeqCst), requests_before_suspend);
        assert_eq!(record(directory.path(), &retained_task.id).status, DownloadStatus::Paused);
        assert!(context.reader_pins.lock().unwrap().contains_key(&77));
        return_sender.send(()).unwrap();
        let mut manager = borrowed.await.unwrap();
        assert!(!manager.reader_is_retired(77));
        manager.resume(&retained_task.id).unwrap();
        wait_until(|| record(directory.path(), &retained_task.id).status == DownloadStatus::Completed).await;
        assert_eq!(source.requested.load(Ordering::SeqCst), requests_before_suspend + 2);
        assert!(context.reader_pins.lock().unwrap().contains_key(&77));
    }

    #[tokio::test]
    async fn prioritized_retained_slots_preserve_inflight_fifo_reader_and_durable_state() {
        let directory = tempfile::tempdir().unwrap();
        let source = Arc::new(OrderedFixtureSource { requests: Mutex::new(Vec::new()),
            cancelled: Arc::new(AtomicUsize::new(0)), gate: Arc::new(Semaphore::new(0)), bytes: fixture_png(34) });
        let mut manager = DownloadManager::new(directory.path(), "fixture-account".to_owned(), 1,
            source.clone(), Handle::current()).unwrap();
        manager.set_concurrency(1).unwrap();
        let retained_request = |episode: &str| DownloadRequest { comic_id: format!("comic-{episode}"),
            episode_id: episode.to_owned(), title: episode.to_owned(), retain: true, ..Default::default() };
        let first = manager.enqueue(retained_request("episode-a")).unwrap();
        wait_until(|| source.requests.lock().unwrap().len() == 1 && queued_for(&manager, &first.id) == 3).await;
        let fifo = manager.enqueue(retained_request("episode-c")).unwrap();
        wait_until(|| queued_for(&manager, &fifo.id) == 1).await;
        let preferred = manager.enqueue(retained_request("episode-b")).unwrap();
        wait_until(|| queued_for(&manager, &preferred.id) == 1).await;
        let mut reader_request = retained_request("episode-reader");
        reader_request.retain = false;
        reader_request.reader_generation = Some(99);
        reader_request.expected_revision = Some("episode-reader-revision".to_owned());
        reader_request.page_indices = Some(vec![0]);
        let reader = manager.enqueue(reader_request).unwrap();
        wait_until(|| queued_for(&manager, &reader.id) == 1).await;
        let in_use = manager.remove_episode("episode-reader", "episode-reader-revision").unwrap_err();
        assert_eq!(in_use.downcast_ref::<DownloadControlError>(), Some(&DownloadControlError::ReaderInUse));

        let paused = manager.enqueue(retained_request("episode-paused")).unwrap();
        manager.pause(&paused.id).unwrap();
        let stale = record(directory.path(), &preferred.id);
        manager.prioritize(&preferred.id).unwrap();
        let store = Store::open(manager.database_path()).unwrap();
        let priority = store.load_download("fixture-account", &preferred.id).unwrap().unwrap().priority;
        assert!(priority > 0);
        store.save_download("fixture-account", &stale).unwrap();
        assert_eq!(Store::open(manager.database_path()).unwrap()
            .load_download("fixture-account", &preferred.id).unwrap().unwrap().priority, priority);
        assert!(manager.prioritize(&paused.id).is_err());
        assert!(manager.prioritize(&reader.id).is_err());
        assert_eq!(record(directory.path(), &paused.id).status, DownloadStatus::Paused);
        assert_eq!(record(directory.path(), &first.id).generation, first.generation);
        assert_eq!(source.cancelled.load(Ordering::SeqCst), 0);
        assert_eq!(source.requests.lock().unwrap().len(), 1);

        source.gate.add_permits(1);
        wait_until(|| source.requests.lock().unwrap().len() == 2).await;
        assert_eq!(source.requests.lock().unwrap()[1].0, "episode-b");
        assert_eq!(source.cancelled.load(Ordering::SeqCst), 0);
        source.gate.add_permits(1);
        wait_until(|| source.requests.lock().unwrap().len() == 3).await;
        assert_eq!(source.requests.lock().unwrap()[2].0, "episode-reader");
        source.gate.add_permits(1);
        wait_until(|| source.requests.lock().unwrap().len() == 4).await;
        assert_eq!(source.requests.lock().unwrap()[3].0, "episode-b");
        source.gate.add_permits(20);
        wait_until(|| [first.id.as_str(), fifo.id.as_str(), preferred.id.as_str()]
            .iter().all(|id| record(directory.path(), id).status == DownloadStatus::Completed)).await;
        let order = source.requests.lock().unwrap().clone();
        let last_first = order.iter().rposition(|(episode, _)| episode == "episode-a").unwrap();
        let first_fifo = order.iter().position(|(episode, _)| episode == "episode-c").unwrap();
        assert!(last_first < first_fifo);
        assert!(!order.iter().any(|(episode, _)| episode == "episode-paused"));
        assert_eq!(record(directory.path(), &paused.id).status, DownloadStatus::Paused);
        assert_eq!(record(directory.path(), &preferred.id).priority, priority);
        assert_eq!(source.cancelled.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn recovery_gc_keeps_receipts_journals_unknown_names_and_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let manager = DownloadManager::new(directory.path(), "fixture-account".to_owned(), 1,
            FixtureSource::new(0), runtime.handle().clone()).unwrap();
        let mut store = Store::open(manager.database_path()).unwrap();
        let index = ImageIndex {
            episode_id: "456".to_owned(), revision: "revision-one".to_owned(),
            images: (0..2).map(|index| PageDescriptor { id: format!("page-{index}"),
                index, width: 8, height: 12 }).collect(), ..Default::default()
        };
        store.save_index("fixture-account", &index).unwrap();
        let task = DownloadRecord {
            id: "gc-task".to_owned(), comic_id: "123".to_owned(), episode_id: "456".to_owned(),
            revision: "revision-one".to_owned(), status: DownloadStatus::Running, total_pages: 2,
            generation: 1, created_at: now(), updated_at: now(), ..Default::default()
        };
        store.save_download("fixture-account", &task).unwrap();
        let receipt = |page_index: usize, bytes: &[u8]| {
            let sha256 = digest_hex(bytes);
            PageReceipt { episode_id: "456".to_owned(), revision: "revision-one".to_owned(),
                page_id: format!("page-{page_index}"), page_index,
                file_name: format!("{sha256}.png"), sha256, bytes: bytes.len() as u64,
                width: 8, height: 12, verified_at: now(), ..Default::default() }
        };
        let referenced_bytes = fixture_png(34);
        let referenced = receipt(0, &referenced_bytes);
        store.begin_page_commit_generation("fixture-account", &task.id, 1, &referenced).unwrap();
        write_object(manager.pages_root(), &referenced.file_name, &referenced_bytes).unwrap();
        store.commit_page_generation("fixture-account", &task.id, 1, &referenced).unwrap();
        let journal_bytes = fixture_png(68);
        let journal = receipt(1, &journal_bytes);
        store.begin_page_commit_generation("fixture-account", &task.id, 1, &journal).unwrap();
        write_object(manager.pages_root(), &journal.file_name, &journal_bytes).unwrap();
        let orphan_bytes = fixture_png(102);
        let orphan_name = format!("{}.png", digest_hex(&orphan_bytes));
        write_object(manager.pages_root(), &orphan_name, &orphan_bytes).unwrap();
        let unknown = manager.pages_root().join("operator-note.txt");
        fs::write(&unknown, b"Keep this operator-owned file").unwrap();
        #[cfg(unix)]
        let symlink = {
            let path = manager.pages_root().join(format!("{}.png", "e".repeat(64)));
            std::os::unix::fs::symlink(manager.pages_root().join(&referenced.file_name), &path).unwrap();
            path
        };
        let _publication = manager.context.publication.lock().unwrap();
        let removed = cleanup_unreferenced_objects(manager.pages_root(), &store, "fixture-account").unwrap();
        assert_eq!(removed, orphan_bytes.len() as u64);
        assert!(!manager.pages_root().join(orphan_name).exists());
        assert!(manager.pages_root().join(&referenced.file_name).exists());
        assert!(manager.pages_root().join(&journal.file_name).exists());
        assert!(unknown.exists());
        #[cfg(unix)]
        assert!(fs::symlink_metadata(symlink).unwrap().file_type().is_symlink());
        assert_eq!(store.pending_page_commits("fixture-account").unwrap().len(), 1);
    }
}

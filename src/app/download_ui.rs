use super::*;
use crate::{
    downloads::{DownloadEventKind, DownloadRequest},
    performance::{Operation, Span},
    platform::WakeGuard,
    store::{DownloadRecord, DownloadStatus},
    ui::{DownloadChapterItem, DownloadItem},
};

#[derive(Default)]
pub(super) struct DownloadState {
    deferred: Vec<DeferredDownload>,
    pending_controls: Vec<(String, String)>,
    deferred_priorities: BTreeMap<String, u64>,
    sleep: Option<DownloadSleepSnapshot>,
    sleep_retry_at: Option<Instant>,
    page: usize,
    chapter_page: usize,
    window_height: u32,
    selecting: bool,
    selected: BTreeSet<String>,
    sheet: Option<String>,
    demo_paused: BTreeSet<String>,
    demo_removed: BTreeSet<String>,
    demo_enqueued: BTreeMap<String, BTreeMap<String, f64>>,
    demo_retried: BTreeSet<String>,
    demo_cancelled: BTreeSet<String>,
    demo_priority: BTreeMap<String, u64>,
    demo_priority_sequence: u64,
    wake: Option<WakeGuard>,
}

impl DownloadState {
    pub(super) fn close_overlays(&mut self) {
        self.selecting = false;
        self.selected.clear();
        self.sheet = None;
        self.chapter_page = 0;
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct DeferredDownload {
    comic_id: String,
    episode_id: String,
    title: String,
    comic_title: String,
    revision: Option<String>,
    retain: bool,
    #[serde(default)]
    paused: bool,
}

struct DownloadSleepSnapshot {
    cycle: u64,
    account: String,
    epoch: u64,
    resume_requested: bool,
    tasks: BTreeMap<String, SleepDownloadTask>,
    deferred: Vec<DeferredDownload>,
    demo_ids: BTreeSet<String>,
}

impl DownloadSleepSnapshot {
    fn belongs_to(&self, account: &str, epoch: u64) -> bool { self.account == account && self.epoch == epoch }
}

#[derive(Clone, Debug)]
struct SleepDownloadTask {
    id: String,
    episode_id: String,
    revision: String,
    paused_generation: u64,
    retained: bool,
    reader_generation: Option<u64>,
    needs_control: bool,
}

impl SleepDownloadTask {
    fn can_resume(&self, record: &DownloadRecord, reader: Option<&super::reading::ReaderSleepLease>) -> bool {
        record.id == self.id && record.episode_id == self.episode_id && record.revision == self.revision
            && record.generation == self.paused_generation && record.status == DownloadStatus::Paused
            && record.retained == self.retained && !self.needs_control
            && (self.retained || reader.is_some_and(|reader| self.reader_generation == Some(reader.generation)
                && reader.episode_id == self.episode_id && reader.revision == self.revision
                && reader.task_id.as_deref() == Some(self.id.as_str())))
    }
}

impl DeferredDownload {
    fn request(&self) -> DownloadRequest {
        DownloadRequest { comic_id: self.comic_id.clone(), episode_id: self.episode_id.clone(),
            title: self.title.clone(), comic_title: self.comic_title.clone(),
            expected_revision: if self.retain { None } else { self.revision.clone() }, retain: self.retain, ..Default::default() }
    }
}

#[derive(Clone, Default)]
struct DownloadRow {
    id: String,
    title: String,
    status: String,
    subtitle: String,
    summary: String,
    task_title: String,
    task_note: String,
    task_has_bar: bool,
    has_bar: bool,
    can_read: bool,
    priority: u64,
    progress: f32,
    error: bool,
    active: bool,
    queued: bool,
    paused: bool,
    bytes: u64,
    chapters: Vec<DownloadChapterItem>,
}

impl DownloadRow {
    fn has_task(&self) -> bool { self.error || self.active || self.queued || self.paused }

    fn describe_task(&mut self, label: String, saved: u64, total: u64, failure: String) {
        let pages = (total > 0).then(|| page_count_label("dl_page_count", saved, total));
        self.progress = if total == 0 { 0.0 } else { saved as f32 / total as f32 };
        self.task_has_bar = !self.error && (self.active || self.paused);
        self.has_bar = self.task_has_bar || self.error && total > 0;
        if self.error {
            self.status = i18n::text("dl_failed");
            self.task_title = i18n::format("dl_failed_episode", &[("episode", label.clone())]);
            let mut subtitle = vec![label, failure.clone()];
            let mut note = vec![failure];
            if let Some(pages) = pages {
                subtitle.push(pages);
                note.push(page_count_label("dl_saved_pages", saved, total));
            }
            self.subtitle = subtitle.join(" · ");
            self.task_note = note.join(" · ");
        } else if self.active || self.paused {
            self.status = if self.paused { i18n::text("dl_paused") }
                else { i18n::format("dl_progress_value", &[("percent", format!("{:.0}", self.progress * 100.0))]) };
            self.task_title = i18n::format(if self.paused { "dl_paused_episode" } else { "dl_downloading_episode" },
                &[("episode", label.clone())]);
            self.subtitle = pages.as_ref().map_or_else(|| label.clone(), |pages| format!("{label} · {pages}"));
            self.task_note = pages.unwrap_or_else(|| i18n::text("dl_not_started"));
        } else if self.queued {
            self.status = i18n::text("dl_queued");
            self.task_title = i18n::format("dl_queued_episode", &[("episode", label.clone())]);
            self.subtitle = pages.filter(|_| saved > 0).map_or_else(|| label.clone(), |pages| format!("{label} · {pages}"));
        } else {
            self.status = i18n::text(if self.can_read { "dl_offline_ready" } else { "dl_cancelled" });
            self.subtitle = self.summary.clone();
        }
    }
}

fn recovery_span(trace: Option<Operation>, stage: &'static str) -> Option<Span> {
    trace.map(|trace| { trace.mark(stage); Span::start() })
}

fn finish_recovery_span(trace: Option<Operation>, stage: &'static str, span: Option<Span>, outcome: &'static str) {
    if let (Some(trace), Some(span)) = (trace, span) {
        let sample = span.finish();
        trace.record(stage, &serde_json::json!({
            "outcome": outcome, "wall_us": sample.wall_us, "cpu_us": sample.cpu_us,
        }));
    }
}

impl App {
    pub(super) fn recover_downloads(&mut self) {
        self.recover_downloads_traced(None);
    }

    pub(super) fn recover_downloads_traced(&mut self, parent: Option<Operation>) {
        let trace = Operation::new("download-recovery");
        if let Some(trace) = trace {
            trace.record("recovery-request", &serde_json::json!({
                "parent_operation": parent.map(|parent| parent.id()),
                "epoch": self.epoch, "lifecycle_cycle": self.lifecycle_cycle(),
            }));
        }
        self.cache_download_pause_handle();
        if self.is_suspended() {
            if let Some(trace) = trace { trace.mark("recovery-deferred-suspended"); }
            self.set_download_transport_suspended(true);
            return;
        }
        let Some(manager) = self.downloads.as_mut() else {
            if let Some(trace) = trace { trace.mark("recovery-no-manager"); }
            return;
        };
        // A newly created manager starts with its default capacity. Restore the
        // saved preference before recovery can admit any page acquisitions.
        if manager.set_concurrency(self.preferences.concurrency.clamp(1, 4) as usize).is_err() {
            if let Some(trace) = trace { trace.mark("recovery-configuration-failed"); }
            self.toast("common_storage_error");
            return;
        }
        let admission = match self.begin_job("download-recover", false) {
            Ok(admission) => admission,
            Err(_) => {
                if let Some(trace) = trace { trace.mark("recovery-admission-failed"); }
                self.toast("dl_background_busy");
                return;
            }
        };
        if let Some(trace) = trace {
            trace.record("recovery-admitted", &serde_json::json!({
                "epoch": admission.0, "job_sequence": admission.1,
            }));
        }
        let manager = self.downloads.take().expect("Admission preserves the download manager");
        if let Some(trace) = trace { trace.mark("manager-transferred-to-recovery"); }
        let owner = std::sync::Arc::new(std::sync::Mutex::new(Some(manager)));
        let work_owner = owner.clone();
        let resume = !self.is_suspended() && !self.options.offline && self.client.session().is_some();
        if let Some(trace) = trace {
            trace.record("recovery-mode", &serde_json::json!({
                "mode": if resume { "resume" } else { "pause_all" },
            }));
        }
        self.background_reserved_traced("download-recover", admission, trace, move || {
            let span = recovery_span(trace, "recovery-work-begin");
            let result = (|| -> Result<bool> {
                let mut ownership = work_owner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let manager = ownership.as_mut().context("Download manager ownership is unavailable")?;
                let recovered = if resume { manager.recover() } else { manager.pause_all() };
                Ok(recovered.is_ok())
            })();
            finish_recovery_span(trace, "recovery-work-end", span,
                if matches!(&result, Ok(true)) { "success" } else { "failed" });
            result
        }, move |app, result| {
            let span = recovery_span(trace, "recovery-result-apply-begin");
            let mut outcome = if matches!(&result, Ok(true)) { "success" } else { "partial" };
            if let Some(trace) = trace {
                trace.record("recovery-result", &serde_json::json!({
                    "epoch": app.epoch, "lifecycle_cycle": app.lifecycle_cycle(),
                    "outcome": if matches!(&result, Ok(true)) { "success" } else { "failed" },
                }));
            }
            let manager = owner.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
            app.invalidate_catalog_local_state();
            if !matches!(result, Ok(true)) { app.toast("common_storage_error"); }
            if let Some(manager) = manager {
                if app.restore_download_manager_traced(manager, trace).is_err() {
                    outcome = "partial";
                    app.toast("common_storage_error");
                }
            } else {
                outcome = "partial";
                if let Some(trace) = trace { trace.mark("recovery-manager-unavailable"); }
                app.prepare_download_manager();
            }
            finish_recovery_span(trace, "recovery-result-apply-end", span, outcome);
        });
    }

    pub(super) fn restore_download_manager(&mut self, manager: DownloadManager) -> Result<()> {
        self.restore_download_manager_traced(manager, None)
    }

    fn restore_download_manager_traced(&mut self, manager: DownloadManager, trace: Option<Operation>) -> Result<()> {
        let span = recovery_span(trace, "manager-restore-begin");
        let result = (|| {
            // Settings may have changed while a background job owned the manager.
            manager.set_concurrency(self.preferences.concurrency.clamp(1, 4) as usize)?;
            self.downloads = Some(manager);
            if let Some(trace) = trace { trace.mark("manager-restored-installed"); }
            self.cache_download_pause_handle();
            if self.is_suspended() { self.set_download_transport_suspended(true); }
            self.reconcile_download_controls_after_sleep()?;
            let sync_span = recovery_span(trace, "restore-reader-lease-sync-begin");
            let synced = self.sync_reader_lease();
            finish_recovery_span(trace, "restore-reader-lease-sync-end", sync_span,
                if synced.is_ok() { "success" } else { "failed" });
            synced?;
            if self.is_suspended() {
                if let Some(trace) = trace { trace.mark("manager-restore-suspended"); }
                self.pause_downloads_for_sleep(self.lifecycle_cycle())?;
                return Ok(());
            }
            self.resume_downloads_from_sleep()?;
            self.flush_deferred_downloads()?;
            if let Some(trace) = trace { trace.mark("restore-reader-page-notify-begin"); }
            let notified = self.reader_page_ready();
            if let Some(trace) = trace {
                trace.record("restore-reader-page-notify-end", &serde_json::json!({
                    "outcome": if notified.is_ok() { "success" } else { "failed" },
                }));
            }
            notified?;
            Ok(())
        })();
        finish_recovery_span(trace, "manager-restore-end", span,
            if result.is_err() { "failed" } else if self.is_suspended() { "partial" } else { "success" });
        result
    }

    fn flush_deferred_downloads(&mut self) -> Result<()> {
        if self.is_suspended() || self.options.offline || self.demo.is_some()
            || self.client.session().and_then(|session| session.account_key().map(str::to_owned)).as_deref() != Some(self.account.as_str())
        { return Ok(()); }
        let Some(manager) = &mut self.downloads else { return Ok(()); };
        let mut pending = std::mem::take(&mut self.download_ui.deferred);
        pending.sort_by_key(|intent| std::cmp::Reverse(self.download_ui.deferred_priorities.get(&intent.episode_id).copied().unwrap_or_default()));
        let mut failed = false;
        let mut priorities = Vec::new();
        for intent in pending {
            if intent.paused { self.download_ui.deferred.push(intent); continue; }
            let queued = if intent.retain { manager.enqueue_fresh(intent.request()) }
                else { manager.enqueue(intent.request()) };
            match queued {
                Ok(record) => {
                    if let Some(priority) = self.download_ui.deferred_priorities.get(&intent.episode_id) {
                        priorities.push((*priority, record.id, intent.episode_id));
                    }
                }
                Err(_) => { failed = true; self.download_ui.deferred.push(intent); }
            }
        }
        priorities.sort_by_key(|(priority, _, _)| *priority);
        for (_, task, episode) in priorities {
            if manager.prioritize(&task).is_err() {
                self.download_ui.pending_controls.push((task, "priority".into())); failed = true;
            } else { self.download_ui.deferred_priorities.remove(&episode); }
        }
        self.store.set_preference(&self.account, "queued_download_intents", &self.download_ui.deferred)?;
        self.store.set_preference(&self.account, "queued_download_priorities", &self.download_ui.deferred_priorities)?;
        if failed { self.toast("common_storage_error"); }
        Ok(())
    }

    pub(super) fn initialize_downloads(&mut self) -> Result<()> {
        let sleep = self.download_ui.sleep.take().filter(|snapshot| snapshot.belongs_to(&self.account, self.epoch));
        let controls = if sleep.is_some() { std::mem::take(&mut self.download_ui.pending_controls) } else { Vec::new() };
        self.download_ui = DownloadState::default();
        self.download_ui.sleep = sleep;
        self.download_ui.pending_controls = controls;
        self.download_ui.deferred = self.store.preference(&self.account, "queued_download_intents")?.unwrap_or_default();
        self.download_ui.deferred_priorities = self.store.preference(&self.account, "queued_download_priorities")?.unwrap_or_default();
        let (_, pending) = self.download_activity()?;
        self.project_download_badge(pending);
        Ok(())
    }

    fn download_activity(&self) -> Result<(bool, usize)> {
        if let Some(demo) = &self.demo {
            let active = demo.fixture["downloads"].as_array().into_iter().flatten().filter(|value| {
                let id = value["comic_id"].as_u64().unwrap_or_default().to_string();
                !self.download_ui.demo_enqueued.contains_key(&id)
                    && matches!(self.demo_seed_status(&id, value), DownloadStatus::Queued | DownloadStatus::Running)
            }).count();
            let enqueued = self.download_ui.demo_enqueued.keys().filter(|id| !self.download_ui.demo_paused.contains(*id)).count();
            return Ok((false, active + enqueued));
        }
        let deferred = self.download_ui.deferred.iter().filter(|intent| intent.retain && !intent.paused)
            .map(|intent| intent.comic_id.as_str()).collect::<Vec<_>>();
        self.store.download_queue_activity(&self.account, &deferred).map(|(running, pending)| (running, pending as usize))
    }

    fn project_download_badge(&self, pending: usize) {
        if let Some(ui) = self.ui.upgrade() {
            ui.global::<ShellModel>().set_pending(pending.min(i32::MAX as usize) as i32);
        }
    }

    pub(super) fn episode_download_queued(&self, episode_id: &str) -> bool {
        if let Some(demo) = &self.demo {
            return self.download_ui.demo_enqueued.values().any(|episodes| episodes.contains_key(episode_id))
                || demo.fixture["downloads"].as_array().into_iter().flatten().any(|value| {
                    let comic = value["comic_id"].as_u64().unwrap_or_default();
                    matches!(self.demo_seed_status(&comic.to_string(), value), DownloadStatus::Running | DownloadStatus::Queued | DownloadStatus::Paused)
                        && value["task_orders"].as_array().into_iter().flatten().filter_map(|value| value.as_u64())
                            .any(|order| demo_episode_id(comic, order) == episode_id)
                });
        }
        self.download_ui.deferred.iter().any(|intent| intent.retain && intent.episode_id == episode_id)
            || self.store.episode_download_pending(&self.account, episode_id).unwrap_or(false)
    }

    pub(super) fn enqueue_episode(&mut self, episode_id: &str, retain: bool) -> Result<()> {
        self.try_enqueue_episode(episode_id, retain).map(|_| ())
    }

    pub(super) fn try_enqueue_episode(&mut self, episode_id: &str, retain: bool) -> Result<bool> {
        if self.is_suspended() { return Ok(false); }
        if !self.require_session() { return Ok(false); }
        let detail = self.catalog.detail.clone().context("No comic is selected for acquisition")?;
        let episode = detail.episodes.iter().find(|episode| episode.id == episode_id).context("Episode is absent from the current catalog")?;
        if !episode.readable_at(now()) { self.open_purchase(episode_id)?; return Ok(false); }
        if retain && self.episode_download_queued(episode_id) { return Ok(true); }
        if self.demo.is_some() {
            if retain {
                self.download_ui.demo_enqueued.entry(detail.comic.id.clone()).or_default().insert(episode.id.clone(), episode.order);
                self.toast("common_download_queued");
            }
            return Ok(true);
        }
        if !self.require_network("downloads", if retain { "enqueue" } else { "enqueue-prefetch" }, episode_id) { return Ok(false); }
        // Explicit retained downloads refresh access and the source directory;
        // reader prefetch continues to target its existing source revision.
        let expected = if retain { None }
            else { self.store.load_index(&self.account, episode_id, None)?.map(|index| index.revision) };
        let request = DownloadRequest {
            comic_id: detail.comic.id.clone(), episode_id: episode.id.clone(), title: episode.title.clone(),
            comic_title: detail.comic.title.clone(), expected_revision: expected,
            retain, ..Default::default()
        };
        let Some(manager) = &mut self.downloads else {
            self.download_ui.deferred.push(DeferredDownload { comic_id: request.comic_id, episode_id: request.episode_id,
                title: request.title, comic_title: request.comic_title, revision: request.expected_revision, retain: request.retain, paused: false });
            self.store.set_preference(&self.account, "queued_download_intents", &self.download_ui.deferred)?;
            self.toast("common_download_queued"); return Ok(true);
        };
        manager.set_concurrency(self.preferences.concurrency.clamp(1, 4) as usize)?;
        if retain { manager.enqueue_fresh(request)?; } else { manager.enqueue(request)?; }
        if retain { self.toast("common_download_queued"); }
        self.dirty = true;
        Ok(true)
    }

    pub(super) fn tick_downloads(&mut self) -> Result<()> {
        if self.is_suspended() { self.download_ui.wake = None; return Ok(()); }
        let can_resume_network = !self.options.offline
            && self.client.credential_context().map(|(account, _)| account) == Some(self.account.clone());
        let retry_controls = self.downloads.is_some() && (!self.download_ui.pending_controls.is_empty()
            || can_resume_network && self.download_ui.sleep.as_ref().is_some_and(|snapshot| snapshot.resume_requested));
        if retry_controls && self.download_ui.sleep_retry_at.is_none_or(|last| last.elapsed() >= Duration::from_secs(1)) {
            self.download_ui.sleep_retry_at = Some(Instant::now());
            if self.reconcile_download_controls_after_sleep().and_then(|_| self.resume_downloads_from_sleep()).is_err() {
                self.mark_lifecycle_checkpoint_error();
                self.toast("common_storage_error");
                return Ok(());
            }
        }
        let events = self.downloads.as_mut().map(|manager| manager.drain_events(128)).unwrap_or_default();
        let mut reader_changed = false;
        let mut enforce_limit = false;
        for event in events {
            if event.account_epoch != self.epoch { continue; }
            if matches!(&event.kind, DownloadEventKind::IndexReady { .. } | DownloadEventKind::PageReady { .. }
                | DownloadEventKind::Completed | DownloadEventKind::PrefetchReady | DownloadEventKind::Failed { .. }
                | DownloadEventKind::Cancelled) {
                match self.store.load_download(&self.account, &event.task_id) {
                    Ok(Some(record)) => self.invalidate_catalog_local_episode(&record.comic_id, &record.episode_id),
                    Ok(None) | Err(_) => self.invalidate_catalog_local_state(),
                }
            }
            enforce_limit |= matches!(event.kind, DownloadEventKind::Completed | DownloadEventKind::PrefetchReady);
            reader_changed |= self.reader_download_dependency_changed(&event);
            self.dirty = true;
            let _ = self.platform.request_refresh(RefreshReason::Progress);
        }
        if reader_changed { self.reader_page_ready()?; }
        if enforce_limit {
            if self.enforce_cache_limit().is_err() { self.toast("common_storage_error"); }
            self.invalidate_account_storage();
        }
        let (active, pending) = self.download_activity()?;
        self.project_download_badge(pending);
        if active && self.platform.capabilities().keep_awake && self.download_ui.wake.is_none() {
            self.download_ui.wake = self.platform.hold_awake_deferred().ok();
        }
        if !active { self.download_ui.wake = None; }
        if self.route == "downloads" && self.ui.upgrade().is_some_and(|ui| self.download_window_height(&ui) != self.download_ui.window_height) {
            self.view_dirty.mark(super::View::Downloads);
        }
        Ok(())
    }

    fn demo_seed_status(&self, id: &str, seed: &serde_json::Value) -> DownloadStatus {
        if self.download_ui.demo_removed.contains(id) || self.download_ui.demo_cancelled.contains(id) {
            return DownloadStatus::Cancelled;
        }
        let status = if self.download_ui.demo_retried.contains(id) { DownloadStatus::Queued }
            else { match seed["kind"].as_str() {
                Some("active") => DownloadStatus::Running,
                Some("queued") => DownloadStatus::Queued,
                Some("error") => DownloadStatus::Failed,
                _ => DownloadStatus::Completed,
            }};
        if self.download_ui.demo_paused.contains(id) && matches!(status, DownloadStatus::Running | DownloadStatus::Queued) {
            DownloadStatus::Paused
        } else { status }
    }

    pub(super) fn demo_manual_download_usage(&self) -> Option<u64> { self.demo_download_bytes(false) }

    pub(super) fn demo_removed_download_bytes(&self) -> u64 { self.demo_download_bytes(true).unwrap_or_default() }

    fn demo_download_bytes(&self, removed: bool) -> Option<u64> {
        let demo = self.demo.as_ref()?;
        Some(demo.fixture["downloads"].as_array().into_iter().flatten().filter(|value| {
            let id = value["comic_id"].as_u64().unwrap_or_default().to_string();
            self.download_ui.demo_removed.contains(&id) == removed
        }).map(demo_storage_bytes).sum())
    }

    fn demo_download_rows(&self, demo: &DemoData) -> Vec<DownloadRow> {
        let mut rows = Vec::new();
        for seed in demo.fixture["downloads"].as_array().into_iter().flatten() {
            let id = seed["comic_id"].as_u64().unwrap_or_default().to_string();
            if self.download_ui.demo_removed.contains(&id) { continue; }
            let status = self.demo_seed_status(&id, seed);
            let bytes = demo_storage_bytes(seed);
            if status == DownloadStatus::Cancelled && bytes == 0 { continue; }
            let (chapters, complete) = demo_download_chapters(seed);
            let mut row = DownloadRow { id, title: seed["title"].as_str().unwrap_or_default().into(),
                error: status == DownloadStatus::Failed, active: status == DownloadStatus::Running,
                queued: status == DownloadStatus::Queued, paused: status == DownloadStatus::Paused,
                can_read: complete > 0, bytes, chapters, ..Default::default() };
            let mut orders = if row.has_task() {
                seed["task_orders"].as_array().into_iter().flatten().filter_map(|value| value.as_f64()).collect()
            } else { Vec::new() };
            let (saved, total) = if row.has_task() { demo_page_counts(seed) } else { (0, 0) };
            if let Some(added) = self.download_ui.demo_enqueued.get(&row.id) {
                if !row.error { orders.extend(added.values().copied()); }
                if self.download_ui.demo_paused.contains(&row.id) { row.paused = !row.active; }
                else { row.queued = true; }
            }
            row.priority = self.download_ui.demo_priority.get(&row.id).copied().unwrap_or_default();
            row.summary = download_summary(complete, format_design_bytes(bytes), row.has_task());
            row.describe_task(download_episode_scope(orders), saved, total,
                download_failure_message(seed["failure_code"].as_str().unwrap_or_default()));
            rows.push(row);
        }
        for (id, episodes) in &self.download_ui.demo_enqueued {
            if rows.iter().any(|row| &row.id == id) { continue; }
            let Some(comic) = demo.comics.iter().find(|comic| &comic.id == id) else { continue; };
            let paused = self.download_ui.demo_paused.contains(id);
            let mut row = DownloadRow { id: id.clone(), title: comic.title.clone(), queued: !paused, paused,
                priority: self.download_ui.demo_priority.get(id).copied().unwrap_or_default(),
                summary: i18n::text("dl_no_complete_episodes"), ..Default::default() };
            row.describe_task(download_episode_scope(episodes.values().copied().collect()), 0, 0, String::new());
            rows.push(row);
        }
        rows
    }

    fn download_rows(&self) -> Result<Vec<DownloadRow>> {
        if let Some(demo) = &self.demo {
            let mut rows = self.demo_download_rows(demo);
            sort_download_rows(&mut rows);
            return Ok(rows);
        }
        let mut groups = BTreeMap::<String, Vec<DownloadRecord>>::new();
        for record in self.store.downloads(&self.account)? {
            if record.retained && (record.status != DownloadStatus::Cancelled || record.completed_pages > 0) {
                groups.entry(record.comic_id.clone()).or_default().push(record);
            }
        }
        for intent in self.download_ui.deferred.iter().filter(|intent| intent.retain) {
            groups.entry(intent.comic_id.clone()).or_default().push(DownloadRecord {
                id: format!("deferred:{}", intent.episode_id), comic_id: intent.comic_id.clone(), episode_id: intent.episode_id.clone(),
                title: intent.title.clone(), comic_title: intent.comic_title.clone(),
                revision: intent.revision.clone().unwrap_or_else(|| crate::store::PENDING_REVISION.into()),
                priority: self.download_ui.deferred_priorities.get(&intent.episode_id).copied().unwrap_or_default(),
                status: if intent.paused { DownloadStatus::Paused } else { DownloadStatus::Queued }, retained: true, ..Default::default()
            });
        }
        let mut rows = Vec::new();
        for (id, records) in groups {
            let has = |status| records.iter().any(|record| record.status == status);
            let active = has(DownloadStatus::Running);
            let queued = has(DownloadStatus::Queued);
            let paused = !active && !queued && has(DownloadStatus::Paused);
            let error = has(DownloadStatus::Failed);
            let detail = self.store.load_detail(&self.account, &id)?;
            let order_by_id: BTreeMap<_, _> = detail.iter().flat_map(|detail| &detail.episodes)
                .map(|episode| (episode.id.as_str(), episode.order)).collect();
            let order = |record: &DownloadRecord| order_by_id.get(record.episode_id.as_str()).copied();
            let mut local = BTreeMap::<&str, &DownloadRecord>::new();
            for record in records.iter().filter(|record| record.completed_pages > 0) {
                let episode = record.episode_id.as_str();
                if local.get(episode).is_none_or(|previous| local_chapter_rank(record) > local_chapter_rank(previous)) {
                    local.insert(episode, record);
                }
            }
            let mut local: Vec<_> = local.into_values().collect();
            local.sort_by(|left, right| order(left).unwrap_or(f64::INFINITY).total_cmp(&order(right).unwrap_or(f64::INFINITY))
                .then_with(|| left.episode_id.cmp(&right.episode_id)));
            let complete = local.iter().filter(|record| record.status == DownloadStatus::Completed).count();
            let chapters = local.iter().map(|record| DownloadChapterItem {
                id: record.episode_id.clone().into(), number: order(record).map(episode_number).unwrap_or_default().into(),
                title: record.title.clone().into(), size: if record.status == DownloadStatus::Completed { format_bytes(record.bytes) }
                    else { page_count_label("dl_page_count", record.completed_pages.into(), record.total_pages.into()) }.into(),
            }).collect();
            let bytes = records.iter().fold(0_u64, |total, record| total.saturating_add(record.bytes));
            let priority = records.iter().filter(|record| matches!(record.status, DownloadStatus::Queued | DownloadStatus::Running))
                .map(|record| record.priority).max().unwrap_or_default();
            let mut row = DownloadRow { id, title: records.first().map(|record| record.comic_title.clone()).unwrap_or_default(),
                error, active, queued, paused, can_read: complete > 0, bytes, chapters, priority, ..Default::default() };
            row.summary = download_summary(complete, format_bytes(bytes), row.has_task());
            let tasks: Vec<_> = records.iter().filter(|record| if error { record.status == DownloadStatus::Failed }
                else if active { matches!(record.status, DownloadStatus::Running | DownloadStatus::Queued) }
                else if queued { record.status == DownloadStatus::Queued }
                else { paused && record.status == DownloadStatus::Paused }).collect();
            let orders = tasks.iter().filter_map(|record| order(record)).collect::<Vec<_>>();
            let label = if orders.is_empty() { tasks.first().filter(|record| !record.title.is_empty())
                .map_or_else(|| i18n::text("dl_current_episode"), |record| record.title.clone()) }
                else { download_episode_scope(orders) };
            let (saved, total) = tasks.iter().fold((0_u64, 0_u64), |(saved, total), record|
                (saved + u64::from(record.completed_pages), total + u64::from(record.total_pages)));
            let failure = tasks.first().and_then(|record| record.error_code.as_deref()).map(download_failure_message)
                .unwrap_or_else(|| i18n::text("common_download_failed"));
            row.describe_task(label, saved, total, failure);
            rows.push(row);
        }
        sort_download_rows(&mut rows);
        Ok(rows)
    }

    pub(super) fn close_download_overlays(&mut self) {
        self.download_ui.close_overlays();
        if let Some(ui) = self.ui.upgrade() {
            let model = ui.global::<DownloadModel>();
            model.set_selecting(false);
            model.set_selected_count(0);
            reset_download_sheet(&model);
        }
    }

    pub(super) fn refresh_download_view(&mut self, ui: &AppWindow) -> Result<()> {
        self.schedule_storage_summary();
        let rows = self.download_rows()?;
        let scribe = self.is_scribe();
        let height = self.download_window_height(ui);
        self.download_ui.window_height = height;
        let per_page = download_list_capacity(scribe, height, rows.len());
        let pages = rows.len().div_ceil(per_page).max(1);
        self.download_ui.page = self.download_ui.page.min(pages - 1);
        self.download_ui.selected.retain(|id| rows.iter().any(|row| &row.id == id));
        if rows.is_empty() { self.download_ui.selecting = false; }
        let mut items = Vec::new();
        for row in rows.iter().skip(self.download_ui.page * per_page).take(per_page) {
            let comic = self.download_comic(row)?;
            items.push(DownloadItem { id: row.id.clone().into(), title: row.title.clone().into(), cover: self.cover_image(&comic),
                status: row.status.clone().into(), subtitle: row.subtitle.clone().into(), progress: row.progress,
                error: row.error, active: row.active && !row.error, has_bar: row.has_bar,
                selected: self.download_ui.selected.contains(&row.id) });
        }
        let model = ui.global::<DownloadModel>();
        model.set_items(super::model(items));
        model.set_selecting(self.download_ui.selecting);
        let selected_count = self.download_ui.selected.len();
        model.set_selected_count(selected_count as i32);
        model.set_selection_label(i18n::format("dl_selected_count", &[("count", selected_count.to_string())]).into());
        model.set_remove_label(if selected_count == 0 { tr("dl_remove_short") }
            else if let Ok(bytes) = self.selected_removal_estimate(&rows) {
                let size = if self.demo.is_some() { format_design_bytes(bytes) } else { format_bytes(bytes) };
                i18n::format("dl_remove_release", &[("size", size)]).into()
            } else { tr("dl_remove_selected") });
        model.set_smart_label(i18n::format("dl_smart_value", &[("state", i18n::text(if self.preferences.smart { "acct_on" } else { "acct_off" }))]).into());
        model.set_page(self.download_ui.page as i32 + 1);
        model.set_page_count(pages as i32);
        let active = rows.iter().filter(|row| row.active || row.queued).count();
        let paused = rows.iter().filter(|row| row.paused).count();
        let failed = rows.iter().filter(|row| row.error).count();
        self.project_download_badge(active);
        let (has_controls, all_paused) = global_download_controls(&rows);
        model.set_has_active(has_controls);
        model.set_all_paused(all_paused);
        let status: Vec<_> = [("dl_status_active", active), ("dl_status_paused", paused), ("dl_status_failed", failed)]
            .into_iter().filter(|(_, count)| *count > 0)
            .map(|(key, count)| i18n::format(key, &[("count", count.to_string())])).collect();
        model.set_status_summary(if status.is_empty() { i18n::text("dl_title") } else { status.join(" · ") }.into());
        let (used, free) = self.download_storage_labels();
        model.set_summary(if rows.is_empty() { i18n::format("dl_storage_remaining", &[("free", free)]) }
            else { i18n::format("dl_storage_summary", &[("count", rows.len().to_string()), ("size", used), ("free", free)]) }.into());
        let selected = self.download_ui.sheet.as_ref().and_then(|id| rows.iter().find(|row| &row.id == id));
        if let Some(row) = selected {
            let comic = self.download_comic(row)?;
            model.set_sheet_id(row.id.clone().into());
            model.set_sheet_title(row.title.clone().into());
            model.set_sheet_summary(row.summary.clone().into());
            model.set_sheet_cover(self.cover_image(&comic));
            let capacity = download_chapter_capacity(scribe, height, row.has_task());
            let chapter_pages = row.chapters.len().div_ceil(capacity).max(1);
            self.download_ui.chapter_page = self.download_ui.chapter_page.min(chapter_pages - 1);
            model.set_chapter_page(self.download_ui.chapter_page as i32 + 1);
            model.set_chapter_page_count(chapter_pages as i32);
            model.set_chapters(super::model(row.chapters.iter().skip(self.download_ui.chapter_page * capacity).take(capacity).cloned().collect()));
            model.set_task_visible(row.has_task());
            model.set_task_has_bar(row.task_has_bar);
            model.set_task_title(row.task_title.clone().into());
            model.set_task_note(row.task_note.clone().into());
            model.set_task_progress(row.progress);
            let prioritized = row.queued && !row.error && !row.active && is_prioritized(row, &rows);
            model.set_task_primary(tr(if row.error { "dl_retry" } else if row.active { "dl_pause" }
                else if prioritized { "dl_prioritized" } else if row.queued { "dl_priority" } else { "dl_resume" }));
            model.set_task_primary_enabled(!prioritized);
            model.set_task_primary_action(if row.error { "retry" } else if row.active { "pause" } else if row.queued { "priority" } else { "resume" }.into());
            model.set_task_secondary(tr(if row.error { "dl_release_space" } else { "common_cancel" }));
            model.set_task_secondary_action(if row.error { "storage" } else if row.queued && !row.active { "cancel-queued" } else { "cancel" }.into());
            model.set_can_read(row.can_read);
            model.set_remove_current_label(tr(if row.bytes > 0 { "dl_remove_all_downloads" } else { "dl_cancel_download" }));
            model.set_remove_current_action(if row.bytes > 0 { "remove-current" } else { "cancel-current" }.into());
            model.set_sheet_open(true);
        } else {
            self.download_ui.sheet = None;
            self.download_ui.chapter_page = 0;
            reset_download_sheet(&model);
        }
        Ok(())
    }

    fn download_comic(&self, row: &DownloadRow) -> Result<Comic> {
        Ok(self.store.comic(&self.account, &row.id)?.unwrap_or_else(|| Comic { id: row.id.clone(), title: row.title.clone(), ..Default::default() }))
    }

    fn download_window_height(&self, ui: &AppWindow) -> u32 {
        let height = ui.window().size().height;
        let scale = ui.window().scale_factor();
        if height > 0 && scale.is_finite() && scale > 0.0 { return (height as f32 / scale).round() as u32; }
        let (width, height) = self.platform.logical_viewport_size();
        if self.options.landscape { width } else { height }
    }

    fn selected_removal_estimate(&self, rows: &[DownloadRow]) -> Result<u64> {
        if self.download_ui.selected.is_empty() { return Ok(0); }
        if self.demo.is_some() {
            return Ok(rows.iter().filter(|row| self.download_ui.selected.contains(&row.id)).map(|row| row.bytes).sum());
        }
        let records = self.store.downloads(&self.account)?;
        let mut objects = BTreeMap::<String, (u64, BTreeSet<String>)>::new();
        let mut seen = BTreeSet::new();
        for record in records {
            if !seen.insert((record.comic_id.clone(), record.episode_id.clone(), record.revision.clone())) { continue; }
            for receipt in self.store.page_receipts(&self.account, &record.episode_id, &record.revision)? {
                let entry = objects.entry(receipt.sha256).or_insert_with(|| (receipt.bytes, BTreeSet::new()));
                entry.1.insert(record.comic_id.clone());
            }
        }
        Ok(removable_object_bytes(&objects, &self.download_ui.selected))
    }

    pub(super) fn download_action(&mut self, action: &str, value: &str) -> Result<()> {
        match action {
            "enqueue" => self.enqueue_episode(value, true)?,
            "enqueue-prefetch" => self.enqueue_episode(value, false)?,
            "select" => { self.download_ui.selecting = true; self.download_ui.selected.clear(); }
            "cancel-select" => self.download_ui.close_overlays(),
            "toggle-item" if self.download_ui.selecting => {
                if !self.download_ui.selected.remove(value) { self.download_ui.selected.insert(value.to_owned()); }
            }
            "open" if !self.download_ui.selecting => {
                self.download_ui.sheet = Some(value.into());
                self.download_ui.chapter_page = 0;
            }
            "sheet-close" => { self.download_ui.sheet = None; self.download_ui.chapter_page = 0; }
            "page" if self.download_ui.sheet.is_none() => {
                self.download_ui.page = self.download_ui.page.saturating_add_signed(value.parse::<isize>().unwrap_or_default());
            }
            "chapter-page" if self.download_ui.sheet.is_some() => {
                self.download_ui.chapter_page = self.download_ui.chapter_page.saturating_add_signed(value.parse::<isize>().unwrap_or_default());
            }
            "store" => self.navigate("store"),
            "smart-settings" => { self.navigate("account"); self.account_action("section", "download")?; }
            "toggle-all" => {
                let rows = self.download_rows()?;
                let (has_controls, all_paused) = global_download_controls(&rows);
                if !has_controls { return Ok(()); }
                let pause = !all_paused;
                if !pause && (!self.require_network("downloads", "toggle-all", "") || !self.require_session()) { return Ok(()); }
                if self.demo.is_some() {
                    for row in rows.iter().filter(|row| row.active || row.queued || row.paused) {
                        if pause { self.download_ui.demo_paused.insert(row.id.clone()); }
                        else { self.download_ui.demo_paused.remove(&row.id); }
                    }
                } else {
                    for record in self.store.downloads(&self.account)?.iter().filter(|record| record.retained) {
                        if pause && matches!(record.status, DownloadStatus::Running | DownloadStatus::Queued) { self.apply_download_control(&record.id, "pause")?; }
                        else if !pause && record.status == DownloadStatus::Paused { self.apply_download_control(&record.id, "resume")?; }
                    }
                    for intent in self.download_ui.deferred.iter_mut().filter(|intent| intent.retain) { intent.paused = pause; }
                    self.store.set_preference(&self.account, "queued_download_intents", &self.download_ui.deferred)?;
                    self.flush_deferred_downloads()?;
                }
            }
            "retry-item" => self.retry_download_group(value)?,
            "task" => {
                let Some(id) = self.download_ui.sheet.clone() else { return Ok(()); };
                if value == "storage" {
                    self.navigate("account"); self.account_action("section", "storage")?;
                } else if value == "retry" {
                    self.retry_download_group(&id)?;
                } else if value != "resume" || self.require_network("downloads", "task", value) && self.require_session() {
                    self.control_download_group(&id, value)?;
                    if value == "cancel-queued" { self.download_ui.sheet = None; }
                }
            }
            "cancel-current" => {
                if let Some(id) = self.download_ui.sheet.take() { self.control_download_group(&id, "cancel")?; }
            }
            "remove-current" => {
                if let Some(id) = self.download_ui.sheet.take() { self.remove_download_groups(vec![id])?; }
            }
            "remove-selected" if self.download_ui.selecting && !self.download_ui.selected.is_empty() => {
                let ids = self.download_ui.selected.iter().cloned().collect();
                self.remove_download_groups(ids)?;
                self.download_ui.close_overlays();
            }
            "read" => {
                if let Some(id) = self.download_ui.sheet.clone() { self.read_download_group(&id)?; }
            }
            _ => {}
        }
        Ok(())
    }

    fn retry_download_group(&mut self, comic_id: &str) -> Result<()> {
        if self.require_network("downloads", "retry-item", comic_id) && self.require_session() {
            self.control_download_group(comic_id, "retry")?;
        }
        Ok(())
    }

    fn control_download_group(&mut self, comic_id: &str, action: &str) -> Result<()> {
        if let Some(demo) = &self.demo {
            let seed = demo.fixture["downloads"].as_array().into_iter().flatten()
                .find(|seed| seed["comic_id"].as_u64().is_some_and(|id| id.to_string() == comic_id));
            match action {
                "pause" => { self.download_ui.demo_paused.insert(comic_id.into()); }
                "resume" => { self.download_ui.demo_paused.remove(comic_id); }
                "retry" if seed.is_some_and(|seed| self.demo_seed_status(comic_id, seed) == DownloadStatus::Failed) => {
                    self.download_ui.demo_retried.insert(comic_id.into());
                }
                "priority" => {
                    self.download_ui.demo_priority_sequence = self.download_ui.demo_priority_sequence.checked_add(1).context("Preview priority overflow")?;
                    self.download_ui.demo_priority.insert(comic_id.into(), self.download_ui.demo_priority_sequence);
                }
                "cancel" | "cancel-queued" => {
                    if seed.is_some_and(|seed| seed["kind"].as_str() != Some("done")) {
                        self.download_ui.demo_cancelled.insert(comic_id.into());
                    }
                    self.download_ui.demo_enqueued.remove(comic_id);
                    self.download_ui.demo_retried.remove(comic_id);
                    self.download_ui.demo_paused.remove(comic_id);
                    self.download_ui.demo_priority.remove(comic_id);
                }
                _ => {}
            }
            return Ok(());
        }
        for record in self.store.downloads(&self.account)?.iter().filter(|record| record.comic_id == comic_id && record.retained) {
            let eligible = match action {
                "pause" => matches!(record.status, DownloadStatus::Running | DownloadStatus::Queued),
                "resume" => record.status == DownloadStatus::Paused,
                "retry" => record.status == DownloadStatus::Failed,
                "priority" => matches!(record.status, DownloadStatus::Queued | DownloadStatus::Running),
                "cancel-queued" => record.status == DownloadStatus::Queued,
                "cancel" => matches!(record.status, DownloadStatus::Queued | DownloadStatus::Running | DownloadStatus::Paused | DownloadStatus::Failed),
                _ => false,
            };
            if eligible { self.apply_download_control(&record.id, action)?; }
        }
        if action == "priority" {
            let next = self.download_ui.deferred_priorities.values().copied().max().unwrap_or_default()
                .checked_add(1).context("Deferred priority overflow")?;
            for intent in self.download_ui.deferred.iter().filter(|intent| intent.comic_id == comic_id && intent.retain && !intent.paused) {
                self.download_ui.deferred_priorities.insert(intent.episode_id.clone(), next);
            }
            self.store.set_preference(&self.account, "queued_download_priorities", &self.download_ui.deferred_priorities)?;
        } else if matches!(action, "cancel" | "cancel-queued") {
            let priorities = &mut self.download_ui.deferred_priorities;
            self.download_ui.deferred.retain(|intent| {
                let cancel = intent.comic_id == comic_id && intent.retain && (action == "cancel" || !intent.paused);
                if cancel { priorities.remove(&intent.episode_id); }
                !cancel
            });
            self.store.set_preference(&self.account, "queued_download_priorities", &self.download_ui.deferred_priorities)?;
        } else if matches!(action, "pause" | "resume") {
            for intent in self.download_ui.deferred.iter_mut().filter(|intent| intent.comic_id == comic_id && intent.retain) {
                intent.paused = action == "pause";
            }
        }
        self.store.set_preference(&self.account, "queued_download_intents", &self.download_ui.deferred)?;
        self.flush_deferred_downloads()
    }

    fn read_download_group(&mut self, comic_id: &str) -> Result<()> {
        if self.demo.is_some() {
            if !self.download_rows()?.iter().any(|row| row.id == comic_id && row.can_read) { return Ok(()); }
            let current = self.first_demo_offline_episode(comic_id)?;
            if let Some(episode) = current { self.open_episode(comic_id, &episode)?; }
            else { self.open_reader_for_comic(comic_id)?; }
            return Ok(());
        }
        let Some(detail) = self.store.load_detail(&self.account, comic_id)? else {
            self.toast("common_no_download"); return Ok(());
        };
        let anchor = self.store.load_anchor(&self.account, comic_id)?;
        let source = self.store.downloads(&self.account)?;
        let complete = |episode: &Episode| source.iter().find(|record| record.comic_id == comic_id
            && record.episode_id == episode.id && record.retained && record.status == DownloadStatus::Completed);
        let readable = now();
        let current = anchor.as_ref().and_then(|anchor| {
            let episode = detail.episodes.iter().find(|episode| episode.id == anchor.episode_id && episode.readable_at(readable))?;
            source.iter().find(|record| record.comic_id == comic_id && record.episode_id == episode.id && record.revision == anchor.revision
                && record.retained && record.status == DownloadStatus::Completed).or_else(|| complete(episode))
        });
        let selected = current.or_else(|| detail.episodes.iter().filter(|episode| episode.readable_at(readable))
            .filter_map(|episode| complete(episode).map(|record| (episode.order, record)))
            .min_by(|left, right| left.0.total_cmp(&right.0)).map(|(_, record)| record));
        if let Some(record) = selected { self.open_offline_episode(comic_id, &record.episode_id, &record.revision)?; }
        else { self.toast("common_no_download"); }
        Ok(())
    }

    fn first_demo_offline_episode(&self, comic_id: &str) -> Result<Option<String>> {
        let Some(detail) = self.store.load_detail(&self.account, comic_id)? else { return Ok(None); };
        let anchor = self.store.load_anchor(&self.account, comic_id)?;
        let available = |episode: &&Episode| episode.readable_at(now()) && self.reader_episode_downloaded(episode);
        let preferred = anchor.as_ref().and_then(|anchor| detail.episodes.iter().find(|episode| episode.id == anchor.episode_id))
            .filter(available).or_else(|| detail.episodes.iter().find(available));
        Ok(preferred.map(|episode| episode.id.clone()))
    }

    fn remove_download_groups(&mut self, ids: Vec<String>) -> Result<()> {
        if self.is_suspended() { return Ok(()); }
        if self.demo.is_some() {
            for id in ids {
                self.control_download_group(&id, "cancel")?;
                self.download_ui.demo_removed.insert(id);
            }
            return Ok(());
        }
        let records = self.store.downloads(&self.account)?.into_iter().filter(|record| ids.contains(&record.comic_id)).collect::<Vec<_>>();
        let priorities = &mut self.download_ui.deferred_priorities;
        self.download_ui.deferred.retain(|intent| {
            if ids.contains(&intent.comic_id) { priorities.remove(&intent.episode_id); false } else { true }
        });
        self.store.set_preference(&self.account, "queued_download_intents", &self.download_ui.deferred)?;
        self.store.set_preference(&self.account, "queued_download_priorities", &self.download_ui.deferred_priorities)?;
        if self.downloads.is_none() { self.toast("dl_background_busy"); return Ok(()); }
        let admission = match self.begin_job("download-remove", false) {
            Ok(admission) => admission,
            Err(_) => { self.toast("dl_background_busy"); return Ok(()); }
        };
        let manager = self.downloads.take().expect("Admission preserves the download manager");
        let owner = std::sync::Arc::new(std::sync::Mutex::new(Some(manager)));
        let work_owner = owner.clone();
        self.background_reserved("download-remove", admission, move || {
            let mut ownership = work_owner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let manager = ownership.as_mut().context("Download manager ownership is unavailable")?;
            let mut error = None;
            let mut seen = BTreeSet::new();
            for record in records {
                if seen.insert((record.episode_id.clone(), record.revision.clone())) {
                    if let Err(failure) = manager.remove_episode(&record.episode_id, &record.revision) { error = Some(failure); break; }
                }
            }
            let failure = error.map(|error| error.to_string() == "episode is in use by a reader");
            Ok(failure)
        }, move |app, result| {
            let manager = owner.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
            app.invalidate_catalog_local_state();
            app.invalidate_account_storage();
            if let Some(manager) = manager {
                if app.restore_download_manager(manager).is_err() { app.toast("common_storage_error"); }
            } else {
                app.prepare_download_manager();
            }
            match result {
                Ok(Some(true)) => app.toast("dl_reading_in_use"),
                Ok(Some(false)) | Err(_) => app.toast("common_storage_error"),
                Ok(None) => {}
            }
        });
        Ok(())
    }

    fn apply_download_control(&mut self, task: &str, action: &str) -> Result<()> {
        if action != "priority" && !self.is_suspended() {
            if let Some(snapshot) = self.download_ui.sleep.as_mut() { snapshot.tasks.remove(task); }
        }
        self.apply_download_control_raw(task, action)
    }

    fn apply_download_control_raw(&mut self, task: &str, action: &str) -> Result<()> {
        let needs_network = matches!(action, "resume" | "retry");
        if needs_network && (self.is_suspended() || self.options.offline
            || self.client.credential_context().map(|(account, _)| account) != Some(self.account.clone()))
        { return Ok(()); }
        if let Some(manager) = &mut self.downloads {
            match action {
                "pause" => manager.pause(task)?,
                "cancel" | "cancel-queued" => manager.cancel_unfinished_retained(task, action == "cancel-queued")?,
                "resume" => { manager.resume(task)?; }, "retry" => { manager.retry(task)?; },
                "priority" => manager.prioritize(task)?, _ => {}
            }
        } else {
            if matches!(action, "cancel" | "cancel-queued") {
                // A borrowed manager may still be publishing a completed chapter. Replay under its publication lock.
                if !self.download_ui.pending_controls.iter().any(|(id, pending)| id == task && pending == action) {
                    self.download_ui.pending_controls.push((task.to_owned(), action.to_owned()));
                }
                return Ok(());
            }
            if action == "priority" {
                self.download_ui.pending_controls.push((task.to_owned(), action.to_owned()));
                self.store.prioritize_download(&self.account, task)?;
                return Ok(());
            }
            let status = match action {
                "pause" => DownloadStatus::Paused,
                "resume" | "retry" => DownloadStatus::Queued, _ => return Ok(())
            };
            self.store.update_download_state(&self.account, task, status, None, now())?;
            self.download_ui.pending_controls.push((task.to_owned(), action.to_owned()));
        }
        Ok(())
    }

    pub(super) fn sleep_download_resume_pending(&self) -> bool {
        self.download_ui.sleep.as_ref().is_some_and(|snapshot| snapshot.belongs_to(&self.account, self.epoch))
    }

    pub(super) fn has_sleep_download_snapshot(&self, cycle: u64) -> bool {
        self.download_ui.sleep.as_ref().is_some_and(|snapshot| snapshot.cycle == cycle && snapshot.belongs_to(&self.account, self.epoch))
    }

    pub(super) fn release_download_wake_guard(&mut self) { self.download_ui.wake = None; }

    /// Prepare only RAM state on the control thread; the maintenance worker
    /// persists this snapshot without delaying a native resume acknowledgement.
    pub(super) fn prepare_sleep_deferred_checkpoint(&mut self, cycle: u64) -> Result<serde_json::Value> {
        anyhow::ensure!(self.is_suspended() && self.has_sleep_download_snapshot(cycle), "The sleep checkpoint scope is inactive");
        let newly_deferred = self.download_ui.deferred.iter().filter(|intent| !intent.paused).cloned().collect::<Vec<_>>();
        for intent in &mut self.download_ui.deferred { intent.paused = true; }
        if let Some(snapshot) = self.download_ui.sleep.as_mut() { snapshot.deferred.extend(newly_deferred); }
        if self.demo.is_some() {
            let active = self.download_rows()?.into_iter().filter(|row|
                (row.active || row.queued) && !row.paused && !row.error).map(|row| row.id).collect::<Vec<_>>();
            for id in active {
                self.download_ui.demo_paused.insert(id.clone());
                if let Some(snapshot) = self.download_ui.sleep.as_mut() { snapshot.demo_ids.insert(id); }
            }
        }
        serde_json::to_value(&self.download_ui.deferred).context("Prepare paused deferred downloads")
    }

    /// Install resume ownership before the worker can commit a paused row. If
    /// resume wins the race, its existing full checkpoint updates these tokens.
    pub(super) fn stage_sleep_task_checkpoints(&mut self, cycle: u64, records: &[DownloadRecord]) -> Result<()> {
        anyhow::ensure!(self.is_suspended() && self.has_sleep_download_snapshot(cycle), "The sleep checkpoint scope is inactive");
        let reader = self.reader_sleep_lease();
        for current in records.iter().filter(|record| active_for_sleep(record)) {
            let generation = current.generation.checked_add(1).context("Download generation overflow")?;
            let reader_generation = reader.as_ref().filter(|reader|
                reader.episode_id == current.episode_id && reader.revision == current.revision
                    && reader.task_id.as_deref() == Some(current.id.as_str())
                    && reader.task_generation == Some(current.generation)).map(|reader| reader.generation);
            if reader_generation.is_some() { self.hold_reader_prefetch_after_sleep(false); }
            if !self.download_ui.pending_controls.iter().any(|(task, action)| task == &current.id && action == "pause") {
                self.download_ui.pending_controls.push((current.id.clone(), "pause".into()));
            }
            if let Some(snapshot) = self.download_ui.sleep.as_mut() {
                snapshot.tasks.insert(current.id.clone(), SleepDownloadTask {
                    id: current.id.clone(), episode_id: current.episode_id.clone(), revision: current.revision.clone(),
                    paused_generation: generation, retained: current.retained, reader_generation, needs_control: true,
                });
            }
        }
        Ok(())
    }

    pub(super) fn pause_downloads_for_sleep(&mut self, cycle: u64) -> Result<()> {
        self.download_ui.wake = None;
        let new_cycle = self.download_ui.sleep.as_ref().is_none_or(|snapshot|
            snapshot.cycle != cycle || !snapshot.belongs_to(&self.account, self.epoch));
        if new_cycle {
            let carried = self.download_ui.sleep.take().filter(|snapshot|
                snapshot.resume_requested && snapshot.belongs_to(&self.account, self.epoch))
                .map_or_else(BTreeMap::new, |snapshot| snapshot.tasks);
            self.download_ui.sleep = Some(DownloadSleepSnapshot { cycle, account: self.account.clone(), epoch: self.epoch,
                resume_requested: false, tasks: carried, deferred: Vec::new(), demo_ids: BTreeSet::new() });
            if let Some(reader) = self.reader_sleep_lease() {
                let active = match reader.task_id.as_ref().map(|task| self.store.load_download(&self.account, task)).transpose() {
                    Ok(active) => active,
                    Err(error) => { self.hold_reader_prefetch_after_sleep(true); return Err(error); }
                };
                let carried = self.download_ui.sleep.as_ref().is_some_and(|snapshot|
                    reader.task_id.as_ref().is_some_and(|task| snapshot.tasks.contains_key(task)));
                let held = !carried && active.flatten().is_some_and(|record|
                    !matches!(record.status, DownloadStatus::Running | DownloadStatus::Queued));
                self.hold_reader_prefetch_after_sleep(held);
            }
        }
        let mut failed = false;
        let newly_deferred = self.download_ui.deferred.iter().filter(|intent| !intent.paused).cloned().collect::<Vec<_>>();
        for intent in &mut self.download_ui.deferred { intent.paused = true; }
        if let Some(snapshot) = self.download_ui.sleep.as_mut() { snapshot.deferred.extend(newly_deferred); }
        if self.store.set_preference(&self.account, "queued_download_intents", &self.download_ui.deferred).is_err() { failed = true; }
        if self.demo.is_some() {
            let active = self.download_rows()?.into_iter().filter(|row| (row.active || row.queued) && !row.paused && !row.error).map(|row| row.id).collect::<Vec<_>>();
            for id in active {
                self.download_ui.demo_paused.insert(id.clone());
                if let Some(snapshot) = self.download_ui.sleep.as_mut() { snapshot.demo_ids.insert(id); }
            }
        }
        let reader = self.reader_sleep_lease();
        let records = self.store.downloads(&self.account)?;
        for record in records.into_iter().filter(active_for_sleep) {
            let Some(current) = self.store.load_download(&self.account, &record.id)? else { continue; };
            if current.generation != record.generation || !matches!(current.status, DownloadStatus::Running | DownloadStatus::Queued) { continue; }
            let reader_generation = reader.as_ref().filter(|reader| reader.episode_id == current.episode_id
                && reader.revision == current.revision && reader.task_id.as_deref() == Some(current.id.as_str())
                && reader.task_generation == Some(current.generation)).map(|reader| reader.generation);
            let missing_manager = self.downloads.is_none();
            let paused = if let Some(manager) = self.downloads.as_mut() { manager.pause(&current.id) }
                else {
                    let mut paused = current.clone();
                    paused.generation = paused.generation.checked_add(1).context("Download generation overflow")?;
                    paused.status = DownloadStatus::Paused;
                    paused.error_code = None;
                    paused.updated_at = now();
                    self.store.save_download(&self.account, &paused)
                };
            if paused.is_err() { failed = true; continue; }
            let Some(paused) = self.store.load_download(&self.account, &current.id)? else { failed = true; continue; };
            if paused.status != DownloadStatus::Paused { continue; }
            if missing_manager { self.download_ui.pending_controls.push((paused.id.clone(), "pause".into())); }
            else { self.download_ui.pending_controls.retain(|(task, action)| task != &paused.id || action != "pause"); }
            if let Some(snapshot) = self.download_ui.sleep.as_mut() {
                snapshot.tasks.insert(paused.id.clone(), SleepDownloadTask { id: paused.id, episode_id: paused.episode_id,
                    revision: paused.revision, paused_generation: paused.generation, retained: paused.retained,
                    reader_generation, needs_control: missing_manager });
            }
        }
        anyhow::ensure!(!failed, "One or more download pause checkpoints failed");
        Ok(())
    }

    fn reconcile_download_controls_after_sleep(&mut self) -> Result<()> {
        if self.downloads.is_none() { return Ok(()); }
        let pending = std::mem::take(&mut self.download_ui.pending_controls);
        let controls = if self.download_ui.sleep.is_some() {
            let mut last = BTreeMap::new(); let mut priorities = BTreeSet::new();
            for (task, action) in pending {
                if action == "priority" { priorities.insert(task); }
                else { last.insert(task, action); }
            }
            let mut controls = last.into_iter().collect::<Vec<_>>();
            controls.extend(priorities.into_iter().map(|task| (task, "priority".into())));
            controls
        } else { pending };
        let (deferred, result) = replay_download_controls(controls, |task, action| {
            if matches!(action, "resume" | "retry") && self.is_suspended() { return Ok(false); }
            if action == "priority" {
                if self.is_suspended() { return Ok(false); }
                let Some(current) = self.store.load_download(&self.account, task)? else { return Ok(true); };
                if current.status == DownloadStatus::Paused && self.download_ui.sleep.as_ref()
                    .filter(|snapshot| snapshot.belongs_to(&self.account, self.epoch))
                    .is_some_and(|snapshot| snapshot.tasks.contains_key(task)) { return Ok(false); }
                if !matches!(current.status, DownloadStatus::Queued | DownloadStatus::Running) || !current.retained { return Ok(true); }
            }
            let owned_pause = self.download_ui.sleep.as_ref().filter(|snapshot| snapshot.belongs_to(&self.account, self.epoch))
                .and_then(|snapshot| snapshot.tasks.get(task)).filter(|token| token.needs_control).cloned();
            if let Some(token) = owned_pause.filter(|_| action == "pause") {
                let current = self.store.load_download(&self.account, task)?;
                if !current.as_ref().is_some_and(|record| record.status == DownloadStatus::Paused
                    && record.generation == token.paused_generation && record.revision == token.revision && record.episode_id == token.episode_id) {
                    if let Some(snapshot) = self.download_ui.sleep.as_mut() { snapshot.tasks.remove(task); }
                    return Ok(true);
                }
                let next_generation = token.paused_generation.checked_add(1).context("Download generation overflow")?;
                self.apply_download_control_raw(task, "pause")?;
                // pause() commits this exact increment before returning. Do not
                // make durable control acknowledgement depend on another read.
                if let Some(token) = self.download_ui.sleep.as_mut().and_then(|snapshot| snapshot.tasks.get_mut(task)) {
                    token.paused_generation = next_generation;
                    token.needs_control = false;
                }
            } else { self.apply_download_control_raw(task, action)?; }
            Ok(true)
        });
        self.download_ui.pending_controls.extend(deferred);
        if result.is_err() { self.mark_lifecycle_checkpoint_error(); }
        result
    }

    pub(super) fn resume_downloads_from_sleep(&mut self) -> Result<()> {
        if self.is_suspended() { return Ok(()); }
        let Some(snapshot) = self.download_ui.sleep.as_ref() else { return Ok(()); };
        if !snapshot.belongs_to(&self.account, self.epoch) {
            self.download_ui.sleep = None;
            return Ok(());
        }
        if let Some(snapshot) = self.download_ui.sleep.as_mut() { snapshot.resume_requested = true; }
        let deferred = self.download_ui.sleep.as_mut().map(|snapshot| std::mem::take(&mut snapshot.deferred)).unwrap_or_default();
        for saved in deferred {
            if let Some(intent) = self.download_ui.deferred.iter_mut().find(|intent| intent.paused && same_deferred_intent(intent, &saved)) { intent.paused = false; }
        }
        let demo_ids = self.download_ui.sleep.as_mut().map(|snapshot| std::mem::take(&mut snapshot.demo_ids)).unwrap_or_default();
        for id in demo_ids { self.download_ui.demo_paused.remove(&id); }
        self.store.set_preference(&self.account, "queued_download_intents", &self.download_ui.deferred)?;
        if self.downloads.is_none() {
            if self.demo.is_some() { self.download_ui.sleep = None; }
            return Ok(());
        }
        self.reconcile_download_controls_after_sleep()?;
        if self.options.offline || self.client.credential_context().map(|(account, _)| account) != Some(self.account.clone()) { return Ok(()); }
        let tokens = self.download_ui.sleep.as_ref().map(|snapshot| snapshot.tasks.values().cloned().collect::<Vec<_>>()).unwrap_or_default();
        let reader = self.reader_sleep_lease();
        let mut failed = false;
        for token in tokens {
            if token.needs_control { failed = true; continue; }
            let record = self.store.load_download(&self.account, &token.id)?;
            let eligible = record.as_ref().is_some_and(|record| token.can_resume(record, reader.as_ref()));
            if !eligible {
                if let Some(snapshot) = self.download_ui.sleep.as_mut() { snapshot.tasks.remove(&token.id); }
                continue;
            }
            let resumed = self.downloads.as_mut().context("Download manager unavailable")?.resume(&token.id);
            match resumed {
                Ok(record) => {
                    self.reader_task_resumed(&record);
                    if let Some(snapshot) = self.download_ui.sleep.as_mut() { snapshot.tasks.remove(&token.id); }
                }
                Err(_) => failed = true,
            }
        }
        if self.download_ui.sleep.as_ref().is_some_and(|snapshot| snapshot.tasks.is_empty() && snapshot.deferred.is_empty() && snapshot.demo_ids.is_empty()) {
            self.download_ui.sleep = None;
        }
        self.flush_deferred_downloads()?;
        anyhow::ensure!(!failed, "One or more suspended downloads could not resume");
        Ok(())
    }
}

fn same_deferred_intent(left: &DeferredDownload, right: &DeferredDownload) -> bool {
    left.comic_id == right.comic_id && left.episode_id == right.episode_id && left.title == right.title
        && left.comic_title == right.comic_title && left.revision == right.revision && left.retain == right.retain
}

fn active_for_sleep(record: &DownloadRecord) -> bool {
    matches!(record.status, DownloadStatus::Running | DownloadStatus::Queued)
}

fn replay_download_controls<F>(controls: Vec<(String, String)>, mut apply: F) -> (Vec<(String, String)>, Result<()>)
where F: FnMut(&str, &str) -> Result<bool> {
    let mut remaining = controls.into_iter();
    let mut deferred = Vec::new();
    while let Some((task, action)) = remaining.next() {
        match apply(&task, &action) {
            Ok(true) => {},
            Ok(false) => deferred.push((task, action)),
            Err(error) => {
                deferred.push((task, action));
                deferred.extend(remaining);
                return (deferred, Err(error));
            }
        }
    }
    (deferred, Ok(()))
}

fn reset_download_sheet(model: &DownloadModel<'_>) {
    if !model.get_sheet_open() && model.get_sheet_id().is_empty() { return; }
    model.set_sheet_open(false);
    model.set_sheet_id("".into());
    model.set_chapter_page(1);
    model.set_chapter_page_count(1);
    model.set_chapters(ModelRc::default());
    model.set_task_visible(false);
    model.set_task_primary_action("".into());
    model.set_task_secondary_action("".into());
    model.set_can_read(false);
}

// Keep the portrait limits while fitting complete rows in smaller or landscape viewports.
fn download_list_capacity(scribe: bool, height: u32, groups: usize) -> usize {
    let (insets, row, pager, limit) = if scribe { (32 + 80 + 72 + 64, 108, 64 + 12, 9) }
        else { (28 + 64 + 56 + 48, 80, 52 + 8, 8) };
    let available = height.saturating_sub(insets);
    let unpaged = (available / row).clamp(1, limit) as usize;
    if groups <= unpaged { unpaged }
    else { (available.saturating_sub(pager) / row).clamp(1, limit - 1) as usize }
}

fn download_chapter_capacity(scribe: bool, height: u32, has_task: bool) -> usize {
    let (status, panel, fixed, row, task, limit) = if scribe { (32, 840, 176 + 56 + 98, 64, 202 + 16, 7) }
        else { (28, 612, 120 + 44 + 78, 52, 149 + 12, 6) };
    let available = height.saturating_sub(status).min(panel).saturating_sub(fixed + if has_task { task } else { 0 });
    (available / row).clamp(1, if has_task { 4 } else { limit }) as usize
}

fn demo_storage_bytes(seed: &serde_json::Value) -> u64 {
    seed["downloaded_mb"].as_u64().unwrap_or_default().saturating_mul(1024 * 1024)
}

fn demo_episode_id(comic: u64, order: u64) -> String { format!("{comic}{order:02}") }

fn demo_page_counts(seed: &serde_json::Value) -> (u64, u64) {
    (seed["saved_pages"].as_u64().unwrap_or_default(), seed["total_pages"].as_u64().unwrap_or_default())
}

fn demo_download_chapters(seed: &serde_json::Value) -> (Vec<DownloadChapterItem>, usize) {
    let comic = seed["comic_id"].as_u64().unwrap_or_default();
    if let Some(count) = seed["chapter_count"].as_u64().filter(|count| *count > 0) {
        let bytes_per_chapter = demo_storage_bytes(seed) / count;
        let size = format!("{} MB", (bytes_per_chapter as f64 / (1024.0 * 1024.0)).round() as u64);
        let mut chapters = Vec::new();
        let titles = seed["chapter_titles"].as_array().filter(|titles| !titles.is_empty());
        for order in 1..=count {
            let title = (order == count).then(|| seed["last_chapter_title"].as_str()).flatten()
                .or_else(|| titles.and_then(|titles| titles[(order as usize - 1) % titles.len()].as_str())).unwrap_or_default();
            chapters.push(DownloadChapterItem { id: demo_episode_id(comic, order).into(), number: episode_number(order as f64).into(),
                title: title.into(), size: size.clone().into() });
        }
        return (chapters, count as usize);
    }
    let mut chapters = Vec::new();
    let mut complete = 0;
    for chapter in seed["chapter_rows"].as_array().into_iter().flatten() {
        let Some(order) = chapter["order"].as_u64() else { continue; };
        let size = if let Some(mb) = chapter["mb"].as_u64() {
            complete += 1;
            format!("{mb} MB")
        } else {
            let (saved, total) = demo_page_counts(chapter);
            if saved == 0 { continue; }
            page_count_label("dl_page_count", saved, total)
        };
        chapters.push(DownloadChapterItem { id: demo_episode_id(comic, order).into(), number: episode_number(order as f64).into(),
            title: chapter["title"].as_str().unwrap_or_default().into(), size: size.into() });
    }
    (chapters, complete)
}

fn local_chapter_rank(record: &DownloadRecord) -> (bool, i64, u32) {
    (record.status == DownloadStatus::Completed, record.updated_at, record.completed_pages)
}

fn download_summary(count: usize, size: String, has_task: bool) -> String {
    if count == 0 { i18n::text("dl_no_complete_episodes") }
    else { i18n::format(if has_task { "dl_completed_summary" } else { "dl_chapter_summary" }, &[("count", count.to_string()), ("size", size)]) }
}

fn episode_number(order: f64) -> String { i18n::format("dl_episode_number", &[("number", display_order(order))]) }

fn download_episode_scope(mut orders: Vec<f64>) -> String {
    orders.retain(|order| order.is_finite());
    orders.sort_by(f64::total_cmp);
    orders.dedup();
    match orders.as_slice() {
        [] => i18n::text("dl_current_episode"),
        [order] => episode_number(*order),
        _ if orders.windows(2).all(|pair| pair[1] - pair[0] == 1.0) => i18n::format("dl_episode_range", &[
            ("first", display_order(orders[0])), ("last", display_order(orders[orders.len() - 1])),
        ]),
        _ => i18n::format("dl_episode_number", &[("number", orders.into_iter().map(display_order).collect::<Vec<_>>().join(&i18n::text("dl_episode_separator")))]),
    }
}

fn page_count_label(key: &str, saved: u64, total: u64) -> String {
    i18n::format(key, &[("count", saved.to_string()), ("total", if total > 0 { total.to_string() } else { i18n::text("common_unknown") })])
}

fn row_priority(row: &DownloadRow) -> u8 { if row.error { 0 } else if row.active { 1 } else if row.queued { 2 } else if row.paused { 3 } else { 4 } }

fn is_prioritized(row: &DownloadRow, rows: &[DownloadRow]) -> bool {
    row.priority > 0 && !rows.iter().any(|other| other.queued && !other.error && !other.active && other.priority > row.priority)
}

fn sort_download_rows(rows: &mut [DownloadRow]) {
    rows.sort_by(|left, right| row_priority(left).cmp(&row_priority(right)).then_with(|| right.priority.cmp(&left.priority)));
}

fn global_download_controls(rows: &[DownloadRow]) -> (bool, bool) {
    let active = rows.iter().any(|row| row.active || row.queued);
    let paused = rows.iter().any(|row| row.paused);
    (active || paused, !active && paused)
}

fn removable_object_bytes(objects: &BTreeMap<String, (u64, BTreeSet<String>)>, selected: &BTreeSet<String>) -> u64 {
    objects.values().filter(|(_, owners)| !owners.is_empty() && owners.iter().all(|owner| selected.contains(owner)))
        .fold(0_u64, |total, (bytes, _)| total.saturating_add(*bytes))
}

fn download_failure_message(code: &str) -> String {
    if code.starts_with("episode_") { i18n::text("dl_failure_access") }
    else if code.contains("unsupported") || code.contains("limit") || code.contains("too_large") { i18n::text("dl_failure_image_capability") }
    else if ["image_", "png_", "jpeg_", "webp_", "gif_", "page_geometry", "page_digest"].iter().any(|prefix| code.starts_with(prefix)) { i18n::text("dl_failure_integrity") }
    else if ["storage", "disk", "space", "write", "commit"].iter().any(|part| code.contains(part)) { i18n::text("dl_failure_storage") }
    else if ["authentication", "session", "account"].iter().any(|part| code.contains(part)) { i18n::text("common_session_expired") }
    else if ["network", "timeout", "http"].iter().any(|part| code.contains(part)) { i18n::text("common_network_error") }
    else { i18n::text("common_download_failed") }
}

pub(super) fn format_design_bytes(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB { format!("{:.2} GB", bytes as f64 / GIB as f64) }
    else { format!("{} MB", bytes / MIB) }
}

pub(super) fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 { format!("{:.2} GB", bytes as f64 / 1_000_000_000.0) }
    else if bytes >= 1_000_000 { format!("{:.1} MB", bytes as f64 / 1_000_000.0) }
    else if bytes >= 1000 { format!("{:.0} KB", bytes as f64 / 1000.0) }
    else { format!("{bytes} B") }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    fn token(retained: bool) -> SleepDownloadTask {
        SleepDownloadTask { id: "sleep-task".into(), episode_id: "episode-12".into(), revision: "revision-one".into(),
            paused_generation: 8, retained, reader_generation: (!retained).then_some(41), needs_control: false }
    }

    fn paused_record(retained: bool) -> DownloadRecord {
        DownloadRecord { id: "sleep-task".into(), episode_id: "episode-12".into(), revision: "revision-one".into(),
            generation: 8, status: DownloadStatus::Paused, retained, ..Default::default() }
    }

    #[test]
    fn sleep_snapshot_excludes_existing_user_pauses_failures_cancellations_and_completion() {
        for status in [DownloadStatus::Paused, DownloadStatus::Failed, DownloadStatus::Cancelled, DownloadStatus::Completed] {
            assert!(!active_for_sleep(&DownloadRecord { status, ..Default::default() }));
        }
        for status in [DownloadStatus::Queued, DownloadStatus::Running] {
            assert!(active_for_sleep(&DownloadRecord { status, ..Default::default() }));
        }
    }

    #[test]
    fn borrowed_manager_pause_requires_reconciled_generation_before_resume() {
        let mut token = token(true);
        let mut record = paused_record(true);
        token.needs_control = true;
        assert!(!token.can_resume(&record, None));
        // The returned manager synchronizes its watch control with this pause.
        record.generation += 1;
        token.paused_generation = record.generation;
        token.needs_control = false;
        assert!(token.can_resume(&record, None));
        record.generation += 1;
        assert!(!token.can_resume(&record, None));
        record.generation = token.paused_generation;
        record.status = DownloadStatus::Failed;
        assert!(!token.can_resume(&record, None));
    }

    #[test]
    fn sleep_resume_is_bound_to_account_epoch_and_current_reader_identity() {
        let snapshot = DownloadSleepSnapshot { cycle: 3, account: "fixture-account".into(), epoch: 5,
            resume_requested: false, tasks: BTreeMap::new(), deferred: Vec::new(), demo_ids: BTreeSet::new() };
        assert!(snapshot.belongs_to("fixture-account", 5));
        assert!(!snapshot.belongs_to("another-account", 5));
        assert!(!snapshot.belongs_to("fixture-account", 6));
        let token = token(false);
        let record = paused_record(false);
        let mut reader = super::super::reading::ReaderSleepLease { generation: 41, episode_id: "episode-12".into(),
            revision: "revision-one".into(), task_id: Some("sleep-task".into()), task_generation: Some(7) };
        assert!(token.can_resume(&record, Some(&reader)));
        reader.generation = 42;
        assert!(!token.can_resume(&record, Some(&reader)));
        reader.generation = 41;
        reader.revision = "new-revision".into();
        assert!(!token.can_resume(&record, Some(&reader)));
        assert!(!token.can_resume(&record, None));
    }

    #[test]
    fn deferred_sleep_intent_does_not_match_another_episode_or_changed_scope() {
        let original = DeferredDownload { comic_id: "comic".into(), episode_id: "episode-12".into(), title: "Chapter".into(),
            comic_title: "Comic".into(), revision: Some("revision-one".into()), retain: true, paused: false };
        let mut paused = original.clone();
        paused.paused = true;
        assert!(same_deferred_intent(&original, &paused));
        paused.episode_id = "episode-13".into();
        assert!(!same_deferred_intent(&original, &paused));
        paused = original.clone(); paused.retain = false;
        assert!(!same_deferred_intent(&original, &paused));
    }

    #[test]
    fn retained_deferred_requests_refresh_the_index_while_reader_requests_keep_their_revision() {
        let mut intent = DeferredDownload { comic_id: "comic".into(), episode_id: "episode-12".into(), title: "Chapter".into(),
            comic_title: "Comic".into(), revision: Some("previous-observed-revision".into()), retain: true, paused: false };
        let retained = intent.request();
        assert!(retained.retain);
        assert!(retained.expected_revision.is_none());
        assert!(retained.page_indices.is_none());
        intent.retain = false;
        let reader = intent.request();
        assert!(!reader.retain);
        assert_eq!(reader.expected_revision.as_deref(), Some("previous-observed-revision"));
    }

    #[test]
    fn failed_pending_control_preserves_itself_and_later_intents_for_retry() {
        let controls = vec![("first".into(), "pause".into()), ("blocked".into(), "pause".into()), ("last".into(), "cancel".into())];
        let mut applied = Vec::new();
        let (pending, result) = replay_download_controls(controls, |task, _| {
            if task == "blocked" { anyhow::bail!("fixture storage failure"); }
            applied.push(task.to_owned());
            Ok(true)
        });
        assert!(result.is_err());
        assert_eq!(applied, vec!["first"]);
        assert_eq!(pending, vec![("blocked".into(), "pause".into()), ("last".into(), "cancel".into())]);
        let (remaining, result) = replay_download_controls(pending, |task, _| { applied.push(task.to_owned()); Ok(true) });
        assert!(result.is_ok() && remaining.is_empty());
        assert_eq!(applied, vec!["first", "blocked", "last"]);
    }

    #[test]
    fn queue_badge_counts_retained_comics_including_deferred_work() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Store::open(directory.path().join("state.sqlite3"))?;
        let account = "fixture-account";
        for (id, comic, status, retained) in [
            ("queued-1", "comic-1", DownloadStatus::Queued, true),
            ("queued-2", "comic-1", DownloadStatus::Queued, true),
            ("reader-task", "comic-2", DownloadStatus::Running, false),
            ("paused-task", "comic-3", DownloadStatus::Paused, true),
        ] {
            store.save_download(account, &DownloadRecord { id: id.into(), comic_id: comic.into(), episode_id: id.into(),
                revision: "revision-1".into(), status, retained, ..Default::default() })?;
        }
        assert_eq!(store.download_queue_activity(account, &["comic-1", "comic-4", "comic-4"])?, (true, 2));
        assert!(store.episode_download_pending(account, "paused-task")?);
        assert!(!store.episode_download_pending(account, "reader-task")?);
        Ok(())
    }

    #[test]
    fn list_pagination_leaves_room_for_the_navigation_gap_and_whole_rows() {
        assert_eq!(download_list_capacity(false, 840, 8), 8);
        assert_eq!(download_list_capacity(false, 840, 9), 7);
        assert_eq!(download_list_capacity(true, 1240, 9), 9);
        assert_eq!(download_list_capacity(true, 1240, 10), 8);
        assert_eq!(download_list_capacity(false, 632, 8), 4);
    }

    #[test]
    fn chapter_capacity_reserves_the_task_card_and_footer() {
        assert_eq!(download_chapter_capacity(false, 840, false), 6);
        assert_eq!(download_chapter_capacity(false, 840, true), 4);
        assert_eq!(download_chapter_capacity(true, 1240, false), 7);
        assert_eq!(download_chapter_capacity(true, 1240, true), 4);
        assert_eq!(download_chapter_capacity(false, 632, true), 3);
    }

    #[test]
    fn all_paused_downloads_keep_a_visible_resume_control() {
        let rows = vec![DownloadRow { paused: true, ..Default::default() },
            DownloadRow { paused: true, ..Default::default() },
            DownloadRow { error: true, ..Default::default() }];
        assert_eq!(global_download_controls(&rows), (true, true));
        let mut mixed = rows;
        mixed[0].paused = false; mixed[0].queued = true;
        assert_eq!(global_download_controls(&mixed), (true, false));
        assert_eq!(global_download_controls(&[DownloadRow { error: true, ..Default::default() }]), (false, false));
    }

    #[test]
    fn queued_priority_changes_order_without_moving_ahead_of_failures_or_running_rows() {
        let mut rows = vec![DownloadRow { id: "queued-low".into(), queued: true, priority: 1, ..Default::default() },
            DownloadRow { id: "running".into(), active: true, ..Default::default() },
            DownloadRow { id: "queued-selected".into(), queued: true, priority: 4, ..Default::default() },
            DownloadRow { id: "failed".into(), error: true, ..Default::default() }];
        sort_download_rows(&mut rows);
        assert_eq!(rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(), vec!["failed", "running", "queued-selected", "queued-low"]);
    }

    #[test]
    fn selection_capacity_counts_a_shared_object_only_when_all_owners_are_selected() {
        let objects = BTreeMap::from([
            ("shared".into(), (200, BTreeSet::from(["first".into(), "second".into()]))),
            ("first-only".into(), (100, BTreeSet::from(["first".into()]))),
        ]);
        assert_eq!(removable_object_bytes(&objects, &BTreeSet::from(["first".into()])), 100);
        assert_eq!(removable_object_bytes(&objects, &BTreeSet::from(["first".into(), "second".into()])), 300);
    }
}

#[cfg(test)]
#[path = "lifecycle_download_checkpoint_tests.rs"]
mod maintenance_tests;

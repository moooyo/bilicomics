use super::*;
use crate::{
    downloads::{DownloadEventKind, DownloadRequest},
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
    selecting: bool,
    selected: BTreeSet<String>,
    sheet: Option<String>,
    demo_paused: BTreeSet<String>,
    demo_removed: BTreeSet<String>,
    demo_queued: BTreeSet<String>,
    demo_cancelled: BTreeSet<String>,
    demo_priority: BTreeMap<String, u64>,
    demo_priority_sequence: u64,
    wake: Option<WakeGuard>,
    free: String,
}

impl DownloadState {
    pub(super) fn set_free_space(&mut self, bytes: u64) { self.free = format_bytes(bytes); }
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
    can_read: bool,
    has_task: bool,
    priority: u64,
    progress: f32,
    error: bool,
    active: bool,
    queued: bool,
    paused: bool,
    bytes: u64,
    chapters: Vec<DownloadChapterItem>,
}

impl App {
    pub(super) fn recover_downloads(&mut self) {
        self.cache_download_pause_handle();
        if self.is_suspended() { self.set_download_transport_suspended(true); return; }
        let Some(manager) = self.downloads.as_mut() else { return; };
        // A newly created manager starts with its default capacity. Restore the
        // saved preference before recovery can admit any page acquisitions.
        if manager.set_concurrency(self.preferences.concurrency.clamp(1, 4) as usize).is_err() {
            self.toast("common_storage_error");
            return;
        }
        let admission = match self.begin_job("download-recover", false) {
            Ok(admission) => admission,
            Err(_) => { self.toast("dl_background_busy"); return; }
        };
        let manager = self.downloads.take().expect("Admission preserves the download manager");
        let owner = std::sync::Arc::new(std::sync::Mutex::new(Some(manager)));
        let work_owner = owner.clone();
        let resume = !self.is_suspended() && !self.options.offline && self.client.session().is_some();
        self.background_reserved("download-recover", admission, move || {
            let mut ownership = work_owner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let manager = ownership.as_mut().context("Download manager ownership is unavailable")?;
            let recovered = if resume { manager.recover() } else { manager.pause_all() };
            Ok(recovered.is_ok())
        }, move |app, result| {
            let manager = owner.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
            app.invalidate_catalog_local_state();
            if !matches!(result, Ok(true)) { app.toast("common_storage_error"); }
            if let Some(manager) = manager {
                if app.restore_download_manager(manager).is_err() { app.toast("common_storage_error"); }
            } else {
                app.prepare_download_manager();
            }
        });
    }

    pub(super) fn restore_download_manager(&mut self, manager: DownloadManager) -> Result<()> {
        // Settings may have changed while a background job owned the manager.
        manager.set_concurrency(self.preferences.concurrency.clamp(1, 4) as usize)?;
        self.downloads = Some(manager);
        self.cache_download_pause_handle();
        if self.is_suspended() { self.set_download_transport_suspended(true); }
        self.reconcile_download_controls_after_sleep()?;
        self.sync_reader_lease()?;
        if self.is_suspended() {
            self.pause_downloads_for_sleep(self.lifecycle_cycle())?;
            return Ok(());
        }
        self.resume_downloads_from_sleep()?;
        self.flush_deferred_downloads()?;
        self.reader_page_ready()?;
        Ok(())
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
        self.download_ui.free = i18n::text("common_unknown");
        if self.demo.is_some() {
            self.download_ui.free = i18n::text(if self.is_scribe() { "dl_fixture_free_sc" } else { "dl_fixture_free_pw" });
        }
        let (_, failed) = self.download_activity()?;
        self.project_download_badge(failed);
        Ok(())
    }

    fn download_activity(&self) -> Result<(bool, u32)> {
        let Some(demo) = &self.demo else { return self.store.download_activity(&self.account); };
        let failed = demo.fixture["downloads"].as_array().into_iter().flatten().filter(|value| {
            let id = value["comic_id"].as_u64().unwrap_or_default().to_string();
            value["kind"].as_str() == Some("error") && !self.download_ui.demo_removed.contains(&id)
                && !self.download_ui.demo_cancelled.contains(&id) && !self.download_ui.demo_queued.contains(&id)
        }).count();
        Ok((false, failed.try_into().unwrap_or(u32::MAX)))
    }

    fn project_download_badge(&self, failed: u32) {
        if let Some(ui) = self.ui.upgrade() {
            ui.global::<ShellModel>().set_pending(failed.min(i32::MAX as u32) as i32);
        }
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
        if self.demo.is_some() {
            self.download_ui.demo_queued.insert(detail.comic.id.clone());
            self.toast("common_download_queued");
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
            match event.kind {
                DownloadEventKind::PageReady { .. } | DownloadEventKind::IndexReady { .. } | DownloadEventKind::PrefetchReady => reader_changed = true,
                DownloadEventKind::Failed { .. } | DownloadEventKind::Completed => reader_changed = true,
                _ => {}
            }
            self.dirty = true;
            let _ = self.platform.request_refresh(RefreshReason::Progress);
        }
        if reader_changed { self.reader_page_ready()?; }
        if enforce_limit && self.enforce_cache_limit().is_err() { self.toast("common_storage_error"); }
        let (active, failed) = self.download_activity()?;
        self.project_download_badge(failed);
        if active && self.platform.capabilities().keep_awake && self.download_ui.wake.is_none() {
            self.download_ui.wake = self.platform.hold_awake_deferred().ok();
        }
        if !active { self.download_ui.wake = None; }
        Ok(())
    }

    fn download_rows(&self) -> Result<Vec<DownloadRow>> {
        if let Some(demo) = &self.demo {
            let mut rows = Vec::new();
            for value in demo.fixture["downloads"].as_array().into_iter().flatten() {
                let id = value["comic_id"].as_u64().unwrap_or_default().to_string();
                if self.download_ui.demo_removed.contains(&id) { continue; }
                let original_kind = value["kind"].as_str().unwrap_or("done");
                let cancelled = self.download_ui.demo_cancelled.contains(&id);
                if cancelled && value["downloaded_mb"].as_f64().unwrap_or_default() <= 0.0 { continue; }
                let retried = self.download_ui.demo_queued.contains(&id);
                let kind = if cancelled { "done" } else if retried { "queued" } else { original_kind };
                let paused = !cancelled && self.download_ui.demo_paused.contains(&id) && matches!(kind, "active" | "queued");
                let queued = kind == "queued" && !paused;
                let active = kind == "active" && !paused;
                let error = kind == "error";
                let percent = value["percent"].as_f64().unwrap_or_default() as f32;
                let status = if cancelled && original_kind == "error" { i18n::text("dl_cancelled") }
                    else if paused { i18n::text("dl_paused") } else if error { i18n::text("common_download_failed") }
                    else if queued { i18n::text("dl_queued") } else if active { i18n::format("dl_progress_value", &[("percent", format!("{percent:.0}"))]) }
                    else { i18n::text("dl_offline_ready") };
                let task_label = value["task_label"].as_str().unwrap_or_default().to_owned();
                let summary = value["completed_summary"].as_str().unwrap_or_default().to_owned();
                let subtitle = if error {
                    [value["task_label"].as_str().unwrap_or_default(), value["error"].as_str().unwrap_or_default()].join(" · ")
                } else if kind == "queued" {
                    i18n::format(if retried && original_kind == "error" { "dl_requeued_episode" } else { "dl_waiting_episode" }, &[("episode", task_label.clone())])
                } else if kind == "active" {
                    [summary.clone(), i18n::format("dl_downloading_episode", &[("episode", task_label.clone())])].join(" · ")
                } else { value["completed_summary"].as_str().unwrap_or_default().to_owned() };
                let task_title = if error { i18n::format("dl_failed_episode", &[("episode", task_label.clone())]) }
                    else if kind == "active" { format!("{}{}", i18n::format("dl_downloading_episode", &[("episode", task_label.clone())]), if paused { format!(" · {}", i18n::text("dl_paused")) } else { String::new() }) }
                    else { format!("{} · {task_label}", i18n::text("dl_queued")) };
                let task_note = if error {
                    format!("{}{}", value["error"].as_str().unwrap_or_default(), i18n::text("dl_retry_missing_note"))
                } else if kind == "active" {
                    [value["progress_label"].as_str().unwrap_or_default().to_owned(), i18n::text("dl_sleep_resume_note")].join(" · ")
                } else { i18n::text("dl_queue_wait_note") };
                let chapters = value["chapter_rows"].as_array().into_iter().flatten().enumerate().map(|(index, chapter)| DownloadChapterItem {
                    id: index.to_string().into(), number: chapter["chapter"].as_str().unwrap_or_default().into(),
                    title: chapter["title"].as_str().unwrap_or_default().into(), size: chapter["value"].as_str().unwrap_or_default().into(),
                }).collect();
                let bytes = (value["downloaded_mb"].as_f64().unwrap_or_default() * 1_000_000.0) as u64;
                let priority = self.download_ui.demo_priority.get(&id).copied().unwrap_or_default();
                rows.push(DownloadRow { id, title: value["title"].as_str().unwrap_or_default().into(), status, subtitle, summary,
                    task_title, task_note, task_has_bar: kind == "active", can_read: bytes > 0,
                    has_task: matches!(kind, "active" | "queued"), priority,
                    progress: percent / 100.0, error, active, queued, paused,
                    bytes,
                    chapters, ..Default::default() });
            }
            sort_download_rows(&mut rows);
            return Ok(rows);
        }
        let mut groups = BTreeMap::<String, Vec<DownloadRecord>>::new();
        for record in self.store.downloads(&self.account)? {
            if record.status != DownloadStatus::Cancelled && record.retained {
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
            let error = records.iter().any(|record| record.status == DownloadStatus::Failed);
            let active = records.iter().any(|record| record.status == DownloadStatus::Running);
            let queued = records.iter().any(|record| record.status == DownloadStatus::Queued);
            let paused = !active && !queued && records.iter().any(|record| record.status == DownloadStatus::Paused);
            let bytes = records.iter().map(|record| record.bytes).sum::<u64>();
            let completed = records.iter().map(|record| record.completed_pages).sum::<u32>();
            let total = records.iter().map(|record| record.total_pages).sum::<u32>();
            let percent = if total > 0 { completed as f32 / total as f32 } else { 0.0 };
            let detail = self.store.load_detail(&self.account, &id)?;
            let episode_label = |record: &DownloadRecord| detail.as_ref().and_then(|detail| detail.episodes.iter().find(|episode| episode.id == record.episode_id))
                .map(|episode| i18n::format("dl_episode_number", &[("number", display_order(episode.order))]))
                .unwrap_or_else(|| if record.title.is_empty() { i18n::text("dl_current_episode") } else { record.title.clone() });
            let status = if error { i18n::text("common_download_failed") }
                else if active { i18n::format("dl_progress_value", &[("percent", format!("{:.0}", percent * 100.0))]) }
                else if queued { i18n::text("dl_queued") } else if paused { i18n::text("dl_paused") }
                else { i18n::text("dl_offline_ready") };
            let completed_records: Vec<_> = records.iter().filter(|record| record.status == DownloadStatus::Completed).collect();
            let completed_episodes: BTreeSet<_> = completed_records.iter().map(|record| record.episode_id.as_str()).collect();
            let completed_bytes = completed_records.iter().map(|record| record.bytes).sum::<u64>();
            let summary = if completed_episodes.is_empty() { i18n::text("dl_no_complete_episodes") }
                else { i18n::format("dl_completed_summary", &[("count", completed_episodes.len().to_string()), ("size", format_bytes(completed_bytes))]) };
            let selected_task = records.iter().find(|record| record.status == DownloadStatus::Failed)
                .or_else(|| records.iter().find(|record| record.status == DownloadStatus::Running))
                .or_else(|| records.iter().find(|record| matches!(record.status, DownloadStatus::Queued | DownloadStatus::Paused)));
            let label = selected_task.map(&episode_label).unwrap_or_default();
            let failure = selected_task.and_then(|record| record.error_code.as_deref()).map(download_failure_message).unwrap_or_default();
            let partial = selected_task.map(|record| i18n::format("dl_saved_pages", &[("count", record.completed_pages.to_string()),
                ("total", if record.total_pages > 0 { record.total_pages.to_string() } else { i18n::text("common_unknown") })])).unwrap_or_default();
            let task_title = if error { i18n::format("dl_failed_episode", &[("episode", label.clone())]) }
                else if active || paused { format!("{}{}", i18n::format("dl_downloading_episode", &[("episode", label.clone())]), if paused { format!(" · {}", i18n::text("dl_paused")) } else { String::new() }) }
                else { format!("{} · {label}", i18n::text("dl_queued")) };
            let task_note = if error { format!("{}{}", [failure.clone(), partial.clone()].into_iter().filter(|value| !value.is_empty()).collect::<Vec<_>>().join("，"), i18n::text("dl_retry_missing_note")) }
                else if active || paused { [i18n::format("dl_local_count", &[("count", completed.to_string()),
                    ("total", if total > 0 { total.to_string() } else { i18n::text("common_unknown") }), ("size", format_bytes(bytes))]), i18n::text("dl_sleep_resume_note")].join(" · ") }
                else { i18n::text("dl_queue_wait_note") };
            let subtitle = if error { [label, failure, partial].into_iter().filter(|value| !value.is_empty()).collect::<Vec<_>>().join(" · ") }
                else if active || paused { [summary.clone(), task_title.clone()].join(" · ") }
                else if queued { i18n::format("dl_waiting_episode", &[("episode", label)]) }
                else { summary.clone() };
            let chapters = records.iter().filter(|record| record.completed_pages > 0).map(|record| DownloadChapterItem {
                id: record.episode_id.clone().into(), number: episode_label(record).into(),
                title: record.title.clone().into(), size: if record.status == DownloadStatus::Completed { format_bytes(record.bytes) }
                    else { i18n::format("dl_page_count", &[("count", record.completed_pages.to_string()), ("total", if record.total_pages > 0 { record.total_pages.to_string() } else { i18n::text("common_unknown") })]) }.into(),
            }).collect();
            let priority = records.iter().filter(|record| matches!(record.status, DownloadStatus::Running | DownloadStatus::Queued))
                .map(|record| record.priority).max().unwrap_or_default();
            rows.push(DownloadRow { id, title: records.first().map(|record| record.comic_title.clone()).unwrap_or_default(), status,
                subtitle, summary, task_title, task_note, task_has_bar: !error && (active || paused && total > 0), can_read: completed > 0,
                has_task: active || queued || paused, priority, progress: percent, error, active, queued, paused, bytes, chapters });
        }
        sort_download_rows(&mut rows);
        Ok(rows)
    }

    pub(super) fn refresh_download_view(&mut self, ui: &AppWindow) -> Result<()> {
        let rows = self.download_rows()?;
        let per_page = if self.is_scribe() { 8 } else { 6 };
        let pages = rows.len().div_ceil(per_page).max(1);
        self.download_ui.page = self.download_ui.page.min(pages - 1);
        let visible = rows.iter().skip(self.download_ui.page * per_page).take(per_page).cloned().collect::<Vec<_>>();
        let mut items = Vec::new();
        for row in visible {
            let comic = self.store.comic(&self.account, &row.id)?.unwrap_or_else(|| Comic { id: row.id.clone(), title: row.title.clone(), ..Default::default() });
            items.push(DownloadItem { id: row.id.clone().into(), title: row.title.into(), cover: self.cover_image(&comic),
                status: row.status.into(), subtitle: row.subtitle.into(), progress: row.progress,
                error: row.error, active: row.active, has_bar: row.task_has_bar,
                selected: self.download_ui.selected.contains(&row.id) });
        }
        let model = ui.global::<DownloadModel>();
        model.set_items(super::model(items));
        model.set_selecting(self.download_ui.selecting);
        model.set_selected_count(self.download_ui.selected.len() as i32);
        model.set_selection_label(i18n::format("dl_selected_count", &[("count", self.download_ui.selected.len().to_string())]).into());
        let removal = self.selected_removal_estimate(&rows).ok().flatten();
        model.set_remove_label(if self.download_ui.selected.is_empty() { tr("dl_remove_short") }
            else if let Some(bytes) = removal { i18n::format(if self.is_scribe() && self.demo.is_some() { "dl_remove_release" }
                else if self.demo.is_some() || !self.is_scribe() { "dl_remove_size" } else { "dl_remove_estimate" }, &[("size", format_design_bytes(bytes))]).into() }
            else { tr("dl_remove_selected") });
        model.set_smart_label(i18n::format("dl_smart_value", &[("state", i18n::text(if self.preferences.smart { "acct_on" } else { "acct_off" }))]).into());
        model.set_page(self.download_ui.page as i32 + 1);
        model.set_page_count(pages as i32);
        model.set_storage_free(i18n::format("dl_runtime_free_space", &[("size", self.download_ui.free.clone())]).into());
        let active = if self.demo.is_some() { rows.iter().filter(|row| row.has_task).count() }
            else { rows.iter().filter(|row| row.active || row.queued).count() };
        let failed = rows.iter().filter(|row| row.error).count();
        let (has_controls, all_paused) = global_download_controls(&rows);
        model.set_has_active(has_controls);
        model.set_all_paused(all_paused);
        let cache_bytes = self.demo.as_ref().and_then(|demo| demo.fixture["download_summary"]["cache_mb"].as_u64()).unwrap_or_default().saturating_mul(1_000_000);
        let total_bytes = rows.iter().map(|row| row.bytes).sum::<u64>().saturating_add(cache_bytes);
        let summary = i18n::format(if self.is_scribe() { "dl_summary_scribe" } else { "dl_summary_paperwhite" }, &[
            ("count", rows.len().to_string()), ("size", if self.demo.is_some() { format_design_bytes(total_bytes) } else { format_bytes(total_bytes) }),
            ("active", if active > 0 { i18n::format("dl_summary_active", &[("count", active.to_string())]) } else { String::new() }),
            ("failed", if failed > 0 { i18n::format("dl_summary_failed", &[("count", failed.to_string())]) } else { String::new() }),
        ]);
        let summary = if self.is_scribe() {
            format!("{summary} · {}", i18n::format("dl_runtime_free_space", &[("size", self.download_ui.free.clone())]))
        } else { summary };
        model.set_summary(summary.into());
        let selected = self.download_ui.sheet.as_ref().and_then(|id| rows.iter().find(|row| row.id == *id)).cloned();
        model.set_sheet_open(selected.is_some());
        if let Some(row) = selected {
            let comic = self.store.comic(&self.account, &row.id)?.unwrap_or_default();
            model.set_sheet_id(row.id.into()); model.set_sheet_title(row.title.into());
            model.set_sheet_summary(row.summary.clone().into()); model.set_sheet_cover(self.cover_image(&comic));
            model.set_chapters(super::model(row.chapters));
            model.set_task_visible(row.error || row.active || row.queued || row.paused);
            model.set_task_has_bar(row.task_has_bar);
            model.set_task_title(row.task_title.into()); model.set_task_note(row.task_note.into());
            model.set_task_progress(row.progress);
            model.set_task_primary(tr(if row.error { "dl_retry" } else if row.active { "dl_pause" } else if row.queued { "dl_priority" } else { "dl_resume" }));
            model.set_task_primary_action(if row.error { "retry" } else if row.active { "pause" } else if row.queued { "priority" } else { "resume" }.into());
            model.set_task_secondary(tr(if row.error { "dl_release_space" } else { "common_cancel" }));
            model.set_task_secondary_action(if row.error { "storage" } else { "cancel" }.into());
            model.set_can_read(row.can_read);
            model.set_remove_current_label(tr(if row.bytes > 0 { "dl_remove_all_downloads" } else { "dl_cancel_download" }));
        }
        Ok(())
    }

    fn selected_removal_estimate(&self, rows: &[DownloadRow]) -> Result<Option<u64>> {
        if self.download_ui.selected.is_empty() { return Ok(Some(0)); }
        if self.demo.is_some() {
            return Ok(Some(rows.iter().filter(|row| self.download_ui.selected.contains(&row.id)).map(|row| row.bytes).sum()));
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
        Ok(Some(removable_object_bytes(&objects, &self.download_ui.selected)))
    }

    pub(super) fn download_action(&mut self, action: &str, value: &str) -> Result<()> {
        match action {
            "enqueue" => self.enqueue_episode(value, true)?,
            "enqueue-prefetch" => self.enqueue_episode(value, false)?,
            "select" => { self.download_ui.selecting = true; self.download_ui.selected.clear(); }
            "cancel-select" => { self.download_ui.selecting = false; self.download_ui.selected.clear(); }
            "toggle-item" => {
                if !self.download_ui.selected.remove(value) { self.download_ui.selected.insert(value.to_owned()); }
            }
            "open" => self.download_ui.sheet = Some(value.into()),
            "sheet-close" => self.download_ui.sheet = None,
            "page" => self.download_ui.page = self.download_ui.page.saturating_add_signed(value.parse::<isize>().unwrap_or_default()),
            "smart-settings" => { self.navigate("account"); self.account_action("section", "download")?; }
            "toggle-all" => {
                let rows = self.download_rows()?;
                let pause = rows.iter().any(|row| row.active || row.queued);
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
                }
                for intent in &mut self.download_ui.deferred { intent.paused = pause; }
                self.store.set_preference(&self.account, "queued_download_intents", &self.download_ui.deferred)?;
                self.flush_deferred_downloads()?;
            }
            "task" => {
                if value == "storage" {
                    self.download_ui.sheet = None; self.navigate("account"); self.account_action("section", "storage")?;
                    return Ok(());
                }
                if matches!(value, "resume" | "retry")
                    && (!self.require_network("downloads", "task", value) || !self.require_session()) { return Ok(()); }
                if let Some(id) = self.download_ui.sheet.clone() {
                    if self.demo.is_some() {
                        match value {
                            "pause" => { self.download_ui.demo_paused.insert(id.clone()); }
                            "resume" | "retry" => {
                                self.download_ui.demo_cancelled.remove(&id);
                                self.download_ui.demo_paused.remove(&id);
                                if value == "retry" { self.download_ui.demo_queued.insert(id.clone()); }
                            }
                            "priority" => {
                                self.download_ui.demo_priority_sequence = self.download_ui.demo_priority_sequence.checked_add(1).context("Preview priority overflow")?;
                                self.download_ui.demo_priority.insert(id.clone(), self.download_ui.demo_priority_sequence);
                            }
                            "cancel" => {
                                self.download_ui.demo_queued.remove(&id); self.download_ui.demo_paused.remove(&id);
                                self.download_ui.demo_cancelled.insert(id.clone());
                            }
                            _ => {}
                        }
                    } else {
                        for record in self.store.downloads(&self.account)?.iter().filter(|record| record.comic_id == id && record.retained) {
                            match value {
                                "pause" if matches!(record.status, DownloadStatus::Running | DownloadStatus::Queued) => self.apply_download_control(&record.id, "pause")?,
                                "resume" if record.status == DownloadStatus::Paused => self.apply_download_control(&record.id, "resume")?,
                                "retry" if record.status == DownloadStatus::Failed => self.apply_download_control(&record.id, "retry")?,
                                "priority" if matches!(record.status, DownloadStatus::Queued | DownloadStatus::Running) => self.apply_download_control(&record.id, "priority")?,
                                "cancel" if matches!(record.status, DownloadStatus::Running | DownloadStatus::Queued | DownloadStatus::Paused | DownloadStatus::Failed) => self.apply_download_control(&record.id, "cancel")?,
                                _ => {}
                            }
                        }
                    }
                    if self.demo.is_none() {
                        if value == "priority" {
                            let next = self.download_ui.deferred_priorities.values().copied().max().unwrap_or_default()
                                .checked_add(1).context("Deferred priority overflow")?;
                            for intent in self.download_ui.deferred.iter().filter(|intent| intent.comic_id == id && intent.retain && !intent.paused) {
                                self.download_ui.deferred_priorities.insert(intent.episode_id.clone(), next);
                            }
                            self.store.set_preference(&self.account, "queued_download_priorities", &self.download_ui.deferred_priorities)?;
                        }
                        if value == "cancel" { self.download_ui.deferred.retain(|intent| intent.comic_id != id); }
                        else { for intent in self.download_ui.deferred.iter_mut().filter(|intent| intent.comic_id == id) { if value == "pause" { intent.paused = true; } else if value == "resume" { intent.paused = false; } } }
                        self.store.set_preference(&self.account, "queued_download_intents", &self.download_ui.deferred)?;
                        self.flush_deferred_downloads()?;
                    }
                }
            }
            "remove-current" => {
                if let Some(id) = self.download_ui.sheet.take() { self.remove_download_groups(vec![id])?; }
            }
            "remove-selected" => {
                let ids = self.download_ui.selected.iter().cloned().collect();
                self.remove_download_groups(ids)?;
                self.download_ui.selected.clear(); self.download_ui.selecting = false;
            }
            "read" => {
                if let Some(id) = self.download_ui.sheet.take() { self.open_reader_for_comic(&id)?; }
            }
            _ => {}
        }
        Ok(())
    }

    fn remove_download_groups(&mut self, ids: Vec<String>) -> Result<()> {
        if self.is_suspended() { return Ok(()); }
        if self.demo.is_some() {
            self.download_ui.demo_removed.extend(ids);
            return Ok(());
        }
        let records = self.store.downloads(&self.account)?.into_iter().filter(|record| ids.contains(&record.comic_id)).collect::<Vec<_>>();
        self.download_ui.deferred.retain(|intent| !ids.contains(&intent.comic_id));
        self.store.set_preference(&self.account, "queued_download_intents", &self.download_ui.deferred)?;
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
                "pause" => manager.pause(task)?, "cancel" => manager.cancel(task)?,
                "resume" => { manager.resume(task)?; }, "retry" => { manager.retry(task)?; },
                "priority" => manager.prioritize(task)?, _ => {}
            }
        } else {
            if action == "priority" {
                self.download_ui.pending_controls.push((task.to_owned(), action.to_owned()));
                self.store.prioritize_download(&self.account, task)?;
                return Ok(());
            }
            let status = match action {
                "pause" => DownloadStatus::Paused, "cancel" => DownloadStatus::Cancelled,
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

fn row_priority(row: &DownloadRow) -> u8 { if row.error { 0 } else if row.active { 1 } else if row.queued { 2 } else if row.paused { 3 } else { 4 } }

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

fn format_design_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 { format!("{:.2} GB", bytes as f64 / 1_000_000_000.0) }
    else { format!("{} MB", bytes / 1_000_000) }
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
    fn all_paused_downloads_keep_a_visible_resume_control() {
        let rows = vec![DownloadRow { paused: true, has_task: true, ..Default::default() },
            DownloadRow { paused: true, has_task: true, ..Default::default() },
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

//! Presented-frame ownership and bounded reader scheduling regressions.

use super::*;
use crate::{platform::DeviceProfile, source::BiliClient};
use std::sync::atomic::AtomicUsize;

fn index() -> ImageIndex {
    ImageIndex { episode_id: "reader-performance-episode".into(), revision: "reader-performance-revision".into(),
        images: (0..3).map(|position| PageDescriptor { id: format!("reader-performance-page-{position}"),
            index: position, width: 16, height: 24 }).collect(), ..Default::default() }
}

fn ready_state() -> Result<ReadingState> {
    let index = index();
    let mut core = ReaderCore::new(index.clone(), ReaderSettings { landscape_spread: false, ..Default::default() })?;
    let source = image::GrayImage::from_pixel(16, 24, image::Luma([68]));
    let frame = core.render_source(&source, Viewport::new(16, 24))?;
    let mut state = ReadingState::pending("reader-performance-comic".into(), index.episode_id.clone(),
        41, SavedReaderPreferences::default(), false, false);
    state.original_index = Some(index);
    state.page = raster_image(rendered_raster(frame));
    state.presented_core = Some(core.clone());
    state.core = Some(core);
    state.loading = false;
    Ok(state)
}

fn page_ready(position: usize) -> DownloadEvent {
    let index = index();
    DownloadEvent { account_epoch: 1, task_id: "reader-performance-download".into(), generation: 3,
        kind: DownloadEventKind::PageReady { episode_id: index.episode_id, revision: index.revision,
            page_id: index.images[position].id.clone(), page_index: position,
            path: PathBuf::from("unused-fixture-path.png"), retained: false,
            completed_pages: 1, total_pages: 3 } }
}

async fn app(directory: &std::path::Path) -> Result<App> {
    let client = BiliClient::offline(directory)?;
    let mut app = super::super::test_support::app(directory, DeviceProfile::Paperwhite5,
        client, tokio::runtime::Handle::current())?;
    app.options.offline = true;
    app.route = "reader".into();
    app.reading = Some(ready_state()?);
    Ok(app)
}

fn install_render_files(app: &mut App, directory: &std::path::Path) -> Result<ReaderFiles> {
    let paths = (0..index().images.len()).map(|position| {
        let path = directory.join(format!("sleep-render-{position}.png"));
        image::GrayImage::from_pixel(16, 24, image::Luma([68 + position as u8 * 40])).save(&path)?;
        Ok(path)
    }).collect::<Result<Vec<_>>>()?;
    app.demo = Some(Arc::new(DemoData { comics: Vec::new(), details: Vec::new(), indices: vec![index()],
        pages: BTreeMap::from([(index().episode_id, paths.clone())]), covers: BTreeMap::new(),
        account_name: "Reader sleep fixture".into(), fixture: serde_json::json!({}), panels: BTreeMap::new(),
        strip: None, strip_panels: Vec::new() }));
    Ok(ReaderFiles::Demo { paths, strip: None, strip_active: false })
}

struct DeferredRender {
    release: tokio::sync::oneshot::Sender<()>,
    finished: tokio::task::JoinHandle<Result<bool>>,
}

async fn defer_prepared_render(app: &mut App, files: ReaderFiles) -> Result<DeferredRender> {
    let state = app.reading.as_mut().context("The fixture reader must exist")?;
    let mut core = state.core.clone().context("The fixture reader must have a target")?;
    let generation = state.generation;
    let version = state.version;
    let cancelled = Arc::new(AtomicBool::new(false));
    state.render_cancelled = cancelled.clone();
    state.rendering = true;
    state.rendering_version = Some(version);
    state.loading = true;
    state.render_started = Some(Instant::now());
    let (epoch, sequence, ticket) = app.begin_job("reader-render", false)?;
    let delivery = ticket.acquire_delivery().await.context("The fixture render must be admitted")?;
    let (prepared_sender, prepared_receiver) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let finished = app.handle.spawn_blocking(move || {
        let outcome = prepare_reader_frame(&mut core, &files, Viewport::new(16, 24), (8, 12), &cancelled)?;
        anyhow::ensure!(matches!(&outcome, PreparedOutcome::Ready(_)), "The fixture must prepare a real frame");
        let result = Ok(PreparedReader { core, outcome });
        prepared_sender.send(()).map_err(|_| anyhow::anyhow!("The fixture preparation receiver closed"))?;
        // Model the completion race after the renderer's final cancellation check.
        // The real pixels and their mailbox ticket remain owned by this worker.
        released.blocking_recv().context("The fixture render was not released")?;
        Ok(ticket.publish(super::super::jobs::JobEvent { epoch, sequence, key: "reader-render".into(),
            apply: Box::new(move |app| app.apply_prepared_reader(generation, version, None, result)) }, delivery))
    });
    tokio::time::timeout(Duration::from_secs(5), prepared_receiver).await
        .context("The fixture render did not prepare")?.context("The fixture render failed before preparation")?;
    Ok(DeferredRender { release, finished })
}

async fn release_render(render: DeferredRender) -> Result<bool> {
    render.release.send(()).map_err(|_| anyhow::anyhow!("The fixture render worker closed"))?;
    tokio::time::timeout(Duration::from_secs(5), render.finished).await
        .context("The fixture render did not finish")?.context("The fixture render worker failed")?
}

struct AdmittedRender {
    render: DeferredRender,
    worker_exits: Arc<AtomicUsize>,
    callbacks: Arc<AtomicUsize>,
}

async fn defer_cancellable_render(app: &mut App) -> Result<AdmittedRender> {
    let state = app.reading.as_mut().context("The fixture reader must exist")?;
    let core = state.core.clone().context("The fixture reader must have a target")?;
    let generation = state.generation;
    let version = state.version;
    let cancelled = Arc::new(AtomicBool::new(false));
    state.render_cancelled = cancelled.clone();
    state.rendering = true;
    state.rendering_version = Some(version);
    state.loading = true;
    state.render_started = Some(Instant::now());
    let (epoch, sequence, ticket) = app.begin_job("reader-render", false)?;
    let delivery = ticket.acquire_delivery().await.context("The fixture render must be admitted")?;
    let (started, did_start) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let worker_exits = Arc::new(AtomicUsize::new(0));
    let exited = worker_exits.clone();
    let callbacks = Arc::new(AtomicUsize::new(0));
    let applied = callbacks.clone();
    let finished = app.handle.spawn(async move {
        let (delivery, prepared) = crate::source::work::run_blocking(
            crate::source::work::HEAVY_WORK_BYTES, move |worker_cancel| -> Result<_> {
                started.send(()).map_err(|_| anyhow::anyhow!("The fixture admission receiver closed"))?;
                // Retain the real compute reservation and delivery slot until
                // the test permits this obsolete worker to leave its boundary.
                released.blocking_recv().context("The fixture worker was not released")?;
                anyhow::ensure!(!worker_cancel.is_cancelled(), "Reader cancellation retired the actual worker early");
                anyhow::ensure!(cancelled.load(Ordering::Acquire), "The obsolete reader task was not cancelled");
                exited.fetch_add(1, Ordering::SeqCst);
                Ok((delivery, PreparedReader { core, outcome: PreparedOutcome::Cancelled }))
            },
        ).await.context("The fixture computation was not admitted")??;
        Ok(ticket.publish(super::super::jobs::JobEvent { epoch, sequence, key: "reader-render".into(),
            apply: Box::new(move |app| {
                applied.fetch_add(1, Ordering::SeqCst);
                app.apply_prepared_reader(generation, version, None, Ok(prepared));
            }) }, delivery))
    });
    tokio::time::timeout(Duration::from_secs(5), did_start).await
        .context("The fixture computation did not start")?.context("The fixture computation failed before admission")?;
    Ok(AdmittedRender { render: DeferredRender { release, finished }, worker_exits, callbacks })
}

async fn finish_reader_render(app: &mut App) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            app.tick()?;
            let state = app.reading.as_ref().context("The fixture reader disappeared")?;
            anyhow::ensure!(state.error.is_none(), "The final reader render failed");
            if !state.loading && !state.rendering { break; }
            tokio::task::yield_now().await;
        }
        Ok::<(), anyhow::Error>(())
    }).await.context("The final reader render did not finish")?
}

#[test]
fn a_pending_target_does_not_replace_the_displayed_frame_or_checkpoint() -> Result<()> {
    let mut state = ready_state()?;
    state.core.as_mut().unwrap().set_page(1)?;
    state.loading = true;
    state.rendering = true;
    state.rendering_version = Some(state.version);
    state.render_started = Some(Instant::now());
    assert!(state.retain_frame_during_preparation());
    assert_eq!(state.core.as_ref().unwrap().page_index(), 1);
    assert_eq!(state.visible_core().unwrap().page_index(), 0);
    assert_eq!(state.displayed_anchor(9).unwrap().0.page_index, 0);
    assert!(!state.preparation_feedback_due());
    state.render_started = Some(Instant::now() - Duration::from_secs(1));
    assert!(state.preparation_feedback_due());

    state.missing_pages.push(1);
    assert!(!state.retain_frame_during_preparation());
    assert_eq!(state.displayed_anchor(10).unwrap().0.page_index, 0);
    state.missing_pages.clear();
    state.error = Some("Controlled missing target".into());
    assert!(!state.retain_frame_during_preparation());
    assert_eq!(state.displayed_anchor(11).unwrap().0.page_index, 0);
    Ok(())
}

#[test]
fn an_unrendered_initial_target_has_no_new_durable_position() -> Result<()> {
    let mut state = ready_state()?;
    let saved = state.presented_core.take().unwrap().anchor(7);
    state.page = slint::Image::default();
    state.core.as_mut().unwrap().set_page(2)?;
    assert!(state.displayed_anchor(8).is_none());
    state.paged_anchor = Some(saved.clone());
    let restored = state.displayed_anchor(9).unwrap().0;
    assert_eq!(restored.page_index, saved.page_index);
    assert_eq!(restored.source_y, saved.source_y);
    assert_eq!(restored.updated_at, 9);
    assert!(!state.retain_frame_during_preparation());
    Ok(())
}

#[test]
fn panel_and_strip_checkpoints_keep_the_last_presented_crop() -> Result<()> {
    for mode in [DisplayMode::Panel, DisplayMode::Strip] {
        let mut state = ready_state()?;
        let mut core = ReaderCore::new(index(), ReaderSettings { mode,
            landscape_spread: false, ..Default::default() })?;
        if mode == DisplayMode::Panel {
            core.set_panel_map("reader-performance-page-0", vec![
                NormalizedRect { x: 0.0, y: 0.0, width: 1.0, height: 0.5 },
                NormalizedRect { x: 0.0, y: 0.5, width: 1.0, height: 0.5 },
            ])?;
        }
        let source = image::GrayImage::from_pixel(16, 240, image::Luma([68]));
        core.render_source(&source, Viewport::new(16, 24))?;
        let anchor = core.anchor(0);
        state.presented_core = Some(core.clone());
        assert_eq!(core.step(true), ReaderMove::Moved);
        state.core = Some(core);
        state.loading = true;
        assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
        assert_eq!(state.visible_core().unwrap().crop_index(), 0);
    }
    Ok(())
}

#[test]
fn adjacent_and_unrelated_downloads_are_not_visible_dependencies() -> Result<()> {
    let mut state = ready_state()?;
    state.core.as_mut().unwrap().set_page(1)?;
    assert!(state.depends_on_download(&page_ready(1)));
    assert!(!state.depends_on_download(&page_ready(0)));
    assert!(!state.depends_on_download(&page_ready(2)));
    let mut other = page_ready(1);
    if let DownloadEventKind::PageReady { episode_id, .. } = &mut other.kind { *episode_id = "other-episode".into(); }
    assert!(!state.depends_on_download(&other));
    let mut replaced = page_ready(1);
    if let DownloadEventKind::PageReady { revision, .. } = &mut replaced.kind { *revision = "other-revision".into(); }
    assert!(!state.depends_on_download(&replaced));
    let mut wrong_page = page_ready(1);
    if let DownloadEventKind::PageReady { page_id, .. } = &mut wrong_page.kind { *page_id = "other-page".into(); }
    assert!(!state.depends_on_download(&wrong_page));
    Ok(())
}

#[test]
fn terminal_download_events_require_the_current_acquisition_owner() -> Result<()> {
    let mut state = ready_state()?;
    state.acquisition = Some(ReaderAcquisition { task_id: "owned-task".into(), task_generation: 6,
        reader_generation: state.generation, revision: index().revision, visible: vec![0], requested: vec![0, 1] });
    let mut event = DownloadEvent { account_epoch: 1, task_id: "other-task".into(), generation: 6,
        kind: DownloadEventKind::Completed };
    assert!(!state.depends_on_download(&event));
    event.task_id = "owned-task".into();
    assert!(state.depends_on_download(&event));
    event.generation += 1;
    assert!(!state.depends_on_download(&event));
    Ok(())
}

#[tokio::test]
async fn a_visible_page_notification_does_not_revoke_an_in_flight_render() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = app(directory.path()).await?;
    let state = app.reading.as_mut().unwrap();
    state.loading = true;
    state.rendering = true;
    let version = state.version;
    state.rendering_version = Some(version);
    let cancelled = state.render_cancelled.clone();
    app.reader_page_ready()?;
    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.version, version);
    assert!(state.render_again);
    assert!(state.rendering);
    assert_eq!(state.rendering_version, Some(version));
    assert!(!cancelled.load(Ordering::Acquire));
    assert!(Arc::ptr_eq(&state.render_cancelled, &cancelled));
    assert_eq!(state.displayed_anchor(0).unwrap().0.page_index, 0);
    Ok(())
}

#[tokio::test]
async fn superseded_page_targets_wait_for_the_real_worker_then_render_only_the_latest_page() -> Result<()> {
    let _work_serial = crate::source::work::process_test_lock().lock().await;
    let directory = tempfile::tempdir()?;
    let mut app = app(directory.path()).await?;
    install_render_files(&mut app, directory.path())?;
    app.save_reading_position()?;
    let state = app.reading.as_mut().unwrap();
    let retained = state.page.clone();
    let anchor = state.displayed_anchor(0).unwrap().0;
    assert_eq!(state.core.as_ref().unwrap().settings().mode, DisplayMode::Page);
    assert_eq!(state.core.as_ref().unwrap().settings().fit, FitMode::Page);
    state.core.as_mut().unwrap().set_page(1)?;
    state.version = state.version.wrapping_add(1);
    let old_version = state.version;
    let blocked = defer_cancellable_render(&mut app).await?;
    let sequence = app.sequence;
    let old_cancelled = app.reading.as_ref().unwrap().render_cancelled.clone();

    for target in ["3", "1", "2", "3"] {
        app.reader_action("page", target)?;
        let state = app.reading.as_ref().unwrap();
        assert!(old_cancelled.load(Ordering::Acquire));
        assert!(state.rendering);
        assert!(state.render_again);
        assert_eq!(state.rendering_version, Some(old_version));
        assert!(Arc::ptr_eq(&state.render_cancelled, &old_cancelled));
        assert!(state.queued.is_empty());
        assert_eq!(state.page, retained);
        assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
        assert_eq!(app.sequence, sequence);
        assert_eq!(app.pending.get("reader-render"), Some(&sequence));
        assert_eq!(blocked.worker_exits.load(Ordering::SeqCst), 0);
        assert_eq!(blocked.callbacks.load(Ordering::SeqCst), 0);
    }
    let final_version = app.reading.as_ref().unwrap().version;
    assert_ne!(final_version, old_version);
    assert_eq!(app.reading.as_ref().unwrap().core.as_ref().unwrap().page_index(), 2);
    app.save_reading_position()?;
    assert_eq!(app.store.load_anchor(&app.account, "reader-performance-comic")?.unwrap().page_index, 0);
    assert_eq!(app.reader_sleep_checkpoint().unwrap().2.page_index, 0);

    assert!(release_render(blocked.render).await?);
    assert_eq!(blocked.worker_exits.load(Ordering::SeqCst), 1);
    assert_eq!(blocked.callbacks.load(Ordering::SeqCst), 0);
    assert_eq!(app.sequence, sequence);
    assert_eq!(app.reading.as_ref().unwrap().rendering_version, Some(old_version));
    app.tick()?;
    let state = app.reading.as_ref().unwrap();
    assert_eq!(blocked.callbacks.load(Ordering::SeqCst), 1);
    assert_eq!(app.sequence, sequence + 1);
    assert_eq!(app.pending.get("reader-render"), Some(&(sequence + 1)));
    assert!(state.rendering);
    assert_eq!(state.rendering_version, Some(final_version));
    assert!(!state.render_cancelled.load(Ordering::Acquire));
    assert!(!Arc::ptr_eq(&state.render_cancelled, &old_cancelled));
    assert_eq!(state.page, retained);
    assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);

    finish_reader_render(&mut app).await?;
    let state = app.reading.as_ref().unwrap();
    assert_eq!(app.sequence, sequence + 1);
    assert!(!app.pending.contains_key("reader-render"));
    assert!(!state.rendering);
    assert!(state.rendering_version.is_none());
    assert!(state.queued.is_empty());
    assert_eq!(state.core.as_ref().unwrap().page_index(), 2);
    assert_eq!(state.presented_core.as_ref().unwrap().page_index(), 2);
    assert_eq!(state.displayed_anchor(0).unwrap().0.page_index, 2);
    assert_ne!(state.page, retained);
    app.save_reading_position()?;
    assert_eq!(app.store.load_anchor(&app.account, "reader-performance-comic")?.unwrap().page_index, 2);
    Ok(())
}

#[tokio::test]
async fn a_prepared_result_cannot_repopulate_a_closed_reader() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = app(directory.path()).await?;
    let files = install_render_files(&mut app, directory.path())?;
    let state = app.reading.as_mut().unwrap();
    state.core.as_mut().unwrap().set_page(1)?;
    state.version = state.version.wrapping_add(1);
    let render = defer_prepared_render(&mut app, files).await?;
    let cancelled = app.reading.as_ref().unwrap().render_cancelled.clone();
    app.close_reader()?;
    assert!(app.reading.is_none());
    assert!(cancelled.load(Ordering::Acquire));
    assert!(!app.pending.contains_key("reader-render"));
    assert!(!release_render(render).await?);
    app.tick()?;
    assert!(app.reading.is_none());
    assert!(!app.pending.contains_key("reader-render"));
    assert_eq!(app.store.load_anchor(&app.account, "reader-performance-comic")?.unwrap().page_index, 0);
    Ok(())
}

#[tokio::test]
async fn an_old_epoch_result_cannot_replace_a_new_epochs_presented_frame() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = app(directory.path()).await?;
    let files = install_render_files(&mut app, directory.path())?;
    let state = app.reading.as_mut().unwrap();
    state.core.as_mut().unwrap().set_page(1)?;
    state.version = state.version.wrapping_add(1);
    let old_generation = state.generation;
    let old_version = state.version;
    let render = defer_prepared_render(&mut app, files).await?;
    let cancelled = app.reading.as_ref().unwrap().render_cancelled.clone();
    let sequence = app.sequence;
    // Isolate the consumer's epoch fence even if a delayed envelope remains
    // current in the mailbox and reader-local identities happen to coincide.
    app.epoch = app.epoch.wrapping_add(1);
    app.retire_reader_checkpoints();
    let mut replacement = ready_state()?;
    replacement.generation = old_generation;
    replacement.version = old_version;
    replacement.core.as_mut().unwrap().set_page(2)?;
    replacement.presented_core = replacement.core.clone();
    let retained = replacement.page.clone();
    let anchor = replacement.displayed_anchor(0).unwrap().0;
    app.reading = Some(replacement);
    assert!(cancelled.load(Ordering::Acquire));
    assert!(release_render(render).await?);
    app.tick()?;
    let state = app.reading.as_ref().unwrap();
    assert_eq!(app.sequence, sequence);
    assert_eq!(state.page, retained);
    assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
    assert_eq!(state.core.as_ref().unwrap().page_index(), 2);
    assert!(!state.loading);
    assert!(!state.rendering);
    assert!(state.rendering_version.is_none());
    assert!(!state.render_cancelled.load(Ordering::Acquire));
    assert!(!app.pending.contains_key("reader-render"));
    Ok(())
}

#[tokio::test]
async fn a_failed_close_releases_the_reader_and_retries_only_its_lightweight_checkpoint() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = app(directory.path()).await?;
    app.save_reading_position()?;
    let connection = rusqlite::Connection::open(app.store.path())?;
    connection.execute_batch(
        "CREATE TRIGGER reject_reader_checkpoint BEFORE INSERT ON reading_anchors
         BEGIN SELECT RAISE(FAIL, 'controlled reader checkpoint failure'); END;"
    )?;
    let state = app.reading.as_mut().unwrap();
    state.core.as_mut().unwrap().set_page(1)?;
    state.presented_core = state.core.clone();
    let cancellation = state.cancelled.clone();
    app.close_reader()?;
    assert!(app.reading.is_none());
    assert!(cancellation.load(Ordering::Acquire));
    let diagnostics = app.lifecycle.reader_checkpoints.diagnostics();
    assert!(diagnostics.pending);
    assert_eq!(diagnostics.failed_attempts, 1);
    assert_eq!(app.reader_sleep_checkpoint().unwrap().2.page_index, 1);
    assert_eq!(app.store.load_anchor(&app.account, "reader-performance-comic")?.unwrap().page_index, 0);
    assert!(app.begin_reader("other-comic", "other-episode").is_err());
    assert!(app.reading.is_none());

    connection.execute_batch("DROP TRIGGER reject_reader_checkpoint")?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            app.tick_reader_checkpoints();
            if !app.lifecycle.reader_checkpoints.diagnostics().pending { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.context("The closed-reader checkpoint did not recover automatically")?;
    assert!(app.reading.is_none());
    assert_eq!(app.store.load_anchor(&app.account, "reader-performance-comic")?.unwrap().page_index, 1);
    Ok(())
}

#[tokio::test]
async fn pending_navigation_cannot_advance_a_sleep_checkpoint() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = app(directory.path()).await?;
    let state = app.reading.as_mut().unwrap();
    state.core.as_mut().unwrap().set_page(2)?;
    state.loading = true;
    state.rendering = true;
    state.rendering_version = Some(state.version);
    app.save_reading_position()?;
    assert_eq!(app.store.load_anchor(&app.account, "reader-performance-comic")?.unwrap().page_index, 0);
    assert_eq!(app.reader_sleep_checkpoint().unwrap().2.page_index, 0);
    assert_eq!(app.reader_lifecycle_snapshot().unwrap()["page_index"], 0);
    Ok(())
}

#[tokio::test]
async fn a_settled_frame_keeps_its_render_generation_across_sleep() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = app(directory.path()).await?;
    let before = app.reader_lifecycle_snapshot();
    let version = app.reading.as_ref().unwrap().version;
    let token = app.reading.as_ref().unwrap().render_cancelled.clone();
    app.suspend()?;
    assert_eq!(app.reading.as_ref().unwrap().version, version);
    assert!(app.reading.as_ref().unwrap().rendering_version.is_none());
    assert!(!token.load(Ordering::Acquire));
    app.resume()?;
    app.tick()?;
    assert_eq!(app.reader_lifecycle_snapshot(), before);
    assert!(app.reading.as_ref().unwrap().rendering_version.is_none());
    assert!(!app.pending.contains_key("reader-render"));
    Ok(())
}

#[tokio::test]
async fn a_prepared_navigation_finishing_during_sleep_cannot_replace_the_retained_frame() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = app(directory.path()).await?;
    let files = install_render_files(&mut app, directory.path())?;
    let state = app.reading.as_mut().unwrap();
    let retained = state.page.clone();
    let anchor = state.displayed_anchor(0).unwrap().0;
    let reader_cancelled = state.cancelled.clone();
    state.core.as_mut().unwrap().set_page(2)?;
    state.version = state.version.wrapping_add(1);
    state.queued.push_back(("turn".into(), "next".into(), Some(crate::performance::Operation::fixture(31, "reader"))));
    state.performance_request = Some(crate::performance::Operation::fixture(32, "reader"));
    state.performance_presented = Some(crate::performance::Operation::fixture(30, "reader"));
    state.performance_model_id = Some(30);
    let render = defer_prepared_render(&mut app, files).await?;
    let render_cancelled = app.reading.as_ref().unwrap().render_cancelled.clone();
    assert_eq!(app.reading.as_ref().unwrap().rendering_version,
        Some(app.reading.as_ref().unwrap().version));

    app.begin_suspend_intent_gate();
    app.suspend()?;
    let state = app.reading.as_ref().unwrap();
    assert!(render_cancelled.load(Ordering::Acquire));
    assert!(!reader_cancelled.load(Ordering::Acquire));
    assert!(!state.rendering);
    assert!(state.rendering_version.is_none());
    assert!(state.queued.is_empty());
    assert!(state.performance_request.is_none());
    assert!(state.performance_model_id.is_none());
    assert_eq!(state.performance_presented.map(|trace| trace.id()), Some(30));
    assert_eq!(state.page, retained);
    assert_eq!(state.core.as_ref().unwrap().anchor(0), anchor);
    assert!(!release_render(render).await?);
    app.tick()?;
    assert_eq!(app.reading.as_ref().unwrap().page, retained);

    app.resume()?;
    app.tick()?;
    app.save_reading_position()?;
    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
    assert_eq!(state.core.as_ref().unwrap().page_index(), anchor.page_index);
    assert!(!state.rendering);
    assert!(state.rendering_version.is_none());
    assert!(state.queued.is_empty());
    assert_eq!(app.store.load_anchor(&app.account, "reader-performance-comic")?.unwrap().page_index, anchor.page_index);
    assert_eq!(app.reader_lifecycle_snapshot().unwrap()["page_index"], anchor.page_index);
    Ok(())
}

#[tokio::test]
async fn an_initial_frame_finishing_during_sleep_is_installed_after_resume() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = app(directory.path()).await?;
    let files = install_render_files(&mut app, directory.path())?;
    let state = app.reading.as_mut().unwrap();
    state.presented_core = None;
    state.page = slint::Image::default();
    state.core.as_mut().unwrap().set_page(2)?;
    let render = defer_prepared_render(&mut app, files).await?;
    let render_cancelled = app.reading.as_ref().unwrap().render_cancelled.clone();
    let version = app.reading.as_ref().unwrap().version;

    app.suspend()?;
    assert!(!render_cancelled.load(Ordering::Acquire));
    assert!(app.reading.as_ref().unwrap().rendering);
    assert_eq!(app.reading.as_ref().unwrap().rendering_version, Some(version));
    assert!(release_render(render).await?);
    app.tick()?;
    assert!(app.reading.as_ref().unwrap().presented_core.is_none());
    assert_eq!(app.reading.as_ref().unwrap().rendering_version, Some(version));
    assert!(app.pending.contains_key("reader-render"));

    app.resume()?;
    app.tick()?;
    app.save_reading_position()?;
    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.presented_core.as_ref().unwrap().page_index(), 2);
    assert_eq!((state.page.size().width, state.page.size().height), (16, 24));
    assert!(!state.rendering);
    assert!(state.rendering_version.is_none());
    assert!(!state.loading);
    assert!(!app.pending.contains_key("reader-render"));
    assert_eq!(app.store.load_anchor(&app.account, "reader-performance-comic")?.unwrap().page_index, 2);
    Ok(())
}

#[tokio::test]
async fn resume_reflows_the_retained_page_with_pending_manual_settings() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = app(directory.path()).await?;
    let files = install_render_files(&mut app, directory.path())?;
    let state = app.reading.as_mut().unwrap();
    let anchor = state.displayed_anchor(0).unwrap().0;
    state.core.as_mut().unwrap().set_page(2)?;
    let render = defer_prepared_render(&mut app, files).await?;
    app.options.landscape = true;
    app.reader_action("setting", "mode:panel")?;
    app.reader_action("setting", "fit:width")?;
    app.reader_action("setting", "direction:rtl")?;
    app.reader_action("setting", "landscape-layout:single")?;
    app.reader_action("setting", "dither:off")?;
    let state = app.reading.as_mut().unwrap();
    let core = state.core.as_mut().unwrap();
    let mut settings = core.settings().clone();
    settings.rotation = 90;
    settings.zoom_ratio = 1.25;
    core.update_settings(settings.clone())?;
    core.set_landscape(true);

    app.suspend()?;
    assert!(!release_render(render).await?);
    app.resume()?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            app.tick()?;
            let state = app.reading.as_ref().unwrap();
            if !state.loading && !state.rendering { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<(), anyhow::Error>(())
    }).await.context("The retained page did not reflow after resume")??;
    app.save_reading_position()?;

    let state = app.reading.as_ref().unwrap();
    assert!(state.error.is_none());
    assert!(!state.rendering);
    assert!(state.rendering_version.is_none());
    assert_eq!(state.presented_core.as_ref().unwrap().page_index(), anchor.page_index);
    assert_eq!(state.presented_core.as_ref().unwrap().settings(), &settings);
    assert_eq!(state.core.as_ref().unwrap().settings(), &settings);
    assert_eq!(state.saved.mode, "panel");
    assert_eq!(state.saved.fit, "width");
    assert_eq!(state.saved.direction, "rtl");
    assert_eq!(state.saved.landscape, "single");
    assert!(!state.saved.dither);
    assert!(app.options.landscape);
    let saved = app.store.load_anchor(&app.account, "reader-performance-comic")?.unwrap();
    assert_eq!(saved.page_index, anchor.page_index);
    assert_eq!(saved.rotation, settings.rotation);
    assert_eq!(saved.zoom_ratio, settings.zoom_ratio);
    Ok(())
}

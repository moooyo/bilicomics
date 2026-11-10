//! Single-slot prediction ownership, invalidation, and presentation regressions.

use super::*;
use crate::{platform::DeviceProfile, source::BiliClient};
use std::sync::atomic::AtomicUsize;

fn index() -> ImageIndex {
    ImageIndex { episode_id: "prepared-reader-episode".into(), revision: "prepared-reader-revision".into(),
        images: (0..3).map(|position| PageDescriptor { id: format!("prepared-reader-page-{position}"),
            index: position, width: 16, height: 24 }).collect(), ..Default::default() }
}

fn files(directory: &std::path::Path) -> Result<ReaderFiles> {
    let paths = (0..3).map(|position| {
        let path = directory.join(format!("prepared-reader-{position}.png"));
        image::GrayImage::from_fn(16, 24, |x, y| image::Luma([
            ((x * 11 + y * 7 + position * 59) % 256) as u8])).save(&path)?;
        Ok(path)
    }).collect::<Result<Vec<_>>>()?;
    Ok(ReaderFiles::Demo { paths, strip: None, strip_active: false })
}

fn viewport(app: &App, core: &ReaderCore) -> Viewport {
    reader_viewport(app.platform.logical_viewport_size(), app.options.landscape,
        core.settings(), app.platform.scale_factor())
}

async fn app(directory: &std::path::Path) -> Result<(App, ReaderFiles)> {
    let client = BiliClient::offline(directory)?;
    let mut app = super::super::test_support::app(directory, DeviceProfile::Paperwhite5,
        client, tokio::runtime::Handle::current())?;
    let files = files(directory)?;
    let ReaderFiles::Demo { paths, .. } = &files else { unreachable!() };
    app.demo = Some(Arc::new(DemoData { comics: Vec::new(), details: Vec::new(), indices: vec![index()],
        pages: BTreeMap::from([(index().episode_id.clone(), paths.clone())]), covers: BTreeMap::new(),
        account_name: "Prepared reader fixture".into(), fixture: serde_json::json!({}),
        panels: BTreeMap::new(), strip: None, strip_panels: Vec::new() }));
    app.options.offline = true;
    app.route = "reader".into();
    let mut core = ReaderCore::new(index(), ReaderSettings { landscape_spread: false, ..Default::default() })?;
    let size = viewport(&app, &core);
    let PreparedOutcome::Ready(frame) = prepare_reader_frame(&mut core, &files, size, (8, 12), &AtomicBool::new(false))?
        else { anyhow::bail!("The fixture must have a real presented frame"); };
    let mut state = ReadingState::pending("prepared-reader-comic".into(), index().episode_id,
        41, SavedReaderPreferences::default(), false, true);
    state.original_index = Some(index());
    state.core = Some(core.clone());
    state.presented_core = Some(core);
    state.page = raster_image(frame.page);
    state.metadata = frame.metadata;
    state.loading = false;
    app.reading = Some(state);
    Ok((app, files))
}

fn readable(app: &mut App) {
    app.reading.as_mut().unwrap().detail = Some(ComicDetail {
        episodes: vec![Episode { id: index().episode_id, access: EpisodeAccess::Free,
            ..Default::default() }], ..Default::default() });
}

fn target(app: &App) -> (ReaderCore, NextFrameKey) {
    let state = app.reading.as_ref().unwrap();
    let presented = state.presented_core.as_ref().unwrap();
    let mut core = presented.clone();
    assert_eq!(core.step(true), ReaderMove::Moved);
    let key = NextFrameKey { epoch: app.epoch, generation: state.generation,
        origin_version: state.version, origin: FrameInput::new(presented), target: FrameInput::new(&core),
        viewport: viewport(app, &core), landscape: app.options.landscape };
    (core, key)
}

fn candidate(app: &App, files: &ReaderFiles) -> Result<NextFrame> {
    let (core, key) = target(app);
    prepare_next_frame(core, key, files, &AtomicBool::new(false), None)?
        .context("The fixture must prepare the next frame")
}

fn publish_candidate(app: &mut App, frame: NextFrame) {
    let key = frame.key.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    app.reading.as_mut().unwrap().next_frame = NextFrameSlot::Preparing {
        key: key.clone(), cancelled: cancelled.clone() };
    app.apply_next_frame(key, cancelled, None, Ok(Some(frame)));
}

async fn finish_render(app: &mut App) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            app.tick()?;
            let state = app.reading.as_ref().context("The fixture reader disappeared")?;
            anyhow::ensure!(state.error.is_none(), "The foreground fixture render failed");
            if !state.loading && !state.rendering { return Ok::<(), anyhow::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await.context("The foreground fixture render did not finish")?
}

struct DeferredPrediction {
    cancelled: Arc<AtomicBool>,
    release: tokio::sync::oneshot::Sender<()>,
    exits: Arc<AtomicUsize>,
    callbacks: Arc<AtomicUsize>,
}

async fn defer_prediction(app: &mut App, files: &ReaderFiles) -> Result<DeferredPrediction> {
    let (core, key) = target(app);
    // Other parallel tests may briefly own the process budget. Retry admission
    // only in the fixture, then use an explicit worker boundary for assertions.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let cancelled = Arc::new(AtomicBool::new(false));
            let (started, did_start) = tokio::sync::oneshot::channel();
            let (release, released) = tokio::sync::oneshot::channel();
            let exits = Arc::new(AtomicUsize::new(0));
            let worker_exits = exits.clone();
            let callbacks = Arc::new(AtomicUsize::new(0));
            let applied = callbacks.clone();
            let worker_flag = cancelled.clone();
            let callback_flag = cancelled.clone();
            let worker_key = key.clone();
            let callback_key = key.clone();
            let worker_core = core.clone();
            let worker_files = files.clone();
            let admitted = app.background_speculative("reader-prepare-next", None, cancelled.clone(), move || {
                let frame = prepare_next_frame(worker_core, worker_key, &worker_files, &worker_flag, None)?;
                anyhow::ensure!(frame.is_some(), "The fixture prediction must contain real pixels");
                started.send(()).map_err(|_| anyhow::anyhow!("The prediction admission receiver closed"))?;
                released.blocking_recv().context("The prediction fixture was not released")?;
                worker_exits.fetch_add(1, Ordering::SeqCst);
                Ok(frame)
            }, move |app, result| {
                applied.fetch_add(1, Ordering::SeqCst);
                app.apply_next_frame(callback_key, callback_flag, None, result);
            });
            if !admitted {
                tokio::task::yield_now().await;
                continue;
            }
            app.reading.as_mut().unwrap().next_frame = NextFrameSlot::Preparing {
                key: key.clone(), cancelled: cancelled.clone() };
            if did_start.await.is_ok() {
                return Ok::<_, anyhow::Error>(DeferredPrediction { cancelled, release, exits, callbacks });
            }
            // Foreground work from another test can preempt an admitted job
            // before its body starts. Drain that real completion before retry.
            while app.pending.contains_key("reader-prepare-next") {
                app.tick()?;
                tokio::task::yield_now().await;
            }
        }
    }).await.context("The prediction fixture never reached its worker boundary")?
}

#[tokio::test]
async fn prepared_completion_keeps_the_live_frame_model_and_checkpoint_unchanged() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, files) = app(directory.path()).await?;
    readable(&mut app);
    app.save_reading_position()?;
    let frame = candidate(&app, &files)?;
    let state = app.reading.as_mut().unwrap();
    let page = state.page.clone();
    let anchor = state.displayed_anchor(0).unwrap().0;
    let metadata = state.metadata;
    let version = state.version;
    state.performance_presented = Some(Operation::fixture(90, "reader"));
    state.performance_model_id = Some(90);
    publish_candidate(&mut app, frame);
    let state = app.reading.as_ref().unwrap();
    assert!(matches!(state.next_frame, NextFrameSlot::Ready(_)));
    assert!(state.next_frame.retained_bytes() <= NEXT_FRAME_BYTES);
    assert_eq!(state.page, page);
    assert_eq!(state.metadata, metadata);
    assert_eq!(state.version, version);
    assert_eq!(state.core.as_ref().unwrap().anchor(0), anchor);
    assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
    assert_eq!(state.performance_presented.map(Operation::id), Some(90));
    assert_eq!(state.performance_model_id, Some(90));
    assert!(!state.history_sent);
    assert!(state.last_dark_fraction.is_none());
    assert_eq!(app.store.load_anchor(&app.account, "prepared-reader-comic")?.unwrap().page_index, 0);
    app.close_reader()?;
    let resources = app.reader_resource_snapshot();
    assert_eq!(resources["prepared_frame_state"], "empty");
    assert_eq!(resources["prepared_frame_bytes"], 0);
    Ok(())
}

#[tokio::test]
async fn a_real_next_navigation_consumes_the_slot_under_its_new_render_version() -> Result<()> {
    let _serial = crate::source::work::process_test_lock().lock().await;
    let directory = tempfile::tempdir()?;
    let (mut app, files) = app(directory.path()).await?;
    readable(&mut app);
    let frame = candidate(&app, &files)?;
    let expected_anchor = frame.core.anchor(0);
    let expected_metadata = frame.frame.metadata;
    publish_candidate(&mut app, frame);
    // Keep this test focused on the consumed frame rather than a second prediction.
    app.reading.as_mut().unwrap().detail = None;
    app.save_reading_position()?;
    let previous = app.reading.as_ref().unwrap().page.clone();
    let old_version = app.reading.as_ref().unwrap().version;
    app.reader_action_traced("turn", "next", Some(Operation::fixture(91, "reader")))?;
    let state = app.reading.as_ref().unwrap();
    assert!(matches!(state.next_frame, NextFrameSlot::Empty));
    assert_eq!(state.version, old_version.wrapping_add(1));
    assert_eq!(state.rendering_version, Some(state.version));
    assert_eq!(state.page, previous);
    assert_eq!(state.displayed_anchor(0).unwrap().0.page_index, 0);
    assert_eq!(app.store.load_anchor(&app.account, "prepared-reader-comic")?.unwrap().page_index, 0);
    finish_render(&mut app).await?;
    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.presented_core.as_ref().unwrap().anchor(0), expected_anchor);
    assert_eq!(state.metadata, expected_metadata);
    assert_eq!(state.performance_presented.map(Operation::id), Some(91));
    assert!(matches!(state.next_frame, NextFrameSlot::Empty));
    app.save_reading_position()?;
    assert_eq!(app.store.load_anchor(&app.account, "prepared-reader-comic")?.unwrap().page_index, 1);
    Ok(())
}

#[tokio::test]
async fn next_frame_keys_reject_rapid_reverse_epoch_generation_viewport_and_settings_changes() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, _) = app(directory.path()).await?;
    let (_, key) = target(&app);
    let state = app.reading.as_mut().unwrap();
    assert!(key.origin_is_current(state, app.epoch, key.viewport, false));
    state.core.as_mut().unwrap().step(true);
    state.version = state.version.wrapping_add(1);
    assert!(key.target_is_current(state, app.epoch, key.viewport, false));
    assert!(!key.target_is_current(state, app.epoch.wrapping_add(1), key.viewport, false));
    assert!(!key.target_is_current(state, app.epoch, Viewport::new(key.viewport.width + 1, key.viewport.height), false));
    assert!(!key.target_is_current(state, app.epoch, key.viewport, true));
    state.generation = state.generation.wrapping_add(1);
    assert!(!key.target_is_current(state, app.epoch, key.viewport, false));
    state.generation = key.generation;
    let settings = state.core.as_ref().unwrap().settings().clone();
    let mut changed = settings.clone();
    changed.footer = !changed.footer;
    state.core.as_mut().unwrap().update_settings(changed)?;
    assert!(!key.target_is_current(state, app.epoch, key.viewport, false));
    state.core.as_mut().unwrap().update_settings(settings)?;
    state.core.as_mut().unwrap().step(false);
    state.version = state.version.wrapping_add(1);
    assert!(!key.target_is_current(state, app.epoch, key.viewport, false));
    state.core.as_mut().unwrap().step(true);
    state.version = state.version.wrapping_add(1);
    assert!(!key.target_is_current(state, app.epoch, key.viewport, false));
    state.version = key.origin_version.wrapping_add(1);
    let mut revision = index();
    revision.revision = "replaced-reader-revision".into();
    let mut replacement = ReaderCore::new(revision, state.core.as_ref().unwrap().settings().clone())?;
    replacement.set_page(1)?;
    state.core = Some(replacement);
    assert!(!key.target_is_current(state, app.epoch, key.viewport, false));
    Ok(())
}

#[tokio::test]
async fn prediction_admission_skips_unsupported_or_unsettled_reader_positions() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, _) = app(directory.path()).await?;
    readable(&mut app);
    let original = app.reading.as_ref().unwrap().presented_core.clone().unwrap();
    for case in ["panel", "strip", "width", "spread", "last-source", "transition", "queued", "unsettled"] {
        let state = app.reading.as_mut().unwrap();
        state.core = Some(original.clone());
        state.presented_core = Some(original.clone());
        state.queued.clear();
        let core = state.core.as_mut().unwrap();
        let mut settings = core.settings().clone();
        match case {
            "panel" => settings.mode = DisplayMode::Panel,
            "strip" => settings.mode = DisplayMode::Strip,
            "width" => settings.fit = FitMode::Width,
            "spread" => { settings.landscape_spread = true; core.set_landscape(true); }
            "last-source" => core.set_page(2)?,
            "transition" => { core.set_page(2)?; core.step(true); }
            "queued" => state.queued.push_back(("turn".into(), "next".into(), None)),
            "unsettled" => core.set_page(1)?,
            _ => unreachable!(),
        }
        core.update_settings(settings)?;
        if case != "unsettled" { state.presented_core = state.core.clone(); }
        let sequence = app.sequence;
        app.request_next_frame();
        assert!(matches!(app.reading.as_ref().unwrap().next_frame, NextFrameSlot::Empty), "Unexpected prediction for {case}");
        assert_eq!(app.sequence, sequence, "Unexpected background admission for {case}");
    }
    Ok(())
}

#[tokio::test]
async fn footer_changes_and_settled_sleep_revoke_only_the_speculative_frame() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, files) = app(directory.path()).await?;
    readable(&mut app);
    let frame = candidate(&app, &files)?;
    publish_candidate(&mut app, frame);
    let version = app.reading.as_ref().unwrap().version;
    app.reader_action("setting", "show-footer:off")?;
    assert!(matches!(app.reading.as_ref().unwrap().next_frame, NextFrameSlot::Empty));
    assert_eq!(app.reading.as_ref().unwrap().version, version);
    app.reader_action("setting", "show-footer:on")?;
    let frame = candidate(&app, &files)?;
    publish_candidate(&mut app, frame);
    let token = app.reading.as_ref().unwrap().render_cancelled.clone();
    let anchor = app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0;
    let page = app.reading.as_ref().unwrap().page.clone();
    app.suspend()?;
    let state = app.reading.as_ref().unwrap();
    assert!(matches!(state.next_frame, NextFrameSlot::Empty));
    assert_eq!(state.version, version);
    assert!(!token.load(Ordering::Acquire));
    assert_eq!(state.page, page);
    // A resumed eligible reader may independently try a fresh prediction.
    app.reading.as_mut().unwrap().detail = None;
    app.resume()?;
    assert_eq!(app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0, anchor);
    assert!(matches!(app.reading.as_ref().unwrap().next_frame, NextFrameSlot::Empty));
    Ok(())
}

#[tokio::test]
async fn sleep_discards_a_completed_prediction_waiting_in_the_mailbox() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, files) = app(directory.path()).await?;
    let frame = candidate(&app, &files)?;
    let key = frame.key.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let callback_flag = cancelled.clone();
    app.reading.as_mut().unwrap().next_frame = NextFrameSlot::Preparing {
        key: key.clone(), cancelled: cancelled.clone() };
    let (epoch, sequence, ticket) = app.begin_job("reader-prepare-next", false)?;
    let delivery = ticket.acquire_delivery().await.context("The fixture mailbox must admit its result")?;
    let callbacks = Arc::new(AtomicUsize::new(0));
    let applied = callbacks.clone();
    assert!(ticket.publish(super::super::jobs::JobEvent { epoch, sequence, key: "reader-prepare-next".into(),
        apply: Box::new(move |app| {
            applied.fetch_add(1, Ordering::SeqCst);
            app.apply_next_frame(key, callback_flag, None, Ok(Some(frame)));
        }) }, delivery));
    let version = app.reading.as_ref().unwrap().version;
    let render_token = app.reading.as_ref().unwrap().render_cancelled.clone();
    app.suspend()?;
    assert!(cancelled.load(Ordering::Acquire));
    assert!(!render_token.load(Ordering::Acquire));
    assert_eq!(app.reading.as_ref().unwrap().version, version);
    assert!(matches!(app.reading.as_ref().unwrap().next_frame, NextFrameSlot::Empty));
    assert!(!app.pending.contains_key("reader-prepare-next"));
    assert!(app.jobs.take().is_none());
    app.resume()?;
    app.tick()?;
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);
    assert_eq!(app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0.page_index, 0);
    Ok(())
}

#[tokio::test]
async fn sleep_cancels_speculation_without_revoking_an_initial_reader_render() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, _) = app(directory.path()).await?;
    let (_, key) = target(&app);
    let cancelled = Arc::new(AtomicBool::new(false));
    let state = app.reading.as_mut().unwrap();
    state.next_frame = NextFrameSlot::Preparing { key, cancelled: cancelled.clone() };
    state.presented_core = None;
    state.page = slint::Image::default();
    state.loading = true;
    state.rendering = true;
    state.rendering_version = Some(state.version);
    let version = state.version;
    let initial = state.render_cancelled.clone();
    app.begin_suspend_intent_gate();
    let state = app.reading.as_ref().unwrap();
    assert!(cancelled.load(Ordering::Acquire));
    assert!(matches!(state.next_frame, NextFrameSlot::Empty));
    assert!(!initial.load(Ordering::Acquire));
    assert!(state.loading && state.rendering);
    assert_eq!(state.version, version);
    assert_eq!(state.rendering_version, Some(version));
    Ok(())
}

#[tokio::test]
async fn obsolete_epoch_or_expired_access_cannot_install_or_consume_prepared_pixels() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, files) = app(directory.path()).await?;
    readable(&mut app);
    let frame = candidate(&app, &files)?;
    let key = frame.key.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    app.reading.as_mut().unwrap().next_frame = NextFrameSlot::Preparing {
        key: key.clone(), cancelled: cancelled.clone() };
    app.epoch = app.epoch.wrapping_add(1);
    app.apply_next_frame(key, cancelled, None, Ok(Some(frame)));
    assert!(matches!(app.reading.as_ref().unwrap().next_frame, NextFrameSlot::Empty));
    let frame = candidate(&app, &files)?;
    publish_candidate(&mut app, frame);
    let page = app.reading.as_ref().unwrap().page.clone();
    app.reading.as_mut().unwrap().detail.as_mut().unwrap().episodes[0].expires_at = Some(now() - 1);
    app.reader_action("turn", "next")?;
    let state = app.reading.as_ref().unwrap();
    assert!(matches!(state.next_frame, NextFrameSlot::Empty));
    assert!(state.error.is_some());
    assert_eq!(state.page, page);
    assert_eq!(state.displayed_anchor(0).unwrap().0.page_index, 0);
    assert!(!state.rendering);
    Ok(())
}

#[tokio::test]
async fn capacity_accounting_rejects_spare_rgba_allocation_and_releases_slots() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, files) = app(directory.path()).await?;
    readable(&mut app);
    let mut frame = candidate(&app, &files)?;
    frame.frame.page.rgba = Vec::with_capacity(NEXT_FRAME_BYTES as usize + 1);
    frame.frame.page.rgba.extend_from_slice(&[0, 0, 0, 255]);
    assert!(frame.retained_bytes() > NEXT_FRAME_BYTES);
    publish_candidate(&mut app, frame);
    assert!(matches!(app.reading.as_ref().unwrap().next_frame, NextFrameSlot::Empty));
    for _ in 0..8 {
        let frame = candidate(&app, &files)?;
        assert!(frame.retained_bytes() > frame.frame.page.rgba.len() as u64);
        publish_candidate(&mut app, frame);
        assert!(matches!(app.reading.as_ref().unwrap().next_frame, NextFrameSlot::Ready(_)));
        app.reading.as_mut().unwrap().next_frame.cancel();
        assert_eq!(app.reading.as_ref().unwrap().next_frame.retained_bytes(), 0);
    }
    app.close_reader()?;
    assert_eq!(app.reader_resource_snapshot()["prepared_frame_bytes"], 0);
    Ok(())
}

#[test]
fn prediction_metadata_accounting_includes_reserved_panel_capacity() -> Result<()> {
    let mut core = ReaderCore::new(index(), ReaderSettings::default())?;
    let before = core.retained_preparation_metadata_bytes();
    let mut panels = Vec::with_capacity(4096);
    panels.push(NormalizedRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 });
    let reserved = panels.capacity() * std::mem::size_of::<NormalizedRect>();
    core.set_panel_map("prepared-reader-page-0", panels)?;
    assert!(core.retained_preparation_metadata_bytes() >= before + reserved as u64);
    Ok(())
}

#[tokio::test]
async fn a_predicted_source_notification_retires_the_slot_without_invalidating_visible_work() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, files) = app(directory.path()).await?;
    readable(&mut app);
    let frame = candidate(&app, &files)?;
    publish_candidate(&mut app, frame);
    let state = app.reading.as_ref().unwrap();
    let page = state.page.clone();
    let version = state.version;
    let event = DownloadEvent { account_epoch: app.epoch, task_id: "prepared-source-notification".into(),
        generation: 1, kind: DownloadEventKind::PageReady { episode_id: index().episode_id,
            revision: index().revision, page_id: index().images[1].id.clone(), page_index: 1,
            path: PathBuf::from("unused-prepared-notification.png"), retained: false,
            completed_pages: 2, total_pages: 3 } };
    assert!(!app.reader_download_dependency_changed(&event));
    let state = app.reading.as_ref().unwrap();
    assert!(matches!(state.next_frame, NextFrameSlot::Empty));
    assert_eq!(state.version, version);
    assert_eq!(state.page, page);
    assert!(!state.rendering);
    Ok(())
}

#[tokio::test]
async fn preload_off_releases_ready_pixels_and_reenable_offers_a_new_prediction() -> Result<()> {
    let _serial = crate::source::work::process_test_lock().lock().await;
    let directory = tempfile::tempdir()?;
    let (mut app, files) = app(directory.path()).await?;
    readable(&mut app);
    let frame = candidate(&app, &files)?;
    let expected = frame.core.anchor(0);
    let expected_metadata = frame.frame.metadata;
    publish_candidate(&mut app, frame);
    let state = app.reading.as_ref().unwrap();
    let generation = state.generation;
    let version = state.version;
    let anchor = state.displayed_anchor(0).unwrap().0;
    let page = state.page.clone();
    app.account_action("preload", "0")?;
    let state = app.reading.as_ref().unwrap();
    assert_eq!(app.preferences.preload, 0);
    assert!(matches!(state.next_frame, NextFrameSlot::Empty));
    assert_eq!(state.next_frame.retained_bytes(), 0);
    assert_eq!((state.generation, state.version), (generation, version));
    assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
    assert_eq!(state.page, page);
    assert!(!app.pending.contains_key("reader-prepare-next"));
    let sequence = app.sequence;
    app.request_next_frame();
    assert_eq!(app.sequence, sequence);
    assert!(matches!(app.reading.as_ref().unwrap().next_frame, NextFrameSlot::Empty));

    app.reader_action("turn", "next")?;
    assert_eq!(app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0, anchor);
    finish_render(&mut app).await?;
    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.displayed_anchor(0).unwrap().0, expected);
    assert_eq!(state.metadata, expected_metadata);
    assert!(matches!(state.next_frame, NextFrameSlot::Empty));
    let version = state.version;
    // Busy is a legitimate result in production. Only this fixture repeats the
    // existing setting intent if another parallel test owns the process slots.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if matches!(app.reading.as_ref().unwrap().next_frame, NextFrameSlot::Empty) {
                app.account_action("preload", "3")?;
            }
            app.tick()?;
            let state = app.reading.as_ref().unwrap();
            assert_eq!((state.generation, state.version), (generation, version));
            assert_eq!(state.displayed_anchor(0).unwrap().0, expected);
            if matches!(state.next_frame, NextFrameSlot::Ready(_)) { return Ok::<(), anyhow::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await.context("Re-enabling preloading did not offer a fresh prediction")??;
    assert_eq!(app.preferences.preload, 3);
    app.close_reader()?;
    assert_eq!(app.reader_resource_snapshot()["prepared_frame_bytes"], 0);
    Ok(())
}

#[tokio::test]
async fn disabled_preloading_rejects_late_predictions_and_foreground_candidates() -> Result<()> {
    let _serial = crate::source::work::process_test_lock().lock().await;
    let directory = tempfile::tempdir()?;
    let (mut app, files) = app(directory.path()).await?;
    readable(&mut app);
    let frame = candidate(&app, &files)?;
    app.preferences.preload = 0;
    publish_candidate(&mut app, frame);
    assert!(matches!(app.reading.as_ref().unwrap().next_frame, NextFrameSlot::Empty));

    // Inject a late slot to exercise the consumer's independent preference
    // fence. Its poisoned metadata makes accidental reuse observable.
    let mut stale = candidate(&app, &files)?;
    let expected = stale.core.anchor(0);
    stale.frame.metadata.left_page = -71;
    app.reading.as_mut().unwrap().next_frame = NextFrameSlot::Ready(Box::new(stale));
    app.reader_action("turn", "next")?;
    assert!(matches!(app.reading.as_ref().unwrap().next_frame, NextFrameSlot::Empty));
    finish_render(&mut app).await?;
    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.displayed_anchor(0).unwrap().0, expected);
    assert_eq!(state.metadata.left_page, 2);
    assert!(matches!(state.next_frame, NextFrameSlot::Empty));
    Ok(())
}

#[tokio::test]
async fn preload_off_retires_a_preparing_ticket_without_releasing_its_worker_lease_early() -> Result<()> {
    let _serial = crate::source::work::process_test_lock().lock().await;
    let directory = tempfile::tempdir()?;
    let (mut app, files) = app(directory.path()).await?;
    let DeferredPrediction { cancelled, release, exits, callbacks } = defer_prediction(&mut app, &files).await?;
    let state = app.reading.as_ref().unwrap();
    let generation = state.generation;
    let version = state.version;
    let anchor = state.displayed_anchor(0).unwrap().0;
    app.account_action("preload", "0")?;
    let state = app.reading.as_ref().unwrap();
    assert!(cancelled.load(Ordering::Acquire));
    assert!(matches!(state.next_frame, NextFrameSlot::Empty));
    assert_eq!((state.generation, state.version), (generation, version));
    assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
    assert!(!app.pending.contains_key("reader-prepare-next"));
    assert_eq!(exits.load(Ordering::SeqCst), 0);
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);
    assert!(!app.background_speculative("preload-disabled-lease-check", None,
        Arc::new(AtomicBool::new(false)), || Ok(()), |_, _| {}));
    release.send(()).map_err(|_| anyhow::anyhow!("The disabled prediction worker exited early"))?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while exits.load(Ordering::SeqCst) == 0 { tokio::task::yield_now().await; }
    }).await.context("The disabled prediction worker did not leave its boundary")?;
    app.tick()?;
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);
    assert!(matches!(app.reading.as_ref().unwrap().next_frame, NextFrameSlot::Empty));
    assert_eq!(app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0, anchor);
    Ok(())
}

#[tokio::test]
async fn a_cancelled_prediction_keeps_its_real_worker_lease_until_exit_and_never_promotes() -> Result<()> {
    let _serial = crate::source::work::process_test_lock().lock().await;
    let directory = tempfile::tempdir()?;
    let (mut app, files) = app(directory.path()).await?;
    let DeferredPrediction { cancelled, release, exits, callbacks } = defer_prediction(&mut app, &files).await?;
    app.reader_action("turn", "next")?;
    assert!(cancelled.load(Ordering::Acquire));
    assert!(matches!(app.reading.as_ref().unwrap().next_frame, NextFrameSlot::Preparing { .. }));
    assert_eq!(exits.load(Ordering::SeqCst), 0);
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);
    let attempted = Arc::new(AtomicUsize::new(0));
    let executed = attempted.clone();
    assert!(!app.background_speculative("reader-prepare-other", None, Arc::new(AtomicBool::new(false)),
        move || { executed.fetch_add(1, Ordering::SeqCst); Ok(()) }, |_, _| {}));
    assert_eq!(attempted.load(Ordering::SeqCst), 0);
    finish_render(&mut app).await?;
    assert_eq!(app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0.page_index, 1);
    assert_eq!(exits.load(Ordering::SeqCst), 0);
    release.send(()).map_err(|_| anyhow::anyhow!("The prediction fixture exited early"))?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            app.tick()?;
            if callbacks.load(Ordering::SeqCst) == 1 { return Ok::<(), anyhow::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await.context("The cancelled prediction did not report its actual exit")??;
    assert_eq!(exits.load(Ordering::SeqCst), 1);
    assert!(matches!(app.reading.as_ref().unwrap().next_frame, NextFrameSlot::Empty));
    assert_eq!(app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0.page_index, 1);
    Ok(())
}

#[tokio::test]
async fn late_cancelled_prediction_retries_after_delivery_and_prepares_the_new_next_page() -> Result<()> {
    let _serial = crate::source::work::process_test_lock().lock().await;
    let directory = tempfile::tempdir()?;
    let (mut app, files) = app(directory.path()).await?;
    readable(&mut app);
    let DeferredPrediction { cancelled, release, exits, callbacks } = defer_prediction(&mut app, &files).await?;
    let generation = app.reading.as_ref().unwrap().generation;
    app.reader_action("turn", "next")?;
    assert!(cancelled.load(Ordering::Acquire));
    finish_render(&mut app).await?;
    let state = app.reading.as_ref().unwrap();
    let anchor = state.displayed_anchor(0).unwrap().0;
    let page = state.page.clone();
    let version = state.version;
    assert_eq!(anchor.page_index, 1);
    assert!(matches!(state.next_frame, NextFrameSlot::Preparing { .. }));
    assert_eq!(exits.load(Ordering::SeqCst), 0);
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);
    app.save_reading_position()?;

    release.send(()).map_err(|_| anyhow::anyhow!("The late prediction worker exited early"))?;
    // Only the normal UI tick delivers the stale result and offers the new
    // prediction. Calling request_next_frame here would conceal a mailbox-lane
    // retry attempted before its old delivery permit is actually released.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            app.tick()?;
            let state = app.reading.as_ref().context("The settled reader disappeared")?;
            assert_eq!(state.generation, generation);
            assert_eq!(state.version, version);
            assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
            assert_eq!(state.page, page);
            assert!(state.error.is_none());
            if let NextFrameSlot::Ready(frame) = &state.next_frame {
                assert_eq!(frame.key.origin_version, version);
                assert_eq!(frame.key.origin.anchor, anchor);
                assert_eq!(frame.key.target.anchor.page_index, 2);
                assert_eq!(frame.core.anchor(0).page_index, 2);
                assert!(frame.retained_bytes() <= NEXT_FRAME_BYTES);
                assert!(!state.next_frame_retry_pending);
                return Ok::<(), anyhow::Error>(());
            }
            tokio::task::yield_now().await;
        }
    }).await.context("The late prediction did not prepare the new next page after delivery")??;
    assert_eq!(exits.load(Ordering::SeqCst), 1);
    assert_eq!(callbacks.load(Ordering::SeqCst), 1);
    assert_eq!(app.store.load_anchor(&app.account, "prepared-reader-comic")?.unwrap().page_index, 1);
    let sequence = app.sequence;
    for _ in 0..8 { app.tick()?; tokio::task::yield_now().await; }
    assert_eq!(app.sequence, sequence, "A completed deferred opportunity must not keep scheduling work");
    app.close_reader()?;
    assert_eq!(app.reader_resource_snapshot()["prepared_frame_bytes"], 0);
    Ok(())
}

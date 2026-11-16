//! Persistent binary frames, presentation-bound DU scenes, and saved preferences.

use super::*;
use crate::{platform::{DeviceProfile, DitheringSink, PlatformError, RefreshSink}, source::BiliClient};
use std::{path::Path, sync::Mutex};

#[derive(Default)]
struct RecordedRefreshes(Mutex<Vec<RefreshPlan>>);

impl RefreshSink for RecordedRefreshes {
    fn submit(&self, plan: RefreshPlan) -> std::result::Result<(), PlatformError> {
        self.0.lock().unwrap().push(plan);
        Ok(())
    }
}

impl RecordedRefreshes {
    fn take(&self) -> Vec<RefreshPlan> { std::mem::take(&mut *self.0.lock().unwrap()) }
}

struct HardwareDither(AtomicBool);

impl DitheringSink for HardwareDither {
    fn hardware_y4_available(&self) -> bool { true }
    fn set_hardware_y4(&self, enabled: bool) -> bool { self.0.store(enabled, Ordering::Release); enabled }
}

fn index() -> ImageIndex {
    ImageIndex { episode_id: "fast-reader-episode".into(), revision: "fast-reader-revision".into(),
        images: (0..3).map(|position| PageDescriptor { id: format!("fast-reader-page-{position}"),
            index: position, width: 64, height: 96 }).collect(), ..Default::default() }
}

fn source() -> image::GrayImage {
    image::GrayImage::from_fn(64, 96, |x, y| image::Luma([((x * 11 + y * 7) % 256) as u8]))
}

fn fixture(directory: &Path, handle: tokio::runtime::Handle, fast: bool, raw_research: bool)
    -> Result<(App, Arc<RecordedRefreshes>)>
{
    let mut app = super::super::test_support::app(directory, DeviceProfile::Paperwhite5,
        BiliClient::offline(directory)?, handle)?;
    app.route = "reader".into();
    app.options.offline = true;
    app.preferences.preload = 0;
    app.preferences.every = "6".into();
    app.preferences.refresh_mode = if fast { "fast" } else { "normal" }.into();
    app.preferences.dither_method = "bayer8".into();
    app.platform.set_dithering_sink(Arc::new(HardwareDither(AtomicBool::new(false))))?;
    let refreshes = Arc::new(RecordedRefreshes::default());
    app.platform.set_refresh_sink(refreshes.clone())?;
    let saved = SavedReaderPreferences::from(&app.preferences);
    let mut settings = reader_settings_for_display(&saved, false, (930, 1240), true);
    if raw_research {
        // Only a retained-frame handoff regression needs the internal raw path.
        // Stored product choices always remain versioned CPU algorithms.
        settings.spatial_dither = None;
        settings.hardware_dither = true;
    }
    settings.landscape_spread = false;
    settings.rotation = 180;
    settings.zoom_ratio = 1.5;
    let mut core = ReaderCore::new(index(), settings)?;
    core.set_page(1)?;
    let rendered = core.render_source(&source(), Viewport::new(64, 96))?;
    let mut state = ReadingState::pending("fast-reader-comic".into(), index().episode_id.clone(),
        41, saved, false, false);
    state.detail = Some(ComicDetail { episodes: vec![Episode { id: index().episode_id,
        access: EpisodeAccess::Free, ..Default::default() }], ..Default::default() });
    state.original_index = Some(index());
    state.core = Some(core.clone());
    state.presented_core = Some(core);
    state.page = raster_image(rendered_raster(rendered));
    state.loading = false;
    app.reading = Some(state);
    app.configure_reader_refresh()?;
    app.sync_presented_reader_dithering();
    refreshes.take();
    Ok((app, refreshes))
}

fn import_target(app: &mut App) -> Result<Vec<u8>> {
    let mut core = app.reading.as_ref().unwrap().core.clone().unwrap();
    let rendered = core.render_source(&source(), Viewport::new(64, 96))?;
    let pixels = rendered.rgba.clone();
    let frame = PreparedFrame { dark_fraction: rendered.dark_fraction, page: rendered_raster(rendered),
        second: None, overview: None, metadata: FrameMetadata::default() };
    let state = app.reading.as_mut().unwrap();
    let (generation, version) = (state.generation, state.version);
    state.rendering = true;
    state.rendering_version = Some(version);
    state.loading = true;
    app.apply_prepared_reader(generation, version, None,
        Ok(PreparedReader { core, outcome: PreparedOutcome::Ready(frame) }));
    Ok(pixels)
}

fn key(app: &App) -> NextFrameKey {
    let state = app.reading.as_ref().unwrap();
    let origin = state.presented_core.as_ref().unwrap();
    let mut target = origin.clone();
    assert_eq!(target.step(true), ReaderMove::Moved);
    NextFrameKey { epoch: app.epoch, generation: state.generation, origin_version: state.version,
        origin: FrameInput::new(origin), target: FrameInput::new(&target), viewport: Viewport::new(64, 96), landscape: false }
}

fn is_du(kind: RefreshKind) -> bool { matches!(kind, RefreshKind::Fast | RefreshKind::FastFull) }

#[test]
fn visible_global_overlays_outlive_expired_intents_and_toast_publication_is_pre_routed() {
    assert!(reader_global_overlay_visible(Some(true), false, false),
        "A visible toast cannot enter DU merely because its deadline expired");
    assert!(reader_global_overlay_visible(Some(false), false, true),
        "A new toast intent must select gray-safe routing before its UI model is set");
    assert!(!reader_global_overlay_visible(Some(false), true, false),
        "A closed network UI is authoritative even when a deferred intent remains");
    assert!(reader_global_overlay_visible(None, true, false));
    assert!(reader_global_overlay_visible(None, false, true));
    assert!(!reader_global_overlay_visible(None, false, false));
}

#[test]
fn refresh_mode_defaults_and_round_trips_without_losing_normal_preferences() -> Result<()> {
    let legacy: Preferences = serde_json::from_value(serde_json::json!({"every":"6","dither":false,"dither_method":"hardware"}))?;
    let legacy_saved = SavedReaderPreferences::from(&legacy);
    assert_eq!(legacy.refresh_mode, "normal");
    assert_eq!(legacy_saved.refresh_mode, "normal");
    assert_eq!(normalized_refresh_mode("unknown"), "normal");
    let saved = SavedReaderPreferences { refresh_mode: "fast".into(), ..legacy_saved };
    let settings = reader_settings_for_display(&saved, false, (930, 1240), true);
    assert!(settings.fast_mode && !settings.uses_hardware_dither());
    assert!(!settings.dither);
    let restored: SavedReaderPreferences = serde_json::from_value(serde_json::to_value(&saved)?)?;
    assert_eq!(restored.refresh_mode, "fast");
    assert_eq!(restored.every, "6");
    assert!(!restored.dither);
    assert_eq!(restored.dither_method, "bayer8");
    assert_eq!(settings.spatial_dither, Some(SpatialDither::Off));
    assert!(!settings.hardware_dither);
    let directory = tempfile::tempdir()?;
    let store = crate::store::Store::open(directory.path().join("state.sqlite3"))?;
    store.set_preference("fixture", "reader_preferences_book", &restored)?;
    let actual: SavedReaderPreferences = store.preference("fixture", "reader_preferences_book")?.unwrap();
    assert_eq!(actual.refresh_mode, "fast");
    assert_eq!(actual.every, "6");
    assert_eq!(actual.dither_method, "bayer8");
    Ok(())
}

#[tokio::test]
async fn mode_switch_waits_for_matching_pixels_before_changing_y4_and_du_scene() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, refreshes) = fixture(directory.path(), tokio::runtime::Handle::current(), false, true)?;
    let state = app.reading.as_ref().unwrap();
    let page = state.page.clone();
    let anchor = state.displayed_anchor(0).unwrap().0;
    let version = state.version;
    let before_key = key(&app);
    let cancelled = Arc::new(AtomicBool::new(false));
    app.reading.as_mut().unwrap().next_frame = NextFrameSlot::Preparing { key: before_key.clone(), cancelled: cancelled.clone() };
    app.change_reader_setting("refresh-mode:fast")?;
    app.sync_presented_reader_dithering();
    let state = app.reading.as_ref().unwrap();
    assert!(cancelled.load(Ordering::Acquire));
    assert_eq!(state.version, version.wrapping_add(1));
    assert_eq!(state.page, page);
    assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
    assert!(state.core.as_ref().unwrap().settings().fast_mode);
    assert!(!state.presented_core.as_ref().unwrap().settings().fast_mode);
    assert!(!before_key.origin.matches(state.core.as_ref().unwrap()));
    assert_eq!(app.platform.reader_refresh_scene(), ReaderRefreshScene::Normal);
    assert!(app.platform.reader_hardware_y4_requested(), "The explicit retained Gray8 research frame still needs Y4");
    refreshes.take();
    let binary = import_target(&mut app)?;
    assert!(binary.chunks_exact(4).all(|pixel| matches!(pixel[0], 0 | 255)));
    assert_eq!(app.platform.reader_refresh_scene(), ReaderRefreshScene::FastContent);
    assert!(!app.platform.reader_hardware_y4_requested());
    assert!(refreshes.take().iter().all(|plan| is_du(plan.kind)));
    assert_eq!(app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0, anchor);
    app.change_reader_setting("refresh-mode:normal")?;
    app.sync_presented_reader_dithering();
    assert_eq!(app.platform.reader_refresh_scene(), ReaderRefreshScene::FastContent);
    assert!(!app.platform.reader_hardware_y4_requested());
    let gray = import_target(&mut app)?;
    assert!(gray.chunks_exact(4).any(|pixel| !matches!(pixel[0], 0 | 255)));
    assert_eq!(app.platform.reader_refresh_scene(), ReaderRefreshScene::Normal);
    assert!(!app.platform.reader_hardware_y4_requested(), "Normal product output is CPU-quantized, not raw Y4 input");
    assert!(refreshes.take().iter().any(|plan| plan.kind == RefreshKind::Full));
    let state = app.reading.as_ref().unwrap();
    let settings = state.core.as_ref().unwrap().settings();
    assert_eq!(settings.spatial_dither, Some(SpatialDither::Bayer8));
    assert!(!settings.hardware_dither);
    assert_eq!((settings.rotation, settings.zoom_ratio), (180, 1.5));
    assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
    assert_eq!(state.saved.every, "6");
    assert!(state.saved.dither);
    assert_eq!(state.saved.dither_method, "bayer8");
    Ok(())
}

#[tokio::test]
async fn fast_overrides_saved_dithering_and_rejects_period_edits() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, _) = fixture(directory.path(), tokio::runtime::Handle::current(), true, false)?;
    let version = app.reading.as_ref().unwrap().version;
    app.change_reader_setting("refresh-policy:1")?;
    assert_eq!(app.reading.as_ref().unwrap().version, version);
    assert_eq!(app.preferences.every, "6");
    assert_eq!(app.reading.as_ref().unwrap().saved.every, "6");
    app.change_reader_setting("dither-method:floyd_steinberg")?;
    assert!(import_target(&mut app)?.chunks_exact(4).all(|p| matches!(p[0], 0 | 255)));
    assert_eq!(app.reading.as_ref().unwrap().saved.dither_method, "floyd_steinberg");
    app.change_reader_setting("dither-method:off")?;
    assert!(import_target(&mut app)?.chunks_exact(4).all(|p| matches!(p[0], 0 | 255)));
    assert!(!app.reading.as_ref().unwrap().saved.dither);
    assert_eq!(app.reading.as_ref().unwrap().saved.dither_method, "floyd_steinberg");
    assert_eq!(app.reading.as_ref().unwrap().core.as_ref().unwrap().settings().spatial_dither, Some(SpatialDither::Off));
    app.change_reader_setting("dither-method:floyd_steinberg")?;
    assert!(import_target(&mut app)?.chunks_exact(4).all(|p| matches!(p[0], 0 | 255)));
    assert!(app.reading.as_ref().unwrap().saved.dither);
    assert!(!app.platform.reader_hardware_y4_requested());
    app.change_reader_setting("refresh-mode:normal")?;
    let normal = import_target(&mut app)?;
    assert!(normal.chunks_exact(4).any(|p| !matches!(p[0], 0 | 255)));
    assert_eq!(app.reading.as_ref().unwrap().core.as_ref().unwrap().settings().spatial_dither,
        Some(SpatialDither::FloydSteinberg));
    assert_eq!(app.reading.as_ref().unwrap().saved.dither_method, "floyd_steinberg");
    assert!(!app.platform.reader_hardware_y4_requested());
    assert_eq!(app.reading.as_ref().unwrap().saved.every, "6");
    Ok(())
}

#[tokio::test]
async fn actual_gray_overlays_never_use_the_binary_content_scene() -> Result<()> {
    for case in ["menu", "sheet", "guidance", "error", "expired", "feedback", "missing", "network", "toast", "transition", "sleep", "outside"] {
        let directory = tempfile::tempdir()?;
        let (mut app, refreshes) = fixture(directory.path(), tokio::runtime::Handle::current(), true, false)?;
        match case {
            "menu" => app.reading.as_mut().unwrap().menu = true,
            "sheet" => app.reading.as_mut().unwrap().sheet = "display".into(),
            "guidance" => app.reading.as_mut().unwrap().guidance = true,
            "error" => app.reading.as_mut().unwrap().error = Some("Synthetic error".into()),
            "expired" => app.reading.as_mut().unwrap().detail.as_mut().unwrap().episodes[0].expires_at = Some(now() - 1),
            "feedback" => {
                let state = app.reading.as_mut().unwrap();
                state.loading = true;
                state.render_started = Some(Instant::now() - Duration::from_secs(1));
            }
            "missing" => {
                let state = app.reading.as_mut().unwrap();
                state.loading = true;
                state.missing_pages = vec![1];
            }
            "network" => app.pending_network = Some(("reader".into(), "retry".into(), String::new())),
            "toast" => app.toast_text("Synthetic toast".into()),
            "transition" => {
                let core = app.reading.as_mut().unwrap().presented_core.as_mut().unwrap();
                core.set_page(2)?;
                assert_eq!(core.step(true), ReaderMove::EndOfEpisode);
            }
            "sleep" => app.sleep_screen.active = true,
            "outside" => app.route = "account".into(),
            _ => unreachable!(),
        }
        app.sync_presented_reader_dithering();
        assert_eq!(app.platform.reader_refresh_scene(), if matches!(case, "sleep" | "outside") {
            ReaderRefreshScene::Normal
        } else { ReaderRefreshScene::FastOverlay }, "Unexpected scene for {case}");
        assert!(!app.platform.reader_hardware_y4_requested());
        assert!(refreshes.take().iter().all(|plan| !is_du(plan.kind)), "Gray overlay used DU for {case}");
    }
    Ok(())
}

#[tokio::test]
async fn overlays_return_to_du_and_retained_feedback_waits_for_actual_visibility() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, refreshes) = fixture(directory.path(), tokio::runtime::Handle::current(), true, false)?;
    let page = app.reading.as_ref().unwrap().page.clone();
    let state = app.reading.as_mut().unwrap();
    state.loading = true;
    state.render_started = Some(Instant::now());
    app.sync_presented_reader_dithering();
    assert_eq!(app.platform.reader_refresh_scene(), ReaderRefreshScene::FastContent);
    app.reading.as_mut().unwrap().render_started = Some(Instant::now() - Duration::from_secs(1));
    app.sync_presented_reader_dithering();
    assert_eq!(app.platform.reader_refresh_scene(), ReaderRefreshScene::FastOverlay);
    let state = app.reading.as_mut().unwrap();
    state.loading = false;
    state.menu = true;
    app.reader_overlay_refresh(true);
    assert_eq!(app.platform.reader_refresh_scene(), ReaderRefreshScene::FastOverlay);
    app.reading.as_mut().unwrap().menu = false;
    app.reader_overlay_refresh(false);
    assert_eq!(app.platform.reader_refresh_scene(), ReaderRefreshScene::FastContent);
    assert_eq!(app.reading.as_ref().unwrap().page, page);
    let plans = refreshes.take();
    assert!(plans.iter().any(|plan| plan.kind == RefreshKind::Ui));
    assert_eq!(plans.last().unwrap().kind, RefreshKind::FastFull);
    assert!(!plans.iter().any(|plan| matches!(plan.kind, RefreshKind::Full | RefreshKind::FlashUi | RefreshKind::FlashPartial)));
    Ok(())
}

#[tokio::test]
async fn fast_turns_and_idle_never_restore_grayscale_or_periodic_full_clearing() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, refreshes) = fixture(directory.path(), tokio::runtime::Handle::current(), true, false)?;
    let page = app.reading.as_ref().unwrap().page.clone();
    let anchor = app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0;
    for policy in ["1", "6", "smart"] {
        app.reading.as_mut().unwrap().saved.every = policy.into();
        for i in 0..20 { app.reader_turn_refresh(if i % 2 == 0 { 0.9 } else { 0.0 }, false, i % 3 == 0); }
    }
    for _ in 0..20 { app.tick()?; }
    assert!(refreshes.take().iter().all(|plan| is_du(plan.kind)));
    assert_eq!(app.platform.reader_refresh_scene(), ReaderRefreshScene::FastContent);
    assert_eq!(app.reading.as_ref().unwrap().page, page);
    assert_eq!(app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0, anchor);
    assert!(app.reading.as_ref().unwrap().presented_core.as_ref().unwrap().settings().fast_mode);
    app.reader_action("refresh", "")?;
    let manual = refreshes.take();
    assert_eq!(manual.iter().filter(|plan| plan.kind == RefreshKind::Full).count(), 1);
    app.reader_turn_refresh(0.9, false, false);
    assert!(refreshes.take().iter().all(|plan| is_du(plan.kind)));
    Ok(())
}

#[tokio::test]
async fn a_normal_prepared_frame_cannot_short_circuit_a_fast_target() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let paths = (0..3).map(|i| {
        let path = directory.path().join(format!("fast-prepared-{i}.png"));
        source().save(&path)?;
        Ok(path)
    }).collect::<Result<Vec<_>>>()?;
    let files = ReaderFiles::Demo { paths, strip: None, strip_active: false };
    let origin = ReaderCore::new(index(), ReaderSettings { landscape_spread: false, ..Default::default() })?;
    let mut target = origin.clone();
    assert_eq!(target.step(true), ReaderMove::Moved);
    let key = NextFrameKey { epoch: 1, generation: 1, origin_version: 1,
        origin: FrameInput::new(&origin), target: FrameInput::new(&target), viewport: Viewport::new(64, 96), landscape: false };
    let next = prepare_next_frame(target.clone(), key, &files, &AtomicBool::new(false), None)?.unwrap();
    assert!(next.frame.page.pixels.as_bytes().chunks_exact(4).any(|p| !matches!(p[0], 0 | 255)));
    let preparation = target.source_preparation(Viewport::new(64, 96));
    let mut settings = target.settings().clone();
    settings.fast_mode = true;
    target.update_settings(settings)?;
    assert_eq!(preparation, target.source_preparation(Viewport::new(64, 96)), "Shared Gray8 source identity must stay independent of final quantization");
    let mut profile = crate::reader::ReaderPreparationProfile::default();
    let outcome = reuse_next_frame(&mut target, next, &files, Viewport::new(64, 96), (1, 1),
        &AtomicBool::new(false), Some(&mut profile), None)?;
    let PreparedOutcome::Ready(frame) = outcome else { anyhow::bail!("A complete source must render"); };
    assert!(frame.page.pixels.as_bytes().chunks_exact(4).all(|p| matches!(p[0], 0 | 255)));
    assert!(target.settings().fast_mode);
    assert_eq!(profile.unchanged_sources, 0);
    assert_eq!(profile.rendered_frames, 1);
    Ok(())
}

#[tokio::test]
async fn ordinary_default_changes_preserve_existing_per_comic_overrides() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, _) = fixture(directory.path(), tokio::runtime::Handle::current(), false, false)?;
    let saved = app.reading.as_ref().unwrap().saved.clone();
    app.store.set_preference(&app.account, "reader_preferences_fast-reader-comic", &saved)?;
    app.reading = None;
    app.preferences.refresh_mode = "fast".into();
    app.save_preferences()?;
    app.reader_refresh_mode_changed()?;
    let actual: SavedReaderPreferences = app.store.preference(&app.account, "reader_preferences_fast-reader-comic")?.unwrap();
    assert_eq!(actual.refresh_mode, "normal");
    assert_eq!(actual.every, "6");
    assert_eq!(actual.dither_method, "bayer8");
    let global: Preferences = app.store.preference(&app.account, "preferences")?.unwrap();
    assert_eq!(global.refresh_mode, "fast");
    Ok(())
}

//! Product CPU dithering, immutable presentation intent, and explicit Raw research guards.

use super::*;
use crate::{platform::{DeviceProfile, DitheringSink}, reader::SpatialDither,
    source::BiliClient, ui::SleepScreenModel};
use std::path::Path;

struct MockDithering {
    available: AtomicBool,
    requested: AtomicBool,
}

impl MockDithering {
    fn new(available: bool) -> Self {
        Self { available: AtomicBool::new(available), requested: AtomicBool::new(false) }
    }

    fn requested(&self) -> bool { self.requested.load(Ordering::Acquire) }
}

impl DitheringSink for MockDithering {
    fn hardware_y4_available(&self) -> bool { self.available.load(Ordering::Acquire) }

    fn set_hardware_y4(&self, enabled: bool) -> bool {
        self.requested.store(enabled, Ordering::Release);
        enabled && self.hardware_y4_available()
    }
}

fn index() -> ImageIndex {
    ImageIndex { episode_id: "hardware-dither-episode".into(), revision: "hardware-dither-revision".into(),
        images: (0..3).map(|position| PageDescriptor { id: format!("hardware-dither-page-{position}"),
            index: position, width: 16, height: 24 }).collect(), ..Default::default() }
}

fn fixture_files(directory: &Path) -> Result<ReaderFiles> {
    let paths = (0..3).map(|position| {
        let path = directory.join(format!("hardware-dither-{position}.png"));
        image::GrayImage::from_fn(16, 24, |x, y| image::Luma([
            ((x * 11 + y * 7 + position * 59) % 256) as u8])).save(&path)?;
        Ok(path)
    }).collect::<Result<Vec<_>>>()?;
    Ok(ReaderFiles::Demo { paths, strip: None, strip_active: false })
}

fn fixture_app(directory: &Path, handle: tokio::runtime::Handle, raw_research: bool)
    -> Result<(App, ReaderFiles, Arc<MockDithering>)>
{
    let mut app = super::super::test_support::app(directory, DeviceProfile::Paperwhite5,
        BiliClient::offline(directory)?, handle)?;
    app.route = "reader".into();
    app.options.offline = true;
    app.preferences.preload = 0;
    app.preferences.dither_method = "bayer8".into();
    let sink = Arc::new(MockDithering::new(true));
    app.platform.set_dithering_sink(sink.clone())?;
    let saved = SavedReaderPreferences::from(&app.preferences);
    let mut settings = reader_settings_for_display(&saved, false, (930, 1240), true);
    if raw_research {
        // Only retained-frame guards opt into the internal Raw Gray8/Y4 path.
        // Stored preferences always remain valid product CPU selections.
        settings.spatial_dither = None;
        settings.hardware_dither = true;
    }
    settings.landscape_spread = false;
    settings.rotation = 180;
    let mut core = ReaderCore::new(index(), settings)?;
    core.set_page(1)?;
    let files = fixture_files(directory)?;
    let PreparedOutcome::Ready(frame) = prepare_reader_frame(&mut core, &files,
        Viewport::new(16, 24), (8, 12), &AtomicBool::new(false))?
        else { anyhow::bail!("The fixture must prepare a real comic frame"); };
    let mut state = ReadingState::pending("hardware-dither-comic".into(), index().episode_id,
        41, saved, false, false);
    state.original_index = Some(index());
    state.core = Some(core.clone());
    state.presented_core = Some(core);
    state.page = raster_image(frame.page);
    state.metadata = frame.metadata;
    state.loading = false;
    app.reading = Some(state);
    app.sync_presented_reader_dithering();
    Ok((app, files, sink))
}

fn import_target(app: &mut App, files: &ReaderFiles) -> Result<()> {
    let mut core = app.reading.as_ref().unwrap().core.clone().unwrap();
    let PreparedOutcome::Ready(frame) = prepare_reader_frame(&mut core, files,
        Viewport::new(16, 24), (8, 12), &AtomicBool::new(false))?
        else { anyhow::bail!("The replacement fixture frame must be ready"); };
    let state = app.reading.as_mut().unwrap();
    let (generation, version) = (state.generation, state.version);
    state.rendering = true;
    state.rendering_version = Some(version);
    state.loading = true;
    app.apply_prepared_reader(generation, version, None,
        Ok(PreparedReader { core, outcome: PreparedOutcome::Ready(frame) }));
    anyhow::ensure!(!app.reading.as_ref().unwrap().loading, "The replacement frame was not imported");
    Ok(())
}

fn next_key(app: &App) -> Result<NextFrameKey> {
    let state = app.reading.as_ref().unwrap();
    let origin = state.presented_core.as_ref().unwrap();
    let mut target = origin.clone();
    anyhow::ensure!(target.step(true) == ReaderMove::Moved, "The fixture needs another source page");
    Ok(NextFrameKey { epoch: app.epoch, generation: state.generation,
        origin_version: state.version, origin: FrameInput::new(origin), target: FrameInput::new(&target),
        viewport: Viewport::new(16, 24), landscape: false })
}

#[test]
fn legacy_preferences_migrate_to_bayer8_and_keep_explicit_disable() -> Result<()> {
    for enabled in [false, true] {
        let legacy = serde_json::json!({"dither": enabled, "direction": "rtl"});
        let preferences: Preferences = serde_json::from_value(legacy.clone())?;
        let saved: SavedReaderPreferences = serde_json::from_value(legacy)?;
        assert_eq!(preferences.dither, enabled);
        assert_eq!(preferences.dither_method, "bayer8");
        assert_eq!(saved.dither, enabled);
        assert_eq!(saved.dither_method, "bayer8");
        assert_eq!(saved.direction, "rtl");
        let settings = reader_settings_for_display(&saved, false, (930, 1240), true);
        assert_eq!(settings.dither, enabled);
        assert_eq!(settings.spatial_dither, Some(if enabled { SpatialDither::Bayer8 } else { SpatialDither::Off }));
        assert!(!settings.hardware_dither);
        assert!(!settings.uses_hardware_dither());
        for method in ["software", "hardware", "unknown-driver-method"] {
            let legacy = serde_json::json!({"dither": enabled, "dither_method": method});
            let preferences: Preferences = serde_json::from_value(legacy.clone())?;
            let saved: SavedReaderPreferences = serde_json::from_value(legacy)?;
            assert_eq!(preferences.dither_method, "bayer8");
            assert_eq!(saved.dither_method, "bayer8");
            assert_eq!((preferences.dither, saved.dither), (enabled, enabled));
            assert_eq!(reader_settings_for_display(&saved, false, (930, 1240), true).spatial_dither,
                Some(if enabled { SpatialDither::Bayer8 } else { SpatialDither::Off }));
        }
    }
    Ok(())
}

#[test]
fn cpu_preferences_round_trip_and_unknown_methods_fall_back_to_bayer8() -> Result<()> {
    for (stored, canonical, algorithm) in [
        ("bayer8", "bayer8", SpatialDither::Bayer8),
        ("floyd_steinberg", "floyd_steinberg", SpatialDither::FloydSteinberg),
        ("floyd_steinberg_q8_v1", "floyd_steinberg", SpatialDither::FloydSteinberg),
    ] {
        let preferences: Preferences = serde_json::from_value(serde_json::json!({
            "dither": true, "dither_method": stored
        }))?;
        let saved = SavedReaderPreferences::from(&preferences);
        let restored: SavedReaderPreferences = serde_json::from_value(serde_json::to_value(&saved)?)?;
        assert_eq!(restored.dither_method, canonical);
        for available in [false, true] {
            let settings = reader_settings_for_display(&restored, false, (930, 1240), available);
            assert_eq!(settings.spatial_dither, Some(algorithm));
            assert!(!settings.hardware_dither && !settings.uses_hardware_dither());
        }
        let disabled = SavedReaderPreferences { dither: false, ..restored };
        let settings = reader_settings_for_display(&disabled, false, (930, 1240), true);
        assert_eq!(settings.spatial_dither, Some(SpatialDither::Off));
        assert!(!settings.dither && !settings.hardware_dither && !settings.uses_hardware_dither());
    }
    let unknown = SavedReaderPreferences { dither_method: "unknown-driver-method".into(), ..Default::default() };
    let settings = reader_settings_for_display(&unknown, false, (930, 1240), true);
    assert!(settings.dither);
    assert_eq!(settings.spatial_dither, Some(SpatialDither::Bayer8));
    assert!(!settings.hardware_dither && !settings.uses_hardware_dither());
    assert_eq!(normalized_dither_method(&unknown.dither_method), "bayer8");
    Ok(())
}

#[test]
fn raw_research_preserves_gray8_and_product_disable_keeps_sixteen_tones() -> Result<()> {
    let source = image::GrayImage::from_fn(256, 1, |x, _| image::Luma([x as u8]));
    let mut descriptor = index();
    descriptor.images.truncate(1);
    descriptor.images[0].width = 256;
    descriptor.images[0].height = 1;
    let settings = ReaderSettings { spatial_dither: None, hardware_dither: true,
        landscape_spread: false, ..Default::default() };
    let mut core = ReaderCore::new(descriptor, settings)?;
    let raw = core.render_source(&source, Viewport::new(256, 1))?;
    assert_eq!(raw.rgba.len(), 256 * 4);
    for (value, pixel) in raw.rgba.chunks_exact(4).enumerate() {
        assert_eq!(pixel, &[value as u8, value as u8, value as u8, 255]);
    }
    let mut disabled = core.settings().clone();
    disabled.dither = false;
    disabled.spatial_dither = Some(SpatialDither::Off);
    core.update_settings(disabled)?;
    assert!(!core.settings().uses_hardware_dither());
    let quantized = core.render_source(&source, Viewport::new(256, 1))?;
    let tones = quantized.rgba.chunks_exact(4).map(|pixel| pixel[0])
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(tones, (0u8..16).map(|level| level * 17)
        .collect::<std::collections::BTreeSet<_>>());
    Ok(())
}

#[tokio::test]
async fn method_changes_follow_imported_pixels_and_preserve_raw_handoff_and_anchor() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, files, sink) = fixture_app(directory.path(), tokio::runtime::Handle::current(), true)?;
    let original_page = app.reading.as_ref().unwrap().page.clone();
    let anchor = app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0;
    let version = app.reading.as_ref().unwrap().version;
    let key = next_key(&app)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    app.reading.as_mut().unwrap().next_frame = NextFrameSlot::Preparing { key, cancelled: cancelled.clone() };
    for method in ["software", "hardware", "unknown-driver-method"] {
        app.change_reader_setting(&format!("dither-method:{method}"))?;
        let state = app.reading.as_ref().unwrap();
        assert_eq!(state.version, version);
        assert_eq!(state.saved.dither_method, "bayer8");
        assert_eq!(app.preferences.dither_method, "bayer8");
        assert!(!cancelled.load(Ordering::Acquire), "An unsupported UI method must not cancel useful preparation");
        assert_eq!(state.page, original_page);
        assert!(sink.requested() && app.platform.reader_hardware_y4_requested());
    }
    app.change_reader_setting("dither-method:floyd_steinberg")?;
    app.sync_presented_reader_dithering();
    let state = app.reading.as_ref().unwrap();
    assert!(cancelled.load(Ordering::Acquire));
    assert_eq!(state.core.as_ref().unwrap().settings().spatial_dither, Some(SpatialDither::FloydSteinberg));
    assert!(!state.core.as_ref().unwrap().settings().uses_hardware_dither());
    assert!(state.presented_core.as_ref().unwrap().settings().uses_hardware_dither());
    assert_eq!(state.page, original_page);
    assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
    assert_eq!(state.thumbnail_dithering(), (true, true, None));
    assert!(sink.requested() && app.platform.reader_hardware_y4_requested());
    import_target(&mut app, &files)?;
    assert!(!sink.requested() && !app.platform.reader_hardware_y4_requested());
    assert_eq!(app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0, anchor);
    assert_eq!(app.reading.as_ref().unwrap().thumbnail_dithering(),
        (true, false, Some(SpatialDither::FloydSteinberg)));

    let cpu_page = app.reading.as_ref().unwrap().page.clone();
    app.change_reader_setting("dither-method:off")?;
    app.sync_presented_reader_dithering();
    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.page, cpu_page);
    assert_eq!(state.presented_core.as_ref().unwrap().settings().spatial_dither, Some(SpatialDither::FloydSteinberg));
    assert_eq!(state.core.as_ref().unwrap().settings().spatial_dither, Some(SpatialDither::Off));
    assert_eq!(state.thumbnail_dithering(), (true, false, Some(SpatialDither::FloydSteinberg)));
    assert!(!sink.requested() && !app.platform.reader_hardware_y4_requested());
    import_target(&mut app, &files)?;
    assert!(!sink.requested() && !app.platform.reader_hardware_y4_requested());
    let state = app.reading.as_ref().unwrap();
    assert!(!state.presented_core.as_ref().unwrap().settings().dither);
    assert_eq!(state.presented_core.as_ref().unwrap().settings().spatial_dither, Some(SpatialDither::Off));
    assert_eq!(state.saved.dither_method, "floyd_steinberg", "Off must preserve the selected CPU method");
    assert_eq!(state.thumbnail_dithering(), (false, false, Some(SpatialDither::Off)));
    assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
    Ok(())
}

#[tokio::test]
async fn capability_loss_leaves_cpu_pixels_version_and_prefetch_unchanged() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, _, sink) = fixture_app(directory.path(), tokio::runtime::Handle::current(), false)?;
    let cpu_page = app.reading.as_ref().unwrap().page.clone();
    let anchor = app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0;
    let version = app.reading.as_ref().unwrap().version;
    let settings = app.reading.as_ref().unwrap().core.as_ref().unwrap().settings().clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let key = next_key(&app)?;
    app.reading.as_mut().unwrap().next_frame = NextFrameSlot::Preparing { key: key.clone(), cancelled: cancelled.clone() };
    for available in [false, true] {
        sink.available.store(available, Ordering::Release);
        app.reconcile_reader_dithering()?;
        let state = app.reading.as_ref().unwrap();
        assert_eq!(state.version, version);
        assert!(!cancelled.load(Ordering::Acquire));
        assert!(matches!(&state.next_frame, NextFrameSlot::Preparing { key: active, cancelled: token }
            if active == &key && Arc::ptr_eq(token, &cancelled)));
        assert_eq!(state.core.as_ref().unwrap().settings(), &settings);
        assert_eq!(state.presented_core.as_ref().unwrap().settings(), &settings);
        assert_eq!(settings.spatial_dither, Some(SpatialDither::Bayer8));
        assert_eq!(state.page, cpu_page);
        assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
        assert!(!state.loading && !state.rendering);
        assert_eq!(state.saved.dither_method, "bayer8");
        assert!(!sink.requested() && !app.platform.reader_hardware_y4_requested());
    }
    Ok(())
}

#[tokio::test]
async fn prepared_keys_and_thumbnail_derivatives_separate_cpu_methods_and_raster_versions() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, files, _) = fixture_app(directory.path(), tokio::runtime::Handle::current(), false)?;
    let key = next_key(&app)?;
    let origin = app.reading.as_ref().unwrap().presented_core.as_ref().unwrap().clone();
    assert!(key.origin_is_current(app.reading.as_ref().unwrap(), app.epoch, key.viewport, false));
    let mut different = origin.clone();
    let mut settings = different.settings().clone();
    settings.spatial_dither = Some(SpatialDither::FloydSteinberg);
    different.update_settings(settings)?;
    assert!(!FrameInput::new(&origin).matches(&different));
    app.reading.as_mut().unwrap().core = Some(different);
    assert!(!key.origin_is_current(app.reading.as_ref().unwrap(), app.epoch, key.viewport, false));
    let mut target = origin.clone();
    assert_eq!(target.step(true), ReaderMove::Moved);
    let state = app.reading.as_mut().unwrap();
    state.core = Some(target.clone());
    state.version = key.origin_version.wrapping_add(1);
    assert!(key.target_is_current(state, app.epoch, key.viewport, false));
    let mut settings = target.settings().clone();
    settings.spatial_dither = Some(SpatialDither::FloydSteinberg);
    target.update_settings(settings)?;
    state.core = Some(target);
    assert!(!key.target_is_current(state, app.epoch, key.viewport, false));
    state.core = Some(origin);
    state.version = key.origin_version;

    let source = files.source(&index(), 1, None, &AtomicBool::new(false))?
        .flatten().context("The thumbnail fixture source must exist")?;
    app.reading.as_mut().unwrap().sheet = "grid".into();
    app.request_reader_thumbnails()?;
    let original_plan = app.reading.as_ref().unwrap().thumbnail_plan.clone().unwrap();
    assert_eq!(original_plan.spatial_dither, Some(SpatialDither::Bayer8));
    assert_eq!(original_plan.raster_version, READER_RASTER_VERSION);
    app.reading.as_mut().unwrap().thumbnail_proofs.insert(1, source.proof().clone());
    app.change_reader_setting("dither-method:floyd_steinberg")?;
    app.sync_presented_reader_dithering();
    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.thumbnail_dithering(), (true, false, Some(SpatialDither::Bayer8)));
    assert!(state.thumbnail_proofs.contains_key(&1));
    assert!(state.thumbnail_plan.as_ref() == Some(&original_plan));
    import_target(&mut app, &files)?;
    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.thumbnail_dithering(), (true, false, Some(SpatialDither::FloydSteinberg)));
    let mut expected = original_plan;
    expected.spatial_dither = Some(SpatialDither::FloydSteinberg);
    assert!(state.thumbnail_plan.as_ref() == Some(&expected));
    assert!(state.thumbnail_proofs.is_empty(), "Source identity alone cannot reuse a differently processed thumbnail");
    let state = app.reading.as_mut().unwrap();
    state.thumbnail_proofs.insert(1, source.proof().clone());
    state.thumbnail_plan.as_mut().unwrap().raster_version = READER_RASTER_VERSION - 1;
    app.sync_presented_reader_dithering();
    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.thumbnail_plan.as_ref().unwrap().raster_version, READER_RASTER_VERSION);
    assert_eq!(state.thumbnail_plan.as_ref().unwrap().spatial_dither, Some(SpatialDither::FloydSteinberg));
    assert!(state.thumbnail_proofs.is_empty(), "A stale raster version cannot reuse a source-only thumbnail proof");
    Ok(())
}

#[tokio::test]
async fn a_pending_cpu_setting_does_not_change_the_sleep_retained_raw_frame() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, _, sink) = fixture_app(directory.path(), tokio::runtime::Handle::current(), true)?;
    let raw_page = app.reading.as_ref().unwrap().page.clone();
    let anchor = app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0;
    app.change_reader_setting("dither-method:floyd_steinberg")?;
    app.begin_suspend_intent_gate();
    app.sync_presented_reader_dithering();
    let state = app.reading.as_ref().unwrap();
    assert!(app.is_suspended());
    assert!(state.render_after_sleep);
    assert_eq!(state.core.as_ref().unwrap().settings().spatial_dither, Some(SpatialDither::FloydSteinberg));
    assert!(!state.core.as_ref().unwrap().settings().uses_hardware_dither());
    assert!(state.presented_core.as_ref().unwrap().settings().uses_hardware_dither());
    assert_eq!(state.page, raw_page);
    assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
    assert!(sink.requested() && app.platform.reader_hardware_y4_requested());
    Ok(())
}

struct HeadlessPlatform { window: Rc<slint::platform::software_renderer::MinimalSoftwareWindow> }

impl slint::platform::Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> std::result::Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

fn with_ui(work: impl FnOnce(&AppWindow) -> Result<()> + Send + 'static) -> Result<()> {
    std::thread::spawn(move || {
        let window = slint::platform::software_renderer::MinimalSoftwareWindow::new(
            slint::platform::software_renderer::RepaintBufferType::ReusedBuffer);
        slint::platform::set_platform(Box::new(HeadlessPlatform { window }))?;
        let ui = AppWindow::new()?;
        work(&ui)
    }).join().map_err(|_| anyhow::anyhow!("The isolated hardware-dither UI fixture panicked"))?
}

#[test]
fn sleep_overlay_handoff_restores_raw_processing_before_the_application_resumes() -> Result<()> {
    with_ui(|ui| {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_time().build()?;
        let directory = tempfile::tempdir()?;
        let (mut app, _, sink) = fixture_app(directory.path(), runtime.handle().clone(), true)?;
        app.ui = ui.as_weak();
        let raw_page = app.reading.as_ref().unwrap().page.clone();
        let anchor = app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0;
        ui.global::<ReaderModel>().set_page(raw_page.clone());
        app.begin_suspend_intent_gate();
        assert!(app.is_suspended());
        assert!(sink.requested());
        let sequence = app.sequence;
        let pending = app.pending.len();
        app.show_sleep_screen()?;
        assert!(ui.global::<SleepScreenModel>().get_active());
        assert!(!sink.requested() && !app.platform.reader_hardware_y4_requested());
        app.hide_sleep_screen();
        assert!(app.is_suspended(), "Native display resume happens before App::resume");
        assert!(!ui.global::<SleepScreenModel>().get_active());
        assert!(sink.requested() && app.platform.reader_hardware_y4_requested());
        assert_eq!(ui.global::<ReaderModel>().get_page(), raw_page);
        assert_eq!(app.reading.as_ref().unwrap().page, raw_page);
        assert_eq!(app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0, anchor);
        assert_eq!(app.sequence, sequence);
        assert_eq!(app.pending.len(), pending);
        Ok(())
    })
}

#[test]
fn idle_episode_expiry_hides_the_raw_page_in_the_same_tick_that_releases_y4() -> Result<()> {
    with_ui(|ui| {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_time().build()?;
        let directory = tempfile::tempdir()?;
        let (mut app, _, sink) = fixture_app(directory.path(), runtime.handle().clone(), true)?;
        app.ui = ui.as_weak();
        app.preferences.sleep_cover = false;
        // Fixture mode suppresses unrelated disk-maintenance workers. No source
        // pages, network transport, or delayed job completion can dirty the view.
        app.demo = Some(Arc::new(DemoData { comics: Vec::new(), details: Vec::new(), indices: vec![index()],
            pages: BTreeMap::new(), covers: BTreeMap::new(), account_name: "Synthetic expiry account".into(),
            fixture: serde_json::json!({}), panels: BTreeMap::new(), strip: None, strip_panels: Vec::new() }));
        let state = app.reading.as_mut().unwrap();
        state.detail = Some(ComicDetail { comic: crate::domain::Comic {
            id: state.comic_id.clone(), title: "Synthetic temporary comic".into(), ..Default::default()
        }, episodes: vec![Episode { id: state.episode_id.clone(), comic_id: state.comic_id.clone(),
            access: EpisodeAccess::Temporary, expires_at: Some(now() + 60), ..Default::default()
        }], ..Default::default() });
        // The target already follows product CPU preferences while the old Raw
        // research frame remains visible. Idle reconciliation must not mistake
        // normalization of an internal Raw target for a capability transition.
        let mut settings = state.core.as_ref().unwrap().settings().clone();
        settings.spatial_dither = Some(SpatialDither::Bayer8);
        settings.hardware_dither = false;
        state.core.as_mut().unwrap().update_settings(settings)?;
        let version = state.version;
        let raw_page = state.page.clone();
        app.refresh_reader_view(ui)?;
        let view = ui.global::<ReaderModel>();
        assert!(!view.get_failed() && !view.get_loading() && !view.get_transition());
        assert_eq!(view.get_page(), raw_page);
        assert!(sink.requested() && app.platform.reader_hardware_y4_requested());
        app.dirty = false;
        for route in ["shelf", "account", "purchase", "downloads", "reader"] {
            let _ = app.view_dirty.take(route);
        }
        assert!(!app.view_dirty.needs_route("reader"));
        assert!(app.pending.is_empty() && app.jobs.take().is_none());
        app.reading.as_mut().unwrap().detail.as_mut().unwrap().episodes[0].expires_at = Some(now() - 1);

        app.tick()?;

        assert!(!sink.requested() && !app.platform.reader_hardware_y4_requested());
        assert!(view.get_failed(), "The same idle tick must hide the expired raw image");
        assert!(!view.get_retain_page() && !view.get_loading());
        assert_eq!(app.reading.as_ref().unwrap().page, raw_page,
            "Expiry hides the image without destroying the retained reading session");
        assert_eq!(app.reading.as_ref().unwrap().version, version);
        assert!(app.pending.is_empty() && app.jobs.take().is_none());
        Ok(())
    })
}

#[tokio::test]
async fn leaving_the_reader_or_clearing_its_account_releases_the_processing_intent() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, _, sink) = fixture_app(directory.path(), tokio::runtime::Handle::current(), true)?;
    assert!(sink.requested());
    app.route = "account".into();
    app.sync_presented_reader_dithering();
    assert!(!sink.requested());
    app.route = "reader".into();
    app.sync_presented_reader_dithering();
    assert!(sink.requested());
    app.reading = None;
    app.sync_presented_reader_dithering();
    assert!(!sink.requested() && !app.platform.reader_hardware_y4_requested());
    Ok(())
}

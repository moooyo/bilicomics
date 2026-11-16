//! Product CPU choices, durable migration, and presentation-bound raster identity.

use super::*;
use crate::{
    platform::{DeviceProfile, DitheringSink, PlatformError, RefreshSink},
    reader::SpatialDither,
    source::BiliClient,
    store::Store,
};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, atomic::AtomicUsize},
};

const ACCOUNT: &str = "spatial-dither-fixture";
const COMIC: &str = "spatial-dither-comic";
const SOURCE_WIDTH: u32 = 64;
const SOURCE_HEIGHT: u32 = 96;

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

#[derive(Default)]
struct AvailableHardwareDither {
    requested: AtomicBool,
    enabled_requests: AtomicUsize,
}

impl DitheringSink for AvailableHardwareDither {
    fn hardware_y4_available(&self) -> bool { true }

    fn set_hardware_y4(&self, enabled: bool) -> bool {
        self.requested.store(enabled, Ordering::Release);
        if enabled { self.enabled_requests.fetch_add(1, Ordering::AcqRel); }
        enabled
    }
}

struct Fixture {
    app: App,
    files: ReaderFiles,
    viewport: Viewport,
    refreshes: Arc<RecordedRefreshes>,
    hardware: Arc<AvailableHardwareDither>,
}

fn reader_preference_key() -> String { format!("reader_preferences_{COMIC}") }

fn index() -> ImageIndex {
    ImageIndex { episode_id: "spatial-dither-episode".into(), revision: "spatial-dither-revision".into(),
        images: (0..3).map(|position| PageDescriptor { id: format!("spatial-dither-page-{position}"),
            index: position, width: SOURCE_WIDTH, height: SOURCE_HEIGHT }).collect(), ..Default::default() }
}

fn source(position: usize) -> image::GrayImage {
    image::GrayImage::from_fn(SOURCE_WIDTH, SOURCE_HEIGHT, |x, y| {
        image::Luma([((x * 11 + y * 7 + x * y + position as u32 * 59) % 256) as u8])
    })
}

fn files(directory: &Path) -> Result<ReaderFiles> {
    let paths = (0..3).map(|position| {
        let path: PathBuf = directory.join(format!("spatial-dither-{position}.png"));
        source(position).save(&path)?;
        Ok(path)
    }).collect::<Result<Vec<_>>>()?;
    Ok(ReaderFiles::Demo { paths, strip: None, strip_active: false })
}

fn preferences(fast: bool) -> Preferences {
    Preferences { preload: 0, every: "6".into(), landscape: "single".into(),
        refresh_mode: if fast { "fast" } else { "normal" }.into(),
        dither: true, dither_method: "bayer8".into(), ..Default::default() }
}

fn build_fixture(directory: &Path, handle: tokio::runtime::Handle,
    mut preferences: Preferences, saved: SavedReaderPreferences) -> Result<Fixture>
{
    let mut app = super::super::test_support::app(directory, DeviceProfile::Paperwhite5,
        BiliClient::offline(directory)?, handle)?;
    app.account = ACCOUNT.into();
    app.route = "reader".into();
    app.options.offline = true;
    preferences.preload = 0;
    app.preferences = preferences;
    let refreshes = Arc::new(RecordedRefreshes::default());
    let hardware = Arc::new(AvailableHardwareDither::default());
    app.platform.set_refresh_sink(refreshes.clone())?;
    app.platform.set_dithering_sink(hardware.clone())?;
    let logical = app.platform.logical_viewport_size();
    let mut settings = reader_settings_for_display(&saved, false, logical, true);
    settings.landscape_spread = false;
    settings.rotation = 180;
    settings.zoom_ratio = 1.5;
    let viewport = app.reader_render_viewport(&settings);
    let mut core = ReaderCore::new(index(), settings)?;
    core.set_page(1)?;
    let files = files(directory)?;
    let PreparedOutcome::Ready(frame) = prepare_reader_frame(&mut core, &files, viewport,
        (1, 1), &AtomicBool::new(false))? else { anyhow::bail!("The complete fixture must render"); };
    let mut state = ReadingState::pending(COMIC.into(), index().episode_id.clone(),
        41, saved, false, false);
    state.detail = Some(ComicDetail { episodes: vec![Episode { id: index().episode_id,
        access: EpisodeAccess::Free, ..Default::default() }], ..Default::default() });
    state.original_index = Some(index());
    state.core = Some(core.clone());
    state.presented_core = Some(core);
    state.page = raster_image(frame.page);
    state.metadata = frame.metadata;
    state.requested_viewport = Some(viewport);
    state.presented_viewport = Some(viewport);
    state.loading = false;
    app.reading = Some(state);
    app.configure_reader_refresh()?;
    app.sync_presented_reader_dithering();
    app.save_preferences()?;
    app.store.set_preference(ACCOUNT, &reader_preference_key(), &app.reading.as_ref().unwrap().saved)?;
    refreshes.take();
    Ok(Fixture { app, files, viewport, refreshes, hardware })
}

fn fixture(directory: &Path, handle: tokio::runtime::Handle, fast: bool) -> Result<Fixture> {
    let preferences = preferences(fast);
    let saved = SavedReaderPreferences::from(&preferences);
    build_fixture(directory, handle, preferences, saved)
}

fn result_for(core: &ReaderCore, files: &ReaderFiles, viewport: Viewport) -> Result<PreparedReader> {
    let mut core = core.clone();
    let outcome = prepare_reader_frame(&mut core, files, viewport, (1, 1), &AtomicBool::new(false))?;
    Ok(PreparedReader { core, outcome })
}

fn import_target(fixture: &mut Fixture) -> Result<Vec<u8>> {
    let core = fixture.app.reading.as_ref().unwrap().core.as_ref().unwrap();
    let result = result_for(core, &fixture.files, fixture.viewport)?;
    let pixels = match &result.outcome {
        PreparedOutcome::Ready(frame) => frame.page.pixels.as_bytes().to_vec(),
        _ => anyhow::bail!("The replacement fixture must render a complete frame"),
    };
    let state = fixture.app.reading.as_mut().unwrap();
    let (generation, version) = (state.generation, state.version);
    state.rendering = true;
    state.rendering_version = Some(version);
    state.rendering_viewport = Some(fixture.viewport);
    state.loading = true;
    fixture.app.apply_prepared_reader(generation, version, None, Ok(result));
    assert!(!fixture.app.reading.as_ref().unwrap().loading);
    Ok(pixels)
}

fn assert_product_method(settings: &ReaderSettings, expected: SpatialDither) {
    assert!(settings.spatial_dither.is_some(), "Product settings cannot enter the research transport");
    assert_eq!(settings.effective_spatial_dither(), Some(expected));
    assert!(!settings.hardware_dither);
    assert!(!settings.uses_hardware_dither());
}

fn assert_no_y4(fixture: &Fixture) {
    assert!(!fixture.app.platform.reader_hardware_y4_requested());
    assert!(!fixture.hardware.requested.load(Ordering::Acquire));
    assert_eq!(fixture.hardware.enabled_requests.load(Ordering::Acquire), 0);
}

fn assert_palette(rgba: &[u8]) {
    for pixel in rgba.chunks_exact(4) {
        assert_eq!(pixel, &[pixel[0], pixel[0], pixel[0], 255]);
        assert_eq!(pixel[0] % 17, 0);
    }
}

fn assert_binary(rgba: &[u8]) {
    assert_palette(rgba);
    assert!(rgba.chunks_exact(4).all(|pixel| matches!(pixel[0], 0 | 255)));
}

fn is_du(kind: RefreshKind) -> bool { matches!(kind, RefreshKind::Fast | RefreshKind::FastFull) }

#[tokio::test]
async fn legacy_global_and_comic_records_migrate_to_bayer_without_enabling_off() -> Result<()> {
    assert_eq!(Preferences::default().dither_method, "bayer8");
    assert_eq!(SavedReaderPreferences::default().dither_method, "bayer8");
    for legacy in ["software", "hardware", "unknown-method"] {
        for enabled in [false, true] {
            let directory = tempfile::tempdir()?;
            let store = Store::open(directory.path().join("state.sqlite3"))?;
            let raw = serde_json::json!({"dither":enabled,"dither_method":legacy,"every":"6"});
            store.set_preference(ACCOUNT, "preferences", &raw)?;
            store.set_preference(ACCOUNT, &reader_preference_key(), &raw)?;
            let global: Preferences = store.preference(ACCOUNT, "preferences")?.unwrap();
            let saved: SavedReaderPreferences = store.preference(ACCOUNT, &reader_preference_key())?.unwrap();
            assert_eq!(global.dither, enabled);
            assert_eq!(saved.dither, enabled);
            assert_eq!(global.dither_method, "bayer8");
            assert_eq!(saved.dither_method, "bayer8");
            assert_eq!(SavedReaderPreferences::from(&global).dither, enabled);
            let settings = reader_settings_for_display(&saved, false, (618, 824), true);
            assert_product_method(&settings, if enabled { SpatialDither::Bayer8 } else { SpatialDither::Off });
            store.set_preference(ACCOUNT, "preferences", &global)?;
            store.set_preference(ACCOUNT, &reader_preference_key(), &saved)?;
            drop(store);
            let restarted = Store::open(directory.path().join("state.sqlite3"))?;
            let global: Preferences = restarted.preference(ACCOUNT, "preferences")?.unwrap();
            let saved: SavedReaderPreferences = restarted.preference(ACCOUNT, &reader_preference_key())?.unwrap();
            assert_eq!((global.dither, saved.dither), (enabled, enabled));
            assert_eq!(global.dither_method, "bayer8");
            assert_eq!(saved.dither_method, "bayer8");
        }
    }
    for method in ["bayer8", "floyd_steinberg"] {
        let raw = serde_json::json!({"dither":true,"dither_method":method});
        let global: Preferences = serde_json::from_value(raw.clone())?;
        let saved: SavedReaderPreferences = serde_json::from_value(raw)?;
        assert_eq!(global.dither_method, method);
        assert_eq!(saved.dither_method, method);
    }
    Ok(())
}

#[tokio::test]
async fn disabled_legacy_preferences_stay_off_after_app_store_restart() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let raw = serde_json::json!({"dither":false,"dither_method":"hardware","every":"6"});
    let store = Store::open(directory.path().join("state.sqlite3"))?;
    store.set_preference(ACCOUNT, "preferences", &raw)?;
    store.set_preference(ACCOUNT, &reader_preference_key(), &raw)?;
    let preferences: Preferences = store.preference(ACCOUNT, "preferences")?.unwrap();
    let saved: SavedReaderPreferences = store.preference(ACCOUNT, &reader_preference_key())?.unwrap();
    drop(store);
    let first = build_fixture(directory.path(), tokio::runtime::Handle::current(), preferences, saved)?;
    assert_product_method(first.app.reading.as_ref().unwrap().presented_core.as_ref().unwrap().settings(), SpatialDither::Off);
    assert_no_y4(&first);
    drop(first);
    let store = Store::open(directory.path().join("state.sqlite3"))?;
    let preferences: Preferences = store.preference(ACCOUNT, "preferences")?.unwrap();
    let saved: SavedReaderPreferences = store.preference(ACCOUNT, &reader_preference_key())?.unwrap();
    assert!(!preferences.dither && !saved.dither);
    drop(store);
    let mut restarted = build_fixture(directory.path(), tokio::runtime::Handle::current(), preferences, saved)?;
    let settings = restarted.app.reading.as_ref().unwrap().core.as_ref().unwrap().settings();
    assert_product_method(settings, SpatialDither::Off);
    assert_no_y4(&restarted);
    restarted.app.change_reader_setting("dither:on")?;
    import_target(&mut restarted)?;
    let state = restarted.app.reading.as_ref().unwrap();
    assert!(state.saved.dither && restarted.app.preferences.dither);
    assert_eq!(state.saved.dither_method, "bayer8");
    assert_product_method(state.presented_core.as_ref().unwrap().settings(), SpatialDither::Bayer8);
    assert_no_y4(&restarted);
    Ok(())
}

#[tokio::test]
async fn three_choices_round_trip_both_preferences_and_off_restores_the_last_method() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut fixture = fixture(directory.path(), tokio::runtime::Handle::current(), false)?;
    fixture.app.preferences.jpeg_acceleration = true;
    fixture.app.reading.as_mut().unwrap().saved.jpeg_acceleration = true;
    for (command, enabled, remembered, effective) in [
        ("dither-method:floyd_steinberg", true, "floyd_steinberg", SpatialDither::FloydSteinberg),
        ("dither-method:off", false, "floyd_steinberg", SpatialDither::Off),
        ("dither:on", true, "floyd_steinberg", SpatialDither::FloydSteinberg),
        ("dither-method:bayer8", true, "bayer8", SpatialDither::Bayer8),
    ] {
        fixture.app.change_reader_setting(command)?;
        let global: Preferences = fixture.app.store.preference(ACCOUNT, "preferences")?.unwrap();
        let saved: SavedReaderPreferences = fixture.app.store.preference(ACCOUNT, &reader_preference_key())?.unwrap();
        assert_eq!((global.dither, saved.dither), (enabled, enabled));
        assert_eq!(global.dither_method, remembered);
        assert_eq!(saved.dither_method, remembered);
        assert!(global.jpeg_acceleration && saved.jpeg_acceleration);
        assert_eq!((global.every.as_str(), saved.every.as_str()), ("6", "6"));
        let pixels = import_target(&mut fixture)?;
        assert_palette(&pixels);
        assert_product_method(fixture.app.reading.as_ref().unwrap().presented_core.as_ref().unwrap().settings(), effective);
        assert_no_y4(&fixture);
    }
    Ok(())
}

#[tokio::test]
async fn account_choices_persist_defaults_without_rewriting_a_closed_comic_override() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut fixture = fixture(directory.path(), tokio::runtime::Handle::current(), false)?;
    fixture.app.reading = None;
    fixture.app.route = "account".into();
    for (method, enabled, remembered) in [
        ("floyd_steinberg", true, "floyd_steinberg"),
        ("off", false, "floyd_steinberg"),
        ("bayer8", true, "bayer8"),
    ] {
        fixture.app.account_action("dither-method", method)?;
        let global: Preferences = fixture.app.store.preference(ACCOUNT, "preferences")?.unwrap();
        let saved: SavedReaderPreferences = fixture.app.store.preference(ACCOUNT, &reader_preference_key())?.unwrap();
        assert_eq!(global.dither, enabled);
        assert_eq!(global.dither_method, remembered);
        assert_eq!(global.every, "6");
        assert!(saved.dither);
        assert_eq!(saved.dither_method, "bayer8");
        assert_eq!(saved.every, "6");
    }
    assert_no_y4(&fixture);
    Ok(())
}

#[tokio::test]
async fn normal_algorithm_changes_retain_the_presented_frame_until_matching_pixels_arrive() -> Result<()> {
    for (command, effective) in [("dither-method:floyd_steinberg", SpatialDither::FloydSteinberg),
                                 ("dither-method:off", SpatialDither::Off)] {
        let directory = tempfile::tempdir()?;
        let mut fixture = fixture(directory.path(), tokio::runtime::Handle::current(), false)?;
        let state = fixture.app.reading.as_ref().unwrap();
        let page = state.page.clone();
        let anchor = state.displayed_anchor(0).unwrap().0;
        let before = FrameInput::new(state.core.as_ref().unwrap());
        let version = state.version;
        let mut target = state.presented_core.as_ref().unwrap().clone();
        assert_eq!(target.step(true), ReaderMove::Moved);
        let key = NextFrameKey { epoch: fixture.app.epoch, generation: state.generation,
            origin_version: version, origin: before.clone(), target: FrameInput::new(&target),
            viewport: fixture.viewport, landscape: false };
        let cancelled = Arc::new(AtomicBool::new(false));
        fixture.app.reading.as_mut().unwrap().next_frame = NextFrameSlot::Preparing {
            key, cancelled: cancelled.clone() };
        fixture.app.change_reader_setting(command)?;
        fixture.app.sync_presented_reader_dithering();
        let state = fixture.app.reading.as_ref().unwrap();
        assert!(cancelled.load(Ordering::Acquire));
        assert_eq!(state.version, version.wrapping_add(1));
        assert_eq!(state.page, page);
        assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
        assert_product_method(state.presented_core.as_ref().unwrap().settings(), SpatialDither::Bayer8);
        assert_product_method(state.core.as_ref().unwrap().settings(), effective);
        assert!(!before.matches(state.core.as_ref().unwrap()));
        assert_eq!(fixture.app.platform.reader_refresh_scene(), ReaderRefreshScene::Normal);
        assert_no_y4(&fixture);
        let pixels = import_target(&mut fixture)?;
        assert_palette(&pixels);
        let state = fixture.app.reading.as_ref().unwrap();
        assert_product_method(state.presented_core.as_ref().unwrap().settings(), effective);
        assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
        assert_eq!((state.presented_core.as_ref().unwrap().settings().rotation,
                    state.presented_core.as_ref().unwrap().settings().zoom_ratio), (180, 1.5));
        assert_no_y4(&fixture);
    }
    Ok(())
}

#[tokio::test]
async fn a_late_old_algorithm_result_cannot_replace_the_pending_or_published_selection() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut fixture = fixture(directory.path(), tokio::runtime::Handle::current(), false)?;
    let state = fixture.app.reading.as_ref().unwrap();
    let old_core = state.core.as_ref().unwrap().clone();
    let (generation, version) = (state.generation, state.version);
    let page = state.page.clone();
    let anchor = state.displayed_anchor(0).unwrap().0;
    let old_result = result_for(&old_core, &fixture.files, fixture.viewport)?;
    fixture.app.change_reader_setting("dither-method:floyd_steinberg")?;
    let state = fixture.app.reading.as_mut().unwrap();
    state.rendering = true;
    state.rendering_version = Some(version);
    state.rendering_viewport = Some(fixture.viewport);
    fixture.app.apply_prepared_reader(generation, version, None, Ok(old_result));
    let state = fixture.app.reading.as_ref().unwrap();
    assert_eq!(state.page, page);
    assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
    assert_product_method(state.core.as_ref().unwrap().settings(), SpatialDither::FloydSteinberg);
    assert_product_method(state.presented_core.as_ref().unwrap().settings(), SpatialDither::Bayer8);
    import_target(&mut fixture)?;
    let current_page = fixture.app.reading.as_ref().unwrap().page.clone();
    let old_result = result_for(&old_core, &fixture.files, fixture.viewport)?;
    fixture.app.apply_prepared_reader(generation, version, None, Ok(old_result));
    let state = fixture.app.reading.as_ref().unwrap();
    assert_eq!(state.page, current_page);
    assert_product_method(state.presented_core.as_ref().unwrap().settings(), SpatialDither::FloydSteinberg);
    assert_no_y4(&fixture);
    Ok(())
}

#[tokio::test]
async fn a_complete_old_physical_viewport_cannot_publish_the_new_algorithm() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut fixture = fixture(directory.path(), tokio::runtime::Handle::current(), false)?;
    let page = fixture.app.reading.as_ref().unwrap().page.clone();
    fixture.app.change_reader_setting("dither-method:floyd_steinberg")?;
    let state = fixture.app.reading.as_ref().unwrap();
    let (generation, version) = (state.generation, state.version);
    let old_viewport = Viewport { width: fixture.viewport.width + 1, ..fixture.viewport };
    let result = result_for(state.core.as_ref().unwrap(), &fixture.files, old_viewport)?;
    let state = fixture.app.reading.as_mut().unwrap();
    state.rendering = true;
    state.rendering_version = Some(version);
    state.rendering_viewport = Some(old_viewport);
    fixture.app.apply_prepared_reader(generation, version, None, Ok(result));
    let state = fixture.app.reading.as_ref().unwrap();
    assert_eq!(state.page, page);
    assert!(state.version > version);
    assert_product_method(state.presented_core.as_ref().unwrap().settings(), SpatialDither::Bayer8);
    assert_product_method(state.core.as_ref().unwrap().settings(), SpatialDither::FloydSteinberg);
    import_target(&mut fixture)?;
    let state = fixture.app.reading.as_ref().unwrap();
    assert_eq!(state.presented_viewport, Some(fixture.viewport));
    assert_product_method(state.presented_core.as_ref().unwrap().settings(), SpatialDither::FloydSteinberg);
    assert_no_y4(&fixture);
    Ok(())
}

#[test]
fn final_identity_contains_the_effective_method_and_both_algorithm_versions() -> Result<()> {
    for method in [SpatialDither::Off, SpatialDither::Bayer8, SpatialDither::FloydSteinberg] {
        let core = ReaderCore::new(index(), ReaderSettings { spatial_dither: Some(method),
            landscape_spread: false, ..Default::default() })?;
        let input = FrameInput::new(&core);
        assert_eq!(input.raster_version, READER_RASTER_VERSION);
        assert_eq!(input.dither_identity, method.cache_key());
        assert!(input.matches(&core));
        let mut stale = input.clone();
        stale.raster_version = stale.raster_version.wrapping_add(1);
        assert!(!stale.matches(&core));
        let mut stale = input;
        stale.dither_identity = "obsolete-spatial-algorithm-version";
        assert!(!stale.matches(&core));
    }
    let disabled = ReaderCore::new(index(), ReaderSettings { dither: false,
        spatial_dither: Some(SpatialDither::FloydSteinberg), ..Default::default() })?;
    assert_eq!(FrameInput::new(&disabled).dither_identity, SpatialDither::Off.cache_key());
    Ok(())
}

#[test]
fn prepared_pixels_never_cross_methods_or_final_versions_but_gray8_preparation_is_shared() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let files = files(directory.path())?;
    let viewport = Viewport::new(64, 96);
    for mismatch in ["floyd", "off", "raster-version", "algorithm-version"] {
        let mut origin = ReaderCore::new(index(), ReaderSettings { spatial_dither: Some(SpatialDither::Bayer8),
            landscape_spread: false, ..Default::default() })?;
        let PreparedOutcome::Ready(_) = prepare_reader_frame(&mut origin, &files, viewport,
            (1, 1), &AtomicBool::new(false))? else { anyhow::bail!("The origin must render"); };
        let mut target = origin.clone();
        assert_eq!(target.step(true), ReaderMove::Moved);
        let key = NextFrameKey { epoch: 1, generation: 1, origin_version: 1,
            origin: FrameInput::new(&origin), target: FrameInput::new(&target), viewport, landscape: false };
        let mut next = prepare_next_frame(target.clone(), key, &files, &AtomicBool::new(false), None)?
            .context("The complete source must create a prepared candidate")?;
        let preparation = target.source_preparation(viewport);
        match mismatch {
            "floyd" => {
                let mut settings = target.settings().clone();
                settings.spatial_dither = Some(SpatialDither::FloydSteinberg);
                target.update_settings(settings)?;
            }
            "off" => {
                let mut settings = target.settings().clone();
                settings.dither = false;
                target.update_settings(settings)?;
            }
            "raster-version" => next.key.target.raster_version = next.key.target.raster_version.wrapping_add(1),
            "algorithm-version" => next.key.target.dither_identity = "obsolete-spatial-algorithm-version",
            _ => unreachable!(),
        }
        assert_eq!(target.source_preparation(viewport), preparation);
        assert!(!next.key.target.matches(&target));
        let mut expected = target.clone();
        let PreparedOutcome::Ready(expected) = prepare_reader_frame(&mut expected, &files, viewport,
            (1, 1), &AtomicBool::new(false))? else { anyhow::bail!("The current method must render"); };
        let mut profile = crate::reader::ReaderPreparationProfile::default();
        let PreparedOutcome::Ready(actual) = reuse_next_frame(&mut target, next, &files, viewport,
            (1, 1), &AtomicBool::new(false), Some(&mut profile), None)?
            else { anyhow::bail!("A valid changed output identity must render"); };
        assert_eq!(actual.page.pixels.as_bytes(), expected.page.pixels.as_bytes());
        assert_eq!((actual.page.width, actual.page.height), (expected.page.width, expected.page.height));
        assert_eq!(profile.unchanged_sources, 0);
        assert_eq!(profile.rendered_frames, 1);
        assert_palette(actual.page.pixels.as_bytes());
    }
    Ok(())
}

#[tokio::test]
async fn fast_remembers_cpu_choices_without_gray_settlement_and_keeps_manual_full() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut fixture = fixture(directory.path(), tokio::runtime::Handle::current(), true)?;
    let anchor = fixture.app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0;
    let mut initial = fixture.app.reading.as_ref().unwrap().core.as_ref().unwrap().clone();
    let PreparedOutcome::Ready(initial) = prepare_reader_frame(&mut initial, &fixture.files,
        fixture.viewport, (1, 1), &AtomicBool::new(false))?
        else { anyhow::bail!("The fast fixture must render"); };
    assert_binary(initial.page.pixels.as_bytes());
    fixture.app.platform.set_reader_refresh_scene(ReaderRefreshScene::Normal);
    for _ in 0..5 { fixture.app.platform.plan_refresh(RefreshReason::ReaderTurn); }
    fixture.app.platform.set_reader_refresh_scene(ReaderRefreshScene::FastContent);
    for command in ["dither-method:floyd_steinberg", "dither-method:off", "dither:on", "dither-method:bayer8"] {
        fixture.app.change_reader_setting(command)?;
        let pixels = import_target(&mut fixture)?;
        assert_binary(&pixels);
        assert_eq!(pixels.as_slice(), initial.page.pixels.as_bytes());
        let state = fixture.app.reading.as_ref().unwrap();
        assert!(state.presented_core.as_ref().unwrap().settings().fast_mode);
        assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
        assert_eq!((state.saved.every.as_str(), fixture.app.preferences.every.as_str()), ("6", "6"));
        assert_eq!(fixture.app.platform.reader_refresh_scene(), ReaderRefreshScene::FastContent);
        assert!(fixture.refreshes.take().iter().all(|plan| is_du(plan.kind)));
        assert_no_y4(&fixture);
    }
    fixture.app.platform.set_reader_refresh_scene(ReaderRefreshScene::Normal);
    assert_eq!(fixture.app.platform.plan_refresh(RefreshReason::ReaderTurn).unwrap().kind, RefreshKind::Full,
        "Saving a normal CPU choice during Fast must not reset its clearing budget");
    fixture.app.platform.set_reader_refresh_scene(ReaderRefreshScene::FastContent);
    fixture.app.change_reader_setting("dither-method:floyd_steinberg")?;
    import_target(&mut fixture)?;
    fixture.refreshes.take();
    let page = fixture.app.reading.as_ref().unwrap().page.clone();
    fixture.app.reader_action("refresh", "")?;
    let plans = fixture.refreshes.take();
    assert_eq!(plans.iter().filter(|plan| plan.kind == RefreshKind::Full).count(), 1);
    assert_eq!(fixture.app.reading.as_ref().unwrap().page, page);
    assert!(fixture.app.reading.as_ref().unwrap().presented_core.as_ref().unwrap().settings().fast_mode);
    fixture.app.reader_turn_refresh(0.9, false, false);
    assert!(fixture.refreshes.take().iter().all(|plan| is_du(plan.kind)));
    fixture.app.change_reader_setting("refresh-mode:normal")?;
    let pixels = import_target(&mut fixture)?;
    assert_palette(&pixels);
    assert!(pixels.chunks_exact(4).any(|pixel| !matches!(pixel[0], 0 | 255)));
    let state = fixture.app.reading.as_ref().unwrap();
    assert_eq!(state.saved.dither_method, "floyd_steinberg");
    assert_product_method(state.presented_core.as_ref().unwrap().settings(), SpatialDither::FloydSteinberg);
    assert_eq!((state.saved.every.as_str(), fixture.app.preferences.every.as_str()), ("6", "6"));
    assert_no_y4(&fixture);
    Ok(())
}

//! Named-fit no-ops and frame-bound reader geometry refreshes.

use super::*;
use crate::{platform::{DeviceProfile, PlatformError, RefreshSink}, source::BiliClient};
use std::{path::Path, sync::Mutex};

const COMIC: &str = "discrete-fit-comic";
const EPISODE: &str = "discrete-fit-episode";
const VIEWPORT: Viewport = Viewport { width: 16, height: 24, scale_factor: 1.0 };

#[derive(Default)]
struct RecordingRefresh(Mutex<Vec<RefreshPlan>>);

impl RecordingRefresh {
    fn plans(&self) -> Vec<RefreshPlan> { self.0.lock().unwrap().clone() }
}

impl RefreshSink for RecordingRefresh {
    fn submit(&self, plan: RefreshPlan) -> std::result::Result<(), PlatformError> {
        self.0.lock().unwrap().push(plan);
        Ok(())
    }
}

fn fixture_index() -> ImageIndex {
    ImageIndex { episode_id: EPISODE.into(), revision: "discrete-fit-revision".into(),
        images: (0..3).map(|position| PageDescriptor { id: format!("discrete-fit-page-{position}"),
            index: position, width: 32, height: 16 }).collect(), ..Default::default() }
}

fn fixture_files(directory: &Path) -> Result<ReaderFiles> {
    let paths = (0..3).map(|position| {
        let path = directory.join(format!("discrete-fit-{position}.png"));
        image::GrayImage::from_fn(32, 16, |x, y| {
            image::Luma([((x * 11 + y * 7 + position * 59) % 256) as u8])
        }).save(&path)?;
        Ok(path)
    }).collect::<Result<Vec<_>>>()?;
    Ok(ReaderFiles::Demo { paths, strip: None, strip_active: false })
}

fn prepare_in_viewport(mut core: ReaderCore, files: &ReaderFiles, viewport: Viewport) -> Result<PreparedReader> {
    let outcome = prepare_reader_frame(&mut core, files, viewport, (8, 12), &AtomicBool::new(false))?;
    anyhow::ensure!(matches!(&outcome, PreparedOutcome::Ready(_)), "The fixture must prepare actual pixels");
    Ok(PreparedReader { core, outcome })
}

fn prepare(core: ReaderCore, files: &ReaderFiles) -> Result<PreparedReader> {
    prepare_in_viewport(core, files, VIEWPORT)
}

fn fixture_app(directory: &Path) -> Result<(App, ReaderFiles, Arc<RecordingRefresh>)> {
    let mut app = super::super::test_support::app(directory, DeviceProfile::Paperwhite6,
        BiliClient::offline(directory)?, tokio::runtime::Handle::current())?;
    app.route = "reader".into();
    app.options.offline = true;
    app.preferences.preload = 0;
    app.preferences.every = "0".into();
    app.preferences.dither = false;
    app.preferences.landscape = "single".into();
    let files = fixture_files(directory)?;
    let settings = ReaderSettings { rotation: 180, dither: false, landscape_spread: false,
        ..Default::default() };
    let mut core = ReaderCore::new(fixture_index(), settings)?;
    core.set_page(1)?;
    let prepared = prepare(core, &files)?;
    let PreparedOutcome::Ready(frame) = prepared.outcome else { unreachable!() };
    let mut state = ReadingState::pending(COMIC.into(), EPISODE.into(), 41,
        SavedReaderPreferences::from(&app.preferences), false, false);
    state.original_index = Some(fixture_index());
    state.core = Some(prepared.core.clone());
    state.presented_core = Some(prepared.core);
    state.page = raster_image(frame.page);
    state.metadata = frame.metadata;
    state.loading = false;
    // Exclude the initial, smart-darkness, and periodic-full policies from these tests.
    state.last_dark_fraction = Some(0.0);
    app.reading = Some(state);
    let sink = Arc::new(RecordingRefresh::default());
    app.platform.set_refresh_sink(sink.clone())?;
    app.platform.set_reader_refresh_interval(0)?;
    Ok((app, files, sink))
}

fn displayed_anchor(app: &App) -> ReadingAnchor {
    app.reading.as_ref().unwrap().presented_core.as_ref().unwrap().anchor(0)
}

fn prepare_target(app: &App, files: &ReaderFiles) -> Result<PreparedReader> {
    prepare(app.reading.as_ref().unwrap().core.clone().unwrap(), files)
}

fn arm_import(app: &mut App) -> (u64, u64) {
    let state = app.reading.as_mut().unwrap();
    state.rendering = true;
    state.rendering_version = Some(state.version);
    state.loading = true;
    (state.generation, state.version)
}

fn import_target(app: &mut App, files: &ReaderFiles) -> Result<()> {
    let prepared = prepare_target(app, files)?;
    let (generation, version) = arm_import(app);
    app.apply_prepared_reader(generation, version, None, Ok(prepared));
    Ok(())
}

#[tokio::test]
async fn repeating_a_named_fit_preserves_its_matching_in_flight_render() -> Result<()> {
    for fit in ["original", "page", "height", "width"] {
        let directory = tempfile::tempdir()?;
        let (mut app, _files, sink) = fixture_app(directory.path())?;
        app.preferences.fit = fit.into();
        let state = app.reading.as_mut().unwrap();
        state.saved.fit = fit.into();
        let core = state.core.as_mut().unwrap();
        let mut settings = core.settings().clone();
        settings.fit = FitMode::parse(fit).unwrap();
        settings.zoom_ratio = 1.0;
        core.update_settings(settings)?;
        state.version = 9;
        state.rendering = true;
        state.rendering_version = Some(9);
        state.loading = true;
        state.render_cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = state.render_cancelled.clone();
        let old_page = state.page.clone();
        let before = displayed_anchor(&app);
        let pending = app.pending.clone();
        let sequence = app.sequence;

        app.change_reader_setting(&format!("fit:{fit}"))?;
        app.change_reader_setting(&format!("fit:{fit}"))?;

        let state = app.reading.as_ref().unwrap();
        assert_eq!(state.version, 9);
        assert_eq!(state.rendering_version, Some(9));
        assert!(state.rendering && state.loading && !state.render_again);
        assert!(Arc::ptr_eq(&state.render_cancelled, &cancellation));
        assert!(!cancellation.load(Ordering::Acquire));
        assert_eq!(state.page, old_page);
        assert_eq!(displayed_anchor(&app), before);
        assert_eq!(app.pending, pending);
        assert_eq!(app.sequence, sequence);
        assert!(app.jobs.take().is_none());
        assert!(sink.plans().is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn reselecting_page_fit_does_not_cancel_prepared_next_page_work() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, _files, sink) = fixture_app(directory.path())?;
    app.preferences.preload = 2;
    let state = app.reading.as_mut().unwrap();
    let origin = state.presented_core.as_ref().unwrap();
    let mut target = origin.clone();
    assert_eq!(target.step(true), ReaderMove::Moved);
    let key = NextFrameKey { epoch: app.epoch, generation: state.generation,
        origin_version: state.version, origin: FrameInput::new(origin), target: FrameInput::new(&target),
        viewport: reader_viewport(app.platform.logical_viewport_size(), false, origin.settings(), app.platform.scale_factor()),
        landscape: false };
    let before_key = key.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    state.next_frame = NextFrameSlot::Preparing { key, cancelled: cancelled.clone() };
    let version = state.version;
    let before = displayed_anchor(&app);

    app.change_reader_setting("fit:page")?;

    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.version, version);
    assert_eq!(state.next_frame.label(), "preparing");
    match &state.next_frame {
        NextFrameSlot::Preparing { key, cancelled: current } => {
            assert!(key == &before_key);
            assert!(Arc::ptr_eq(current, &cancelled));
        },
        _ => anyhow::bail!("A repeated fit discarded prepared next-page work"),
    }
    assert!(!cancelled.load(Ordering::Acquire));
    assert_eq!(displayed_anchor(&app), before);
    assert!(app.pending.is_empty() && app.jobs.take().is_none());
    assert!(sink.plans().is_empty());
    Ok(())
}

#[tokio::test]
async fn reselecting_a_fit_with_a_custom_multiplier_still_resets_it_and_refreshes_when_ready() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, files, sink) = fixture_app(directory.path())?;
    let state = app.reading.as_mut().unwrap();
    let mut core = state.core.clone().unwrap();
    let mut settings = core.settings().clone();
    settings.zoom_ratio = 1.5;
    core.update_settings(settings)?;
    let prepared = prepare(core, &files)?;
    let PreparedOutcome::Ready(frame) = prepared.outcome else { unreachable!() };
    state.core = Some(prepared.core.clone());
    state.presented_core = Some(prepared.core);
    state.page = raster_image(frame.page);
    state.metadata = frame.metadata;
    let old_page = state.page.clone();
    let version = state.version;
    let before = displayed_anchor(&app);

    app.change_reader_setting("fit:page")?;

    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.version, version + 1);
    assert_eq!(state.core.as_ref().unwrap().settings().zoom_ratio, 1.0);
    assert_eq!(state.core.as_ref().unwrap().settings().rotation, before.rotation);
    assert_eq!(state.core.as_ref().unwrap().page_index(), before.page_index);
    assert_eq!(displayed_anchor(&app), before);
    assert!(sink.plans().is_empty(), "A setting request cannot refresh retained pixels as new geometry");
    import_target(&mut app, &files)?;
    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.presented_core.as_ref().unwrap().settings().fit, FitMode::Page);
    assert_eq!(state.presented_core.as_ref().unwrap().settings().zoom_ratio, 1.0);
    assert_eq!(state.presented_core.as_ref().unwrap().page_index(), before.page_index);
    assert_ne!(state.page, old_page);
    assert_eq!(sink.plans(), vec![RefreshPlan { kind: RefreshKind::Full, rect: None }]);
    Ok(())
}

#[tokio::test]
async fn a_pending_fit_retains_the_old_frame_until_one_full_geometry_presentation() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, files, sink) = fixture_app(directory.path())?;
    let before = displayed_anchor(&app);
    let old_page = app.reading.as_ref().unwrap().page.clone();

    app.change_reader_setting("fit:height")?;

    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.saved.fit, "height");
    assert_eq!(state.core.as_ref().unwrap().settings().fit, FitMode::Height);
    assert_eq!(state.presented_core.as_ref().unwrap().settings().fit, FitMode::Page);
    assert_eq!(state.page, old_page);
    assert_eq!(displayed_anchor(&app), before);
    assert!(sink.plans().is_empty());

    import_target(&mut app, &files)?;

    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.presented_core.as_ref().unwrap().settings().fit, FitMode::Height);
    assert_eq!(state.presented_core.as_ref().unwrap().page_index(), before.page_index);
    assert_ne!(state.page, old_page);
    assert!(!state.loading && !state.rendering);
    assert_eq!(sink.plans(), vec![RefreshPlan { kind: RefreshKind::Full, rect: None }]);

    // A duplicated completion has no matching import lease and cannot flash again.
    let prepared = prepare_target(&app, &files)?;
    let (generation, version) = {
        let state = app.reading.as_ref().unwrap();
        (state.generation, state.version)
    };
    app.apply_prepared_reader(generation, version, None, Ok(prepared));
    assert_eq!(sink.plans(), vec![RefreshPlan { kind: RefreshKind::Full, rect: None }]);
    Ok(())
}

#[tokio::test]
async fn a_superseded_fit_never_publishes_an_intermediate_frame_or_geometry_refresh() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, files, sink) = fixture_app(directory.path())?;
    let old_page = app.reading.as_ref().unwrap().page.clone();
    let before = displayed_anchor(&app);
    app.change_reader_setting("fit:height")?;
    let stale = prepare_target(&app, &files)?;
    let (generation, stale_version) = arm_import(&mut app);

    app.change_reader_setting("fit:original")?;
    let latest_version = app.reading.as_ref().unwrap().version;
    assert!(latest_version > stale_version);
    app.apply_prepared_reader(generation, stale_version, None, Ok(stale));

    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.core.as_ref().unwrap().settings().fit, FitMode::Original);
    assert_eq!(state.presented_core.as_ref().unwrap().settings().fit, FitMode::Page);
    assert_eq!(state.page, old_page);
    assert_eq!(displayed_anchor(&app), before);
    assert!(sink.plans().is_empty());

    import_target(&mut app, &files)?;

    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.version, latest_version);
    assert_eq!(state.presented_core.as_ref().unwrap().settings().fit, FitMode::Original);
    assert_eq!(state.presented_core.as_ref().unwrap().page_index(), before.page_index);
    assert_eq!(sink.plans(), vec![RefreshPlan { kind: RefreshKind::Full, rect: None }]);
    assert!(app.pending.is_empty() && app.jobs.take().is_none());
    Ok(())
}

#[tokio::test]
async fn ordinary_page_turns_keep_partial_refresh_when_geometry_and_policy_are_unchanged() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, files, sink) = fixture_app(directory.path())?;
    let before = displayed_anchor(&app);
    app.reader_action("turn", "next")?;
    assert!(sink.plans().is_empty());
    import_target(&mut app, &files)?;
    let state = app.reading.as_ref().unwrap();
    let presented = state.presented_core.as_ref().unwrap();
    assert_eq!(presented.page_index(), before.page_index + 1);
    assert_eq!(presented.settings().fit, FitMode::Page);
    assert_eq!(presented.settings().zoom_ratio, before.zoom_ratio);
    assert_eq!(state.saved.every, "0");
    assert_eq!(sink.plans(), vec![RefreshPlan { kind: RefreshKind::Partial, rect: None }]);
    Ok(())
}

#[tokio::test]
async fn returning_to_the_presented_fit_supersedes_the_middle_fit_without_a_geometry_flash() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, files, sink) = fixture_app(directory.path())?;
    let old_page = app.reading.as_ref().unwrap().page.clone();
    let before = displayed_anchor(&app);
    app.change_reader_setting("fit:height")?;
    let middle = prepare_target(&app, &files)?;
    let (generation, middle_version) = arm_import(&mut app);

    app.change_reader_setting("fit:page")?;
    let latest_version = app.reading.as_ref().unwrap().version;
    assert!(latest_version > middle_version);
    app.apply_prepared_reader(generation, middle_version, None, Ok(middle));

    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.saved.fit, "page");
    assert_eq!(state.core.as_ref().unwrap().settings().fit, FitMode::Page);
    assert_eq!(state.presented_core.as_ref().unwrap().settings().fit, FitMode::Page);
    assert_eq!(state.page, old_page);
    assert_eq!(displayed_anchor(&app), before);
    assert!(sink.plans().is_empty());

    import_target(&mut app, &files)?;

    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.version, latest_version);
    assert_eq!(state.presented_core.as_ref().unwrap().settings().fit, FitMode::Page);
    assert_eq!(state.presented_core.as_ref().unwrap().page_index(), before.page_index);
    assert_eq!(sink.plans(), vec![RefreshPlan { kind: RefreshKind::Partial, rect: None }]);
    Ok(())
}

#[tokio::test]
async fn sleeping_with_a_pending_fit_refreshes_geometry_only_after_resumed_pixels_are_ready() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (mut app, files, sink) = fixture_app(directory.path())?;
    let old_page = app.reading.as_ref().unwrap().page.clone();
    let before = displayed_anchor(&app);
    app.change_reader_setting("fit:height")?;
    let stale = prepare_target(&app, &files)?;
    let (generation, old_version) = arm_import(&mut app);
    app.retain_reader_frame_for_sleep();
    assert!(app.reading.as_ref().unwrap().render_after_sleep);
    assert_eq!(app.reading.as_ref().unwrap().page, old_page);
    assert_eq!(displayed_anchor(&app), before);
    assert!(sink.plans().is_empty());

    app.apply_prepared_reader(generation, old_version, None, Ok(stale));
    assert_eq!(app.reading.as_ref().unwrap().page, old_page);
    assert!(sink.plans().is_empty());
    app.resume_reader_from_memory()?;
    assert_eq!(app.reading.as_ref().unwrap().page, old_page);
    assert_eq!(displayed_anchor(&app), before);
    assert!(sink.plans().is_empty());

    import_target(&mut app, &files)?;

    let state = app.reading.as_ref().unwrap();
    let presented = state.presented_core.as_ref().unwrap();
    assert_eq!(presented.settings().fit, FitMode::Height);
    assert_eq!(presented.page_index(), before.page_index);
    assert_eq!(presented.settings().rotation, before.rotation);
    assert!(!state.render_after_sleep && !state.loading && !state.rendering);
    assert_eq!(sink.plans(), vec![RefreshPlan { kind: RefreshKind::Full, rect: None }]);
    Ok(())
}

fn pixel_fixture(directory: &Path, source: &image::GrayImage, viewport: Viewport, rotation: u16)
    -> Result<(App, ReaderFiles, Arc<RecordingRefresh>)>
{
    let mut app = super::super::test_support::app(directory, DeviceProfile::Paperwhite6,
        BiliClient::offline(directory)?, tokio::runtime::Handle::current())?;
    app.route = "reader".into();
    app.options.offline = true;
    app.preferences.preload = 0;
    app.preferences.every = "0".into();
    app.preferences.dither = false;
    app.preferences.landscape = "single".into();
    let index = ImageIndex { episode_id: EPISODE.into(),
        revision: format!("app-fit-pixels-{}x{}", source.width(), source.height()),
        images: (0..3).map(|position| PageDescriptor { id: format!("app-fit-pixels-{position}"),
            index: position, width: source.width(), height: source.height() }).collect(), ..Default::default() };
    let paths = (0..3).map(|position| {
        let path = directory.join(format!("app-fit-pixels-{position}.png"));
        source.save(&path)?;
        Ok(path)
    }).collect::<Result<Vec<_>>>()?;
    let files = ReaderFiles::Demo { paths, strip: None, strip_active: false };
    let saved = SavedReaderPreferences::from(&app.preferences);
    app.store.set_preference(&app.account, &format!("reader_preferences_{COMIC}"), &saved)?;
    app.store.set_preference(&app.account, "preferences", &app.preferences)?;
    let mut settings = reader_settings(&saved, false, app.platform.logical_viewport_size());
    settings.rotation = rotation;
    settings.landscape_spread = false;
    let mut core = ReaderCore::new(index.clone(), settings)?;
    core.set_page(1)?;
    let prepared = prepare_in_viewport(core, &files, viewport)?;
    let PreparedOutcome::Ready(frame) = prepared.outcome else { unreachable!() };
    let mut state = ReadingState::pending(COMIC.into(), EPISODE.into(), 41, saved, false, false);
    state.original_index = Some(index);
    state.core = Some(prepared.core.clone());
    state.presented_core = Some(prepared.core);
    state.page = raster_image(frame.page);
    state.metadata = frame.metadata;
    state.loading = false;
    state.last_dark_fraction = Some(0.0);
    app.reading = Some(state);
    let sink = Arc::new(RecordingRefresh::default());
    app.platform.set_refresh_sink(sink.clone())?;
    app.platform.set_reader_refresh_interval(0)?;
    Ok((app, files, sink))
}

fn ink_bounds(raster: &Raster) -> Option<(u32, u32, u32, u32)> {
    let (mut left, mut top, mut right, mut bottom) = (raster.width, raster.height, 0, 0);
    for (position, pixel) in raster.rgba.chunks_exact(4).enumerate() {
        // The threshold isolates the black marker from interpolation fringes.
        if pixel[0] >= 128 { continue; }
        let x = position as u32 % raster.width;
        let y = position as u32 / raster.width;
        left = left.min(x); top = top.min(y);
        right = right.max(x + 1); bottom = bottom.max(y + 1);
    }
    (right > left && bottom > top).then(|| (left, top, right - left, bottom - top))
}

fn assert_stored_fit_pixels(source: &image::GrayImage, viewport: Viewport, fit: &str,
    expected_ink: (u32, u32, u32, u32), expected_focus: (f32, f32, f32, f32), rotation: u16) -> Result<()>
{
    let directory = tempfile::tempdir()?;
    let (mut app, files, sink) = pixel_fixture(directory.path(), source, viewport, rotation)?;
    let before = displayed_anchor(&app);
    app.reader_action("setting", &format!("fit:{fit}"))?;
    let stored: SavedReaderPreferences = app.store.preference(&app.account,
        &format!("reader_preferences_{COMIC}"))?.context("Read the persisted fit before preparation")?;
    assert_eq!(stored.fit, fit);
    let target = app.reading.as_ref().unwrap().core.clone().unwrap();
    assert_eq!(reader_settings(&stored, false, app.platform.logical_viewport_size()).fit, target.settings().fit);
    let prepared = prepare_in_viewport(target, &files, viewport)?;
    let PreparedOutcome::Ready(frame) = &prepared.outcome else { unreachable!() };
    assert_eq!((frame.page.width, frame.page.height), (viewport.width, viewport.height));
    assert_eq!(ink_bounds(&frame.page), Some(expected_ink), "Unexpected imported pixels for {fit}");
    for (actual, expected) in [(frame.metadata.focus_x, expected_focus.0),
        (frame.metadata.focus_y, expected_focus.1), (frame.metadata.focus_width, expected_focus.2),
        (frame.metadata.focus_height, expected_focus.3)] {
        assert!((actual - expected).abs() < 0.000_001, "The visible source window is not centered for {fit}: {actual} != {expected}");
    }
    assert_eq!(prepared.core.crop_count(), 1, "A discrete fit must not restore overflow-region navigation");
    let expected_pixels = frame.page.rgba.clone();
    let expected_metadata = frame.metadata;
    assert_eq!(displayed_anchor(&app), before, "Pending pixels cannot replace the current source anchor");
    assert!(sink.plans().is_empty());
    let (generation, version) = arm_import(&mut app);
    app.apply_prepared_reader(generation, version, None, Ok(prepared));
    let state = app.reading.as_ref().unwrap();
    let presented = state.presented_core.as_ref().unwrap();
    let imported = state.page.to_rgba8().context("Read back the pixels actually imported by the application")?;
    assert_eq!((imported.width(), imported.height()), (viewport.width, viewport.height));
    assert_eq!(imported.as_bytes(), expected_pixels.as_slice());
    assert_eq!(state.metadata, expected_metadata);
    assert_eq!(presented.settings().fit.as_str(), fit);
    assert_eq!(presented.page_index(), before.page_index);
    assert_eq!(presented.settings().rotation, rotation);
    assert_eq!(presented.anchor(0).page_id, before.page_id);
    assert!(!state.loading && !state.rendering);
    let kind = if fit == "page" { RefreshKind::Partial } else { RefreshKind::Full };
    assert_eq!(sink.plans(), vec![RefreshPlan { kind, rect: None }]);
    let mut next = presented.clone();
    assert_eq!(next.step(true), ReaderMove::Moved);
    assert_eq!(next.page_index(), before.page_index + 1, "The next action must advance the source page, not an old fit tile");
    Ok(())
}

#[tokio::test]
async fn stored_fits_import_small_wide_pixels_with_native_original_and_stretched_page_geometry() -> Result<()> {
    // White side columns make centered Height cropping observably different from left cropping.
    let source = image::GrayImage::from_fn(4, 2, |x, _|
        image::Luma([if x == 0 || x == 3 { 255 } else { 0 }]));
    let viewport = Viewport::new(12, 12);
    for rotation in [0, 180] {
        for (fit, ink, focus) in [
            ("original", (5, 5, 2, 2), (0.0, 0.0, 1.0, 1.0)),
            ("page", (3, 0, 6, 12), (0.0, 0.0, 1.0, 1.0)),
            ("height", (0, 0, 12, 12), (0.25, 0.0, 0.5, 1.0)),
            ("width", (3, 3, 6, 6), (0.0, 0.0, 1.0, 1.0)),
        ] {
            assert_stored_fit_pixels(&source, viewport, fit, ink, focus, rotation)?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn stored_fits_import_large_tall_pixels_without_original_overflow_and_with_centered_width_crop() -> Result<()> {
    // The complete black marker is square in the source; Page must stretch it to 16 by 8.
    // White end bands also expose an incorrect top-aligned Width crop.
    let source = image::GrayImage::from_fn(32, 64, |_, y|
        image::Luma([if y < 16 || y >= 48 { 255 } else { 0 }]));
    let viewport = Viewport::new(16, 16);
    for rotation in [0, 180] {
        for (fit, ink, focus) in [
            ("original", (4, 4, 8, 8), (0.0, 0.0, 1.0, 1.0)),
            ("page", (0, 4, 16, 8), (0.0, 0.0, 1.0, 1.0)),
            ("height", (4, 4, 8, 8), (0.0, 0.0, 1.0, 1.0)),
            ("width", (0, 0, 16, 16), (0.0, 0.25, 1.0, 0.5)),
        ] {
            assert_stored_fit_pixels(&source, viewport, fit, ink, focus, rotation)?;
        }
    }
    Ok(())
}

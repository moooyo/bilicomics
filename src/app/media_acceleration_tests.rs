//! Preference gates and complete-frame ownership without native device access.

use super::*;
use crate::{native_media::Acceleration, platform::DeviceProfile, source::BiliClient};

fn index() -> ImageIndex {
    ImageIndex { episode_id: "media-settings-episode".into(), revision: "media-settings-revision".into(),
        images: (0..3).map(|position| PageDescriptor { id: format!("media-settings-{position}"),
            index: position, width: 16, height: 24 }).collect(), ..Default::default() }
}

fn fixture(directory: &std::path::Path, handle: tokio::runtime::Handle) -> Result<App> {
    let mut app = super::super::test_support::app(directory, DeviceProfile::Paperwhite5,
        BiliClient::offline(directory)?, handle)?;
    app.route = "reader".into();
    app.options.offline = true;
    app.preferences.preload = 0;
    let settings = ReaderSettings { rotation: 180, zoom_ratio: 1.5,
        media_acceleration: Acceleration::DecodeAndResize, landscape_spread: false, ..Default::default() };
    let mut core = ReaderCore::new(index(), settings)?;
    core.set_page(1)?;
    let source = image::GrayImage::from_fn(16, 24, |x, y| image::Luma([((x * 11 + y * 7) % 256) as u8]));
    let rendered = core.render_source(&source, Viewport::new(16, 24))?;
    let mut state = ReadingState::pending("media-settings-comic".into(), index().episode_id,
        41, SavedReaderPreferences { jpeg_acceleration: true, ..Default::default() }, false, false);
    state.original_index = Some(index());
    state.core = Some(core.clone());
    state.presented_core = Some(core);
    state.page = raster_image(rendered_raster(rendered));
    state.loading = false;
    app.reading = Some(state);
    Ok(app)
}

fn key(app: &App) -> NextFrameKey {
    let state = app.reading.as_ref().unwrap();
    let origin = state.core.as_ref().unwrap();
    let mut target = origin.clone();
    assert_eq!(target.step(true), ReaderMove::Moved);
    NextFrameKey { epoch: app.epoch, generation: state.generation, origin_version: state.version,
        origin: FrameInput::new(origin), target: FrameInput::new(&target),
        viewport: Viewport::new(16, 24), landscape: false }
}

#[test]
fn jpeg_preferences_default_off_round_trip_and_gate_persisted_requests() -> Result<()> {
    let legacy: Preferences = serde_json::from_value(serde_json::json!({"direction": "rtl"}))?;
    let legacy_saved: SavedReaderPreferences = serde_json::from_value(serde_json::json!({"fit": "height"}))?;
    assert!(!legacy.jpeg_acceleration && !legacy_saved.jpeg_acceleration);
    let requested: Preferences = serde_json::from_value(serde_json::json!({"jpeg_acceleration": true}))?;
    let saved = SavedReaderPreferences::from(&requested);
    let restored: SavedReaderPreferences = serde_json::from_value(serde_json::to_value(&saved)?)?;
    assert!(restored.jpeg_acceleration);
    assert_eq!(reader_media_acceleration(&restored, false), Acceleration::Off);
    assert_eq!(reader_media_acceleration(&restored, true), Acceleration::DecodeAndResize);
    let directory = tempfile::tempdir()?;
    let store = crate::store::Store::open(directory.path().join("preferences.sqlite3"))?;
    store.set_preference("fixture", "preferences", &requested)?;
    let restored: Preferences = store.preference("fixture", "preferences")?.context("Persisted media preference")?;
    assert!(restored.jpeg_acceleration);
    let settings = reader_settings_for_display(&saved, false, (930, 1240), false);
    assert!(!crate::native_media::availability().available());
    assert_eq!(settings.media_acceleration, Acceleration::Off);
    Ok(())
}

#[tokio::test]
async fn unavailable_on_is_rejected_at_the_command_boundary() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = fixture(directory.path(), tokio::runtime::Handle::current())?;
    app.reading.as_mut().unwrap().saved.jpeg_acceleration = false;
    let version = app.reading.as_ref().unwrap().version;
    let anchor = app.reading.as_ref().unwrap().displayed_anchor(0).unwrap().0;
    let before = crate::native_media::statistics().attempts;
    app.change_reader_setting("jpeg-acceleration:on")?;
    let state = app.reading.as_ref().unwrap();
    assert!(!state.saved.jpeg_acceleration && !app.preferences.jpeg_acceleration);
    assert_eq!(state.version, version);
    assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
    assert_eq!(crate::native_media::statistics().attempts, before);
    Ok(())
}

#[tokio::test]
async fn policy_changes_cancel_preparation_and_preserve_the_presented_frame_and_anchor() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = fixture(directory.path(), tokio::runtime::Handle::current())?;
    let before_key = key(&app);
    let before = app.reading.as_ref().unwrap();
    let version = before.version;
    let page = before.page.clone();
    let anchor = before.displayed_anchor(0).unwrap().0;
    let preparation = before.core.as_ref().unwrap().source_preparation(before_key.viewport);
    let cancelled = Arc::new(AtomicBool::new(false));
    app.reading.as_mut().unwrap().next_frame = NextFrameSlot::Preparing {
        key: before_key.clone(), cancelled: cancelled.clone() };
    app.change_reader_setting("jpeg-acceleration:off")?;
    let state = app.reading.as_ref().unwrap();
    let core = state.core.as_ref().unwrap();
    assert!(cancelled.load(Ordering::Acquire));
    assert_eq!(state.version, version.wrapping_add(1));
    assert_eq!(core.settings().media_acceleration, Acceleration::Off);
    assert_eq!((core.settings().rotation, core.settings().zoom_ratio), (180, 1.5));
    assert_eq!(state.page, page);
    assert_eq!(state.displayed_anchor(0).unwrap().0, anchor);
    assert_eq!(state.presented_core.as_ref().unwrap().settings().media_acceleration, Acceleration::DecodeAndResize);
    assert_ne!(preparation, core.source_preparation(before_key.viewport));
    assert!(!before_key.origin.matches(core));
    assert!(!state.saved.jpeg_acceleration && !app.preferences.jpeg_acceleration);
    let saved: SavedReaderPreferences = app.store.preference(&app.account, "reader_preferences_media-settings-comic")?
        .context("Saved per-comic acceleration policy")?;
    assert!(!saved.jpeg_acceleration);
    Ok(())
}

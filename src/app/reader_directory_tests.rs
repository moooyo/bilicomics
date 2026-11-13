//! Directory navigation and named-fit regressions at the application boundary.

use super::*;
use crate::{domain::Comic, platform::DeviceProfile, source::BiliClient};

const COMIC: &str = "directory-fixture-comic";
const CURRENT: &str = "directory-fixture-episode-12";

fn episode_id(order: usize) -> String { format!("directory-fixture-episode-{order}") }

fn fixture_index() -> ImageIndex {
    ImageIndex { episode_id: CURRENT.into(), revision: "directory-fixture-revision".into(),
        images: (0..45).map(|index| PageDescriptor { id: format!("directory-fixture-page-{index}"),
            index, width: 16, height: 24 }).collect(), ..Default::default() }
}

fn fixture_state() -> Result<ReadingState> {
    let index = fixture_index();
    let mut core = ReaderCore::new(index.clone(), ReaderSettings {
        rotation: 180, zoom_ratio: 1.5, dither: false, landscape_spread: false,
        ..Default::default()
    })?;
    core.set_page(17)?;
    let source = image::GrayImage::from_pixel(16, 24, image::Luma([68]));
    let frame = core.render_source(&source, Viewport::new(16, 24))?;
    let mut state = ReadingState::pending(COMIC.into(), CURRENT.into(), 41,
        SavedReaderPreferences { dither: false, landscape: "single".into(), ..Default::default() },
        false, false);
    let timestamp = now();
    let mut episodes = (1..=32).map(|order| {
        let access = match order {
            13 => EpisodeAccess::Locked, 14 | 18 => EpisodeAccess::Temporary,
            15 => EpisodeAccess::Unavailable, 16 => EpisodeAccess::Unknown,
            17 => EpisodeAccess::Owned, _ => EpisodeAccess::Free,
        };
        Episode { id: episode_id(order), comic_id: COMIC.into(), title: format!("Chapter {order}"),
            order: order as f64, access, read: order <= 11,
            expires_at: match order { 14 => Some(timestamp + 86_400), 18 => Some(timestamp - 60), _ => None },
            pay_gold: (order == 13).then_some(32.0),
            // Unproven source flags must not become local download evidence.
            extra: serde_json::json!({"downloaded": true}), ..Default::default() }
    }).collect::<Vec<_>>();
    episodes.rotate_left(17);
    state.detail = Some(ComicDetail { comic: Comic { id: COMIC.into(), title: "Directory fixture".into(),
        chapter_count: 32, ..Default::default() }, episodes, ..Default::default() });
    state.original_index = Some(index);
    state.page = raster_image(rendered_raster(frame));
    state.core = Some(core.clone());
    state.presented_core = Some(core);
    state.loading = false;
    Ok(state)
}

fn fixture_app(path: &std::path::Path, profile: DeviceProfile) -> Result<App> {
    let mut app = super::super::test_support::app(path, profile, BiliClient::offline(path)?,
        tokio::runtime::Handle::current())?;
    app.options.offline = true;
    app.route = "reader".into();
    app.reading = Some(fixture_state()?);
    Ok(app)
}

fn anchor(app: &App) -> ReadingAnchor {
    app.reading.as_ref().unwrap().presented_core.as_ref().unwrap().anchor(0)
}

#[test]
fn directory_capacity_matches_both_portraits_and_stays_bounded_after_resize() {
    assert_eq!(chapter_capacity((930, 1240), true), 14);
    assert_eq!(chapter_capacity((632, 840), false), 10);
    assert_eq!(chapter_capacity((1240, 930), true), 9);
    assert_eq!(chapter_capacity((840, 632), false), 6);
    assert_eq!(chapter_capacity((320, 200), false), 1);
    assert_eq!(chapter_capacity((930, u32::MAX), true), 64);
}

#[tokio::test]
async fn opening_the_complete_directory_locates_the_current_chapter_on_each_profile() -> Result<()> {
    for (profile, capacity, page, pages) in [(DeviceProfile::Scribe, 14, 1, 3),
        (DeviceProfile::Paperwhite6, 10, 2, 4)] {
        let directory = tempfile::tempdir()?;
        let mut app = fixture_app(directory.path(), profile)?;
        let before = anchor(&app);
        app.reader_action("sheet", "toc")?;
        let state = app.reading.as_ref().unwrap();
        assert_eq!(state.sheet, "toc");
        assert!(state.menu);
        let result = app.reader_directory_page(state, capacity)?;
        assert_eq!((result.capacity, result.page, result.pages), (capacity, page, pages));
        assert_eq!(result.rows.len(), capacity);
        let current = result.rows.iter().find(|row| row.current).context("Show the current chapter")?;
        assert_eq!(current.id.as_str(), CURRENT);
        assert!(current.progress.as_str().contains("18") && current.progress.as_str().contains("45"));
        assert_eq!(anchor(&app), before);
        assert!(app.pending.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn complete_directory_pagination_reaches_every_chapter_once_and_clamps_at_its_edges() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = fixture_app(directory.path(), DeviceProfile::Paperwhite6)?;
    app.reader_action("sheet", "toc")?;
    app.reader_action("toc", "previous")?;
    app.reader_action("toc", "previous")?;
    let mut ids = Vec::new();
    for page in 1..=4 {
        let state = app.reading.as_ref().unwrap();
        let result = app.reader_directory_page(state, 10)?;
        assert_eq!(result.page, page);
        ids.extend(result.rows.iter().map(|row| row.id.to_string()));
        app.reader_action("toc", "next")?;
    }
    assert_eq!(ids, (1..=32).map(episode_id).collect::<Vec<_>>());
    assert_eq!(app.reading.as_ref().unwrap().directory.page, 4);
    assert_eq!(anchor(&app).page_index, 17);
    assert!(app.pending.is_empty());
    Ok(())
}

#[tokio::test]
async fn directory_filters_sorting_and_locate_keep_access_and_local_evidence_distinct() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = fixture_app(directory.path(), DeviceProfile::Paperwhite6)?;
    app.reader_action("sheet", "toc")?;
    app.reader_action("toc", "filter:unread")?;
    let selection = app.reader_directory_selection(app.reading.as_ref().unwrap(), 10);
    assert_eq!(selection.episodes.len(), 20);
    assert!(selection.episodes.iter().all(|episode| !episode.read && episode.id != CURRENT));

    app.reader_action("toc", "filter:readable")?;
    let selection = app.reader_directory_selection(app.reading.as_ref().unwrap(), 10);
    assert_eq!(selection.episodes.len(), 28);
    assert!(selection.episodes.iter().all(|episode| episode.readable_at(now())));
    assert!(selection.episodes.iter().any(|episode| episode.id == episode_id(14)));
    for blocked in [13, 15, 16, 18] {
        assert!(!selection.episodes.iter().any(|episode| episode.id == episode_id(blocked)));
    }

    app.reader_action("toc", "filter:downloaded")?;
    let result = app.reader_directory_page(app.reading.as_ref().unwrap(), 10)?;
    assert!(result.rows.is_empty(), "Source flags do not prove a verified local chapter");
    assert_eq!((result.page, result.pages), (1, 1));
    app.reader_action("toc", "filter:all")?;
    app.reader_action("toc", "sort:desc")?;
    let result = app.reader_directory_page(app.reading.as_ref().unwrap(), 10)?;
    assert_eq!(result.rows.first().unwrap().id.as_str(), episode_id(32));
    app.reader_action("toc", "locate")?;
    let state = app.reading.as_ref().unwrap();
    assert_eq!(state.directory.filter, "all");
    assert!(!state.directory.descending);
    assert_eq!(state.directory.page, 2);
    assert!(app.reader_directory_page(state, 10)?.rows.iter().any(|row| row.current));
    Ok(())
}

#[tokio::test]
async fn choosing_the_current_chapter_only_closes_the_directory_and_keeps_the_live_frame() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = fixture_app(directory.path(), DeviceProfile::Paperwhite6)?;
    let before = anchor(&app);
    let before_core = app.reading.as_ref().unwrap().core.as_ref().unwrap().anchor(0);
    let generation = app.reading.as_ref().unwrap().generation;
    let version = app.reading.as_ref().unwrap().version;
    let page = app.reading.as_ref().unwrap().page.clone();
    app.reader_action("sheet", "toc")?;
    app.reader_action("toc", "next")?;
    app.reader_action("episode", CURRENT)?;
    let state = app.reading.as_ref().unwrap();
    assert!(!state.menu && state.sheet.is_empty());
    assert_eq!(app.route, "reader");
    assert_eq!((state.generation, state.version), (generation, version));
    assert_eq!(state.core.as_ref().unwrap().anchor(0), before_core);
    assert_eq!(anchor(&app), before);
    assert_eq!(state.page, page);
    assert!(!state.loading && !state.rendering);
    assert!(app.pending.is_empty());
    Ok(())
}

#[tokio::test]
async fn retired_grid_and_unknown_sheet_intents_never_create_thumbnail_work() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = fixture_app(directory.path(), DeviceProfile::Paperwhite6)?;
    let before = anchor(&app);
    for (action, value) in [("sheet", "grid"), ("grid", "next"), ("sheet", "unknown-sheet"),
        ("sheet", "toc/grid")] {
        app.reader_action(action, value)?;
        let state = app.reading.as_ref().unwrap();
        assert!(!state.menu && state.sheet.is_empty());
        assert!(state.thumbnails.is_empty() && state.thumbnail_plan.is_none());
        assert!(!state.thumbnails_pending && !state.thumbnails_rendering);
        assert!(app.pending.is_empty());
        assert_eq!(anchor(&app), before);
    }
    app.reader_action("sheet", "toc")?;
    app.reader_action("sheet", "grid")?;
    assert_eq!(app.reading.as_ref().unwrap().sheet, "toc");
    assert!(app.pending.is_empty());
    Ok(())
}

#[tokio::test]
async fn all_named_fits_persist_and_reload_without_changing_the_source_page_or_rotation() -> Result<()> {
    for fit in ["original", "page", "height", "width"] {
        let directory = tempfile::tempdir()?;
        let mut app = fixture_app(directory.path(), DeviceProfile::Paperwhite6)?;
        let before = anchor(&app);
        app.reader_action("setting", &format!("fit:{fit}"))?;
        let state = app.reading.as_ref().unwrap();
        let target = state.core.as_ref().unwrap();
        assert_eq!(target.settings().fit.as_str(), fit);
        assert_eq!(target.page_index(), before.page_index);
        assert_eq!(target.settings().rotation, before.rotation);
        assert_eq!(target.settings().zoom_ratio, 1.0);
        assert_eq!(state.saved.fit, fit);
        assert_eq!(anchor(&app), before, "A pending fit cannot relabel the retained frame");
        let stored: SavedReaderPreferences = app.store.preference(&app.account,
            &format!("reader_preferences_{COMIC}"))?.context("Persist the comic fit")?;
        assert_eq!(stored.fit, fit);
        let defaults: Preferences = app.store.preference(&app.account, "preferences")?
            .context("Persist the default fit")?;
        assert_eq!(defaults.fit, fit);
        drop(app);

        let mut reopened = super::super::test_support::app(directory.path(), DeviceProfile::Paperwhite6,
            BiliClient::offline(directory.path())?, tokio::runtime::Handle::current())?;
        reopened.preferences = reopened.store.preference(&reopened.account, "preferences")?.unwrap();
        reopened.begin_reader(COMIC, CURRENT)?;
        assert_eq!(reopened.reading.as_ref().unwrap().saved.fit, fit);
        assert_eq!(reopened.preferences.fit, fit);
    }
    Ok(())
}

#[tokio::test]
async fn new_named_fits_survive_sleep_reconciliation_at_the_presented_source_page() -> Result<()> {
    for fit in ["original", "height"] {
        let directory = tempfile::tempdir()?;
        let mut app = fixture_app(directory.path(), DeviceProfile::Paperwhite6)?;
        let before = anchor(&app);
        app.reader_action("setting", &format!("fit:{fit}"))?;
        app.retain_reader_frame_for_sleep();
        let state = app.reading.as_ref().unwrap();
        assert_eq!(anchor(&app), before);
        assert!(state.render_after_sleep);
        assert_eq!(state.core.as_ref().unwrap().settings().fit.as_str(), fit);
        assert_eq!(state.core.as_ref().unwrap().page_index(), before.page_index);
        app.resume_reader_from_memory()?;
        let target = app.reading.as_ref().unwrap().core.as_ref().unwrap();
        assert_eq!(target.settings().fit.as_str(), fit);
        assert_eq!(target.page_index(), before.page_index);
        assert_eq!(target.settings().rotation, before.rotation);
        assert_eq!(target.settings().zoom_ratio, 1.0);
        assert_eq!(anchor(&app), before);
    }
    Ok(())
}

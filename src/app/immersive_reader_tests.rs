use super::*;
use crate::{platform::DeviceProfile, reader::{MAX_VIEWPORT_PIXELS, SourceRect}};
use slint::private_unstable_api::re_exports::{ModelChangeListener, ModelChangeListenerContainer};
use std::{cell::RefCell, pin::Pin};

fn profiles() -> [(DeviceProfile, (u32, u32)); 4] {
    [
        (DeviceProfile::Paperwhite5, (1236, 1648)),
        (DeviceProfile::Paperwhite6, (1264, 1680)),
        (DeviceProfile::Scribe, (1860, 2480)),
        (DeviceProfile::Scribe2025, (1980, 2640)),
    ]
}

fn synthetic_index(pages: usize, width: u32, height: u32) -> ImageIndex {
    ImageIndex { episode_id: "synthetic-episode".into(), revision: "synthetic-revision".into(),
        images: (0..pages).map(|index| PageDescriptor { id: format!("synthetic-page-{index}"),
            index, width, height }).collect(), ..Default::default() }
}

#[test]
fn all_profile_single_viewports_use_the_complete_window() {
    for (profile, physical) in profiles() {
        for landscape in [false, true] {
            for scale in [1.0, 2.0] {
                for (mode, fit) in [
                    (DisplayMode::Page, FitMode::Page),
                    (DisplayMode::Page, FitMode::Width),
                    (DisplayMode::Panel, FitMode::Page),
                    (DisplayMode::Strip, FitMode::Page),
                ] {
                    for footer in [false, true] {
                        let settings = ReaderSettings { mode, fit, footer,
                            landscape_spread: false, ..Default::default() };
                        let viewport = reader_viewport(profile.logical_size(), landscape, &settings, scale);
                        let divisor = if scale == 2.0 { 1 } else { 2 };
                        let expected = if landscape { (physical.1 / divisor, physical.0 / divisor) }
                            else { (physical.0 / divisor, physical.1 / divisor) };
                        assert_eq!((viewport.width, viewport.height), expected,
                            "{profile:?}, landscape={landscape}, mode={mode:?}, fit={fit:?}, footer={footer}");
                        assert_eq!(viewport.scale_factor, scale);
                        assert!(u64::from(viewport.width) * u64::from(viewport.height) <= MAX_VIEWPORT_PIXELS);
                    }
                }
            }
        }
    }
}

#[test]
fn spreads_keep_full_height_and_half_width_only_on_eligible_landscape_canvases() {
    let saved = SavedReaderPreferences::default();
    for (profile, physical) in profiles() {
        for landscape in [false, true] {
            let settings = reader_settings(&saved, landscape, profile.logical_size());
            let mut core = ReaderCore::new(synthetic_index(4, 100, 140), settings.clone()).unwrap();
            core.set_landscape(landscape);
            let expected_spread = landscape && profile.is_scribe();
            assert_eq!(core.is_spread(), expected_spread, "{profile:?}, landscape={landscape}");
            let viewport = reader_viewport(profile.logical_size(), landscape, &settings, 2.0);
            let expected = if expected_spread { (physical.1 / 2, physical.0) }
                else if landscape { (physical.1, physical.0) } else { physical };
            assert_eq!((viewport.width, viewport.height), expected);

            let mut without_footer = saved.clone();
            without_footer.footer = false;
            let settings = reader_settings(&without_footer, landscape, profile.logical_size());
            assert_eq!(reader_viewport(profile.logical_size(), landscape, &settings, 2.0), viewport);
        }
    }
}

#[test]
fn native_window_geometry_reaches_every_available_pixel() {
    let settings = ReaderSettings { landscape_spread: false, ..Default::default() };
    let portrait = reader_viewport((636, 848), false, &settings, 2.0);
    let landscape = reader_viewport((636, 848), true, &settings, 2.0);
    assert_eq!((portrait.width, portrait.height), (1272, 1696));
    assert_eq!((landscape.width, landscape.height), (1696, 1272));

    let spread = ReaderSettings { landscape_spread: true, ..settings };
    let leaf = reader_viewport((931, 1241), true, &spread, 2.0);
    assert_eq!((leaf.width, leaf.height), (1241, 1862));
}

#[test]
fn fit_page_preserves_the_whole_source_and_all_four_corners() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("synthetic-corners.png");
    let (source_width, source_height) = (161, 239);
    image::GrayImage::from_fn(source_width, source_height, |x, y| {
        image::Luma([match (x < source_width / 2, y < source_height / 2) {
            (true, true) => 17, (false, true) => 68,
            (true, false) => 136, (false, false) => 204,
        }])
    }).save(&path)?;

    for (profile, _) in profiles() {
        for landscape in [false, true] {
            let saved = SavedReaderPreferences { fit: "page".into(), landscape: "single".into(),
                dither: false, ..Default::default() };
            let settings = reader_settings(&saved, landscape, profile.logical_size());
            assert_eq!(settings.fit, FitMode::Page);
            assert!(!settings.landscape_spread);
            let viewport = reader_viewport(profile.logical_size(), landscape, &settings, 1.0);
            let mut core = ReaderCore::new(synthetic_index(1, source_width, source_height), settings)?;
            core.set_landscape(landscape);
            let rendered = core.render(&path, viewport)?;
            assert_eq!(rendered.crop, SourceRect { x: 0, y: 0, width: source_width, height: source_height });
            assert_eq!((rendered.width, rendered.height), (viewport.width, viewport.height));
            assert_eq!((rendered.crop_index, rendered.crop_count), (0, 1));
            let image = image::RgbaImage::from_raw(rendered.width, rendered.height, rendered.rgba)
                .context("Read the synthetic full-page canvas")?;
            let mut bounds = (image.width(), image.height(), 0, 0);
            for (x, y, _) in image.enumerate_pixels().filter(|(_, _, pixel)| pixel[0] != 255) {
                bounds.0 = bounds.0.min(x); bounds.1 = bounds.1.min(y);
                bounds.2 = bounds.2.max(x); bounds.3 = bounds.3.max(y);
            }
            let (left, top, right, bottom) = bounds;
            assert_eq!(image.get_pixel(left, top)[0], 17);
            assert_eq!(image.get_pixel(right, top)[0], 68);
            assert_eq!(image.get_pixel(left, bottom)[0], 136);
            assert_eq!(image.get_pixel(right, bottom)[0], 204);
            let width = right - left + 1;
            let height = bottom - top + 1;
            assert!(width == viewport.width || height == viewport.height);
            assert!(u64::from(width).saturating_mul(u64::from(source_height))
                .abs_diff(u64::from(height) * u64::from(source_width))
                <= u64::from(source_width + source_height));
            assert!(left.abs_diff(viewport.width - right - 1) <= 1);
            assert!(top.abs_diff(viewport.height - bottom - 1) <= 1);
        }
    }
    Ok(())
}

#[test]
fn panel_width_fit_keeps_the_complete_top_aligned_canvas() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("synthetic-flat-panel.png");
    image::GrayImage::from_fn(320, 80, |x, y| {
        image::Luma([match (x < 160, y < 40) {
            (true, true) => 17, (false, true) => 68,
            (true, false) => 136, (false, false) => 204,
        }])
    }).save(&path)?;
    let files = ReaderFiles::Demo { paths: vec![path], strip: None, strip_active: false };
    let settings = ReaderSettings { mode: DisplayMode::Panel, fit: FitMode::Width,
        dither: false, landscape_spread: false, ..Default::default() };
    let viewport = reader_viewport((240, 320), true, &settings, 1.0);
    let mut core = ReaderCore::new(synthetic_index(1, 320, 80), settings)?;
    core.set_landscape(true);
    core.set_panel_map("synthetic-page-0", vec![NormalizedRect {
        x: 0.0, y: 0.0, width: 1.0, height: 1.0,
    }])?;
    let PreparedOutcome::Ready(frame) = prepare_reader_frame(&mut core, &files, viewport,
        (58, 84), &AtomicBool::new(false))?
        else { anyhow::bail!("A complete synthetic panel must render"); };
    assert_eq!((frame.page.width, frame.page.height), (320, 240));
    let canvas = image::RgbaImage::from_raw(frame.page.width, frame.page.height, frame.page.rgba)
        .context("Read the complete top-aligned panel canvas")?;
    assert_eq!(canvas.get_pixel(0, 0)[0], 17);
    assert_eq!(canvas.get_pixel(319, 0)[0], 68);
    assert_eq!(canvas.get_pixel(0, 79)[0], 136);
    assert_eq!(canvas.get_pixel(319, 79)[0], 204);
    assert!(canvas.enumerate_pixels().filter(|(_, y, _)| *y >= 80)
        .all(|(_, _, pixel)| pixel.0 == [255; 4]));
    assert_eq!((frame.metadata.focus_x, frame.metadata.focus_y), (0.0, 0.0));
    assert_eq!((frame.metadata.focus_width, frame.metadata.focus_height), (1.0, 1.0));
    assert_eq!(core.crop_count(), 1);
    let anchor = core.anchor(0);
    assert_eq!((anchor.source_x, anchor.source_y), (0.0, 0.0));
    let overview = frame.overview.context("Retain the complete source overview")?;
    assert_eq!((overview.width, overview.height), (58, 84));
    assert!(overview.rgba.chunks_exact(4).any(|pixel| pixel[0] != 255));
    Ok(())
}

fn install_synthetic_panels(core: &mut ReaderCore) -> Result<()> {
    if core.settings().mode == DisplayMode::Panel {
        core.set_panel_map("synthetic-page-1", vec![
            NormalizedRect { x: 0.0, y: 0.0, width: 1.0, height: 0.25 },
            NormalizedRect { x: 0.0, y: 0.25, width: 1.0, height: 0.5 },
            NormalizedRect { x: 0.0, y: 0.75, width: 1.0, height: 0.25 },
        ])?;
    }
    Ok(())
}

#[test]
fn full_window_regions_and_anchors_survive_legacy_footer_changes() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("synthetic-tall-page.png");
    image::GrayImage::from_pixel(100, 800, image::Luma([17])).save(&path)?;
    for (mode, fit) in [(DisplayMode::Strip, FitMode::Page),
        (DisplayMode::Panel, FitMode::Page), (DisplayMode::Page, FitMode::Width)] {
        let settings = ReaderSettings { mode, fit, dither: false, landscape_spread: false, ..Default::default() };
        let index = synthetic_index(2, 100, 800);
        let viewport = reader_viewport((240, 320), false, &settings, 1.0);
        let mut core = ReaderCore::new(index.clone(), settings)?;
        install_synthetic_panels(&mut core)?;
        core.set_page(1)?;
        core.render(&path, viewport)?;
        assert_eq!(core.step(true), ReaderMove::Moved);
        let selected = core.render(&path, viewport)?;
        assert!(selected.crop_index > 0);
        let anchor = core.anchor(0);
        assert!(anchor.source_y > 0.0);

        let mut without_footer = core.settings().clone();
        without_footer.footer = false;
        core.update_settings(without_footer.clone())?;
        assert_eq!(core.anchor(0), anchor);
        assert_eq!((core.crop_index(), core.crop_count()), (selected.crop_index, selected.crop_count));
        assert_eq!(reader_viewport((240, 320), false, &without_footer, 1.0), viewport);
        let retained = core.render(&path, viewport)?;
        assert_eq!(retained.crop, selected.crop);
        assert_eq!(retained.rgba, selected.rgba);

        let mut restored = ReaderCore::new(index, without_footer)?;
        install_synthetic_panels(&mut restored)?;
        restored.restore(&anchor)?;
        let reopened = restored.render(&path, viewport)?;
        assert_eq!(reopened.crop, selected.crop);
        assert_eq!((reopened.crop_index, reopened.crop_count), (selected.crop_index, selected.crop_count));
        assert_eq!(reopened.rgba, selected.rgba);
        assert_eq!(restored.anchor(0), anchor);
    }
    Ok(())
}

#[test]
fn fullscreen_spread_slots_preserve_cover_blanks_direction_and_selected_source() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let tones = [17, 68, 136, 204];
    let mut paths = Vec::new();
    for (page, tone) in tones.into_iter().enumerate() {
        let path = directory.path().join(format!("synthetic-spread-{page}.png"));
        image::GrayImage::from_pixel(100, 140, image::Luma([tone])).save(&path)?;
        paths.push(path);
    }
    let files = ReaderFiles::Demo { paths, strip: None, strip_active: false };
    for (cover_alone, direction, page, expected) in [
        (true, PageDirection::LeftToRight, 0, [None, Some(0)]),
        (true, PageDirection::RightToLeft, 0, [Some(0), None]),
        (true, PageDirection::LeftToRight, 2, [Some(1), Some(2)]),
        (true, PageDirection::RightToLeft, 2, [Some(2), Some(1)]),
        (true, PageDirection::LeftToRight, 3, [Some(3), None]),
        (true, PageDirection::RightToLeft, 3, [None, Some(3)]),
        (false, PageDirection::LeftToRight, 2, [Some(2), Some(3)]),
        (false, PageDirection::RightToLeft, 2, [Some(3), Some(2)]),
    ] {
        let settings = ReaderSettings { cover_alone, direction, dither: false, ..Default::default() };
        let viewport = reader_viewport(DeviceProfile::Scribe.logical_size(), true, &settings, 1.0);
        assert_eq!((viewport.width, viewport.height), (620, 930));
        let mut core = ReaderCore::new(synthetic_index(4, 100, 140), settings)?;
        core.set_landscape(true);
        core.set_page(page)?;
        let PreparedOutcome::Ready(frame) = prepare_reader_frame(&mut core, &files, viewport, (58, 84), &AtomicBool::new(false))?
            else { anyhow::bail!("A complete synthetic spread must render"); };
        assert_eq!(core.page_index(), page);
        assert_eq!(core.spread_indices(), expected);
        assert!(frame.overview.is_none());
        let numbers = [frame.metadata.left_page, frame.metadata.right_page];
        let leaves = [Some(&frame.page), frame.second.as_ref()];
        for (slot, source) in expected.into_iter().enumerate() {
            if let Some(source) = source {
                assert_eq!(numbers[slot], source as i32 + 1);
                let raster = leaves[slot].context("Keep the occupied spread slot")?;
                assert_eq!((raster.width, raster.height), (viewport.width, viewport.height));
                let center = ((raster.height / 2 * raster.width + raster.width / 2) * 4) as usize;
                assert_eq!(raster.rgba[center], tones[source]);
            } else {
                assert_eq!(numbers[slot], if page == 0 { 0 } else { 5 });
                if let Some(blank) = leaves[slot] {
                    assert_eq!((blank.width, blank.height), (1, 1));
                    assert_eq!(blank.rgba, [255; 4]);
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum ReaderRowChange {
    Changed(usize),
    Added(usize, usize),
    Removed(usize, usize),
    Reset,
}

struct ReaderRowListener(Rc<RefCell<Vec<ReaderRowChange>>>);

impl ModelChangeListener for ReaderRowListener {
    fn row_changed(self: Pin<&Self>, row: usize) {
        self.0.borrow_mut().push(ReaderRowChange::Changed(row));
    }

    fn row_added(self: Pin<&Self>, index: usize, count: usize) {
        self.0.borrow_mut().push(ReaderRowChange::Added(index, count));
    }

    fn row_removed(self: Pin<&Self>, index: usize, count: usize) {
        self.0.borrow_mut().push(ReaderRowChange::Removed(index, count));
    }

    fn reset(self: Pin<&Self>) {
        self.0.borrow_mut().push(ReaderRowChange::Reset);
    }
}

#[test]
fn thumbnail_readiness_updates_keep_model_identity_and_never_reset_existing_rows() {
    let rows = (1..=3).map(|page| ReaderThumbnail { page, picture: slint::Image::default(),
        label: format!("Loading page {page}").into(), cached: false, current: page == 2 }).collect::<Vec<_>>();
    let model = Rc::new(VecModel::from(rows.clone()));
    let bound: ModelRc<ReaderThumbnail> = model.clone().into();
    let notifications = Rc::new(RefCell::new(Vec::new()));
    let listener = Box::pin(ModelChangeListenerContainer::new(ReaderRowListener(notifications.clone())));
    model.model_tracker().attach_peer(listener.as_ref().model_peer());

    for _ in 0..3 { sync_reader_rows(&model, &rows, same_reader_thumbnail); }
    assert!(notifications.borrow().is_empty());
    let refreshed: ModelRc<ReaderThumbnail> = model.clone().into();
    assert!(bound == refreshed);

    let mut ready = rows.clone();
    ready[1].picture = raster_image(Raster { rgba: vec![17, 17, 17, 255], width: 1, height: 1 });
    ready[1].label = "Current page 2".into();
    ready[1].cached = true;
    sync_reader_rows(&model, &ready, same_reader_thumbnail);
    assert_eq!(notifications.borrow().as_slice(), &[ReaderRowChange::Changed(1)]);
    assert_eq!(model.row_count(), 3);
    assert_eq!(model.row_data(1), Some(ready[1].clone()));
    notifications.borrow_mut().clear();
    sync_reader_rows(&model, &ready, same_reader_thumbnail);
    assert!(notifications.borrow().is_empty());

    ready[1].picture = raster_image(Raster { rgba: vec![68, 68, 68, 255], width: 1, height: 1 });
    sync_reader_rows(&model, &ready, same_reader_thumbnail);
    assert_eq!(notifications.borrow().as_slice(), &[ReaderRowChange::Changed(1)]);
    assert_eq!(model.row_data(1), Some(ready[1].clone()));
    notifications.borrow_mut().clear();
    for _ in 0..3 { sync_reader_rows(&model, &ready, same_reader_thumbnail); }
    assert!(notifications.borrow().is_empty());

    ready.push(ReaderThumbnail { page: 4, label: "Loading page 4".into(), ..Default::default() });
    sync_reader_rows(&model, &ready, same_reader_thumbnail);
    assert_eq!(notifications.borrow().as_slice(), &[ReaderRowChange::Added(3, 1)]);
    notifications.borrow_mut().clear();
    ready.truncate(2);
    sync_reader_rows(&model, &ready, same_reader_thumbnail);
    assert_eq!(notifications.borrow().as_slice(), &[
        ReaderRowChange::Removed(3, 1), ReaderRowChange::Removed(2, 1),
    ]);
    assert_eq!(model.row_data(1), Some(ready[1].clone()));
    let resized: ModelRc<ReaderThumbnail> = model.into();
    assert!(bound == resized);
}

#[test]
fn chapter_markers_update_existing_rows_without_replacing_the_model() {
    let mut chapters = vec![
        ReaderChapter { id: "synthetic-chapter-1".into(), current: true, tag: "Reading".into(), ..Default::default() },
        ReaderChapter { id: "synthetic-chapter-2".into(), tag: "Ready".into(), ..Default::default() },
    ];
    let model = Rc::new(VecModel::from(chapters.clone()));
    let bound: ModelRc<ReaderChapter> = model.clone().into();
    let notifications = Rc::new(RefCell::new(Vec::new()));
    let listener = Box::pin(ModelChangeListenerContainer::new(ReaderRowListener(notifications.clone())));
    model.model_tracker().attach_peer(listener.as_ref().model_peer());
    sync_reader_rows(&model, &chapters, PartialEq::eq);
    assert!(notifications.borrow().is_empty());

    chapters[0].current = false;
    chapters[0].tag = "Read".into();
    chapters[1].current = true;
    chapters[1].tag = "Reading".into();
    sync_reader_rows(&model, &chapters, PartialEq::eq);
    assert_eq!(notifications.borrow().as_slice(), &[
        ReaderRowChange::Changed(0), ReaderRowChange::Changed(1),
    ]);
    assert_eq!(model.row_count(), 2);
    let refreshed: ModelRc<ReaderChapter> = model.into();
    assert!(bound == refreshed);
}

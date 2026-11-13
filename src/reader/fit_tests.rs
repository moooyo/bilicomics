//! Named unit-scale fits are verified against geometry and identifiable pixels.

use super::*;
use crate::domain::PageDescriptor;
use image::Luma;


fn index(width: u32, height: u32) -> ImageIndex {
    ImageIndex { episode_id: "fit-episode".into(), revision: "fit-revision".into(),
        images: (0..2).map(|position| PageDescriptor { id: format!("fit-page-{position}"),
            index: position, width, height }).collect(), ..Default::default() }
}

fn settings(fit: FitMode) -> ReaderSettings {
    ReaderSettings { fit, landscape_spread: false, dither: true,
        hardware_dither: true, ..Default::default() }
}

fn reader(source: &GrayImage, fit: FitMode) -> ReaderCore {
    ReaderCore::new(index(source.width(), source.height()), settings(fit)).unwrap()
}

fn quadrants(width: u32, height: u32) -> GrayImage {
    GrayImage::from_fn(width, height, |x, y| Luma([match (x < width / 2, y < height / 2) {
        (true, true) => 17, (false, true) => 68,
        (true, false) => 136, (false, false) => 204,
    }]))
}

fn pixel(frame: &RenderedPage, x: u32, y: u32) -> u8 {
    frame.rgba[((y * frame.width + x) * 4) as usize]
}

fn ink_bounds(frame: &RenderedPage) -> SourceRect {
    let mut left = frame.width;
    let mut top = frame.height;
    let mut right = 0;
    let mut bottom = 0;
    for (position, pixel) in frame.rgba.chunks_exact(4).enumerate() {
        if pixel[0] == 255 { continue; }
        let x = position as u32 % frame.width;
        let y = position as u32 / frame.width;
        left = left.min(x);
        top = top.min(y);
        right = right.max(x + 1);
        bottom = bottom.max(y + 1);
    }
    SourceRect { x: left, y: top, width: right - left, height: bottom - top }
}

fn ideal_output(width: u32, height: u32, viewport: Viewport, fit: FitMode) -> SourceRect {
    let scale = match fit {
        FitMode::Original => 1.0f64.min(f64::from(viewport.width) / f64::from(width))
            .min(f64::from(viewport.height) / f64::from(height)),
        FitMode::Height => f64::from(viewport.height) / f64::from(height),
        FitMode::Width => f64::from(viewport.width) / f64::from(width),
        FitMode::Page => return SourceRect::whole(viewport.width, viewport.height),
    };
    let width = (f64::from(width) * scale).round().clamp(1.0, f64::from(viewport.width)) as u32;
    let height = (f64::from(height) * scale).round().clamp(1.0, f64::from(viewport.height)) as u32;
    SourceRect { x: (viewport.width - width) / 2, y: (viewport.height - height) / 2, width, height }
}

fn assert_near_bounds(actual: SourceRect, expected: SourceRect) {
    assert!(actual.x.abs_diff(expected.x) <= 1, "Horizontal placement differs: {actual:?} != {expected:?}");
    assert!(actual.y.abs_diff(expected.y) <= 1, "Vertical placement differs: {actual:?} != {expected:?}");
    assert!(actual.width.abs_diff(expected.width) <= 1, "Width differs: {actual:?} != {expected:?}");
    assert!(actual.height.abs_diff(expected.height) <= 1, "Height differs: {actual:?} != {expected:?}");
}

fn assert_quadrants(frame: &RenderedPage, bounds: SourceRect, expected: [u8; 4]) {
    let xs = [bounds.x + bounds.width / 4, bounds.x + bounds.width * 3 / 4];
    let ys = [bounds.y + bounds.height / 4, bounds.y + bounds.height * 3 / 4];
    assert_eq!([pixel(frame, xs[0], ys[0]), pixel(frame, xs[1], ys[0]),
                pixel(frame, xs[0], ys[1]), pixel(frame, xs[1], ys[1])], expected);
}

#[test]
fn all_four_fit_keys_round_trip_without_changing_persistence_names() {
    for fit in [FitMode::Original, FitMode::Page, FitMode::Height, FitMode::Width] {
        assert_eq!(FitMode::parse(fit.as_str()), Some(fit));
        let value = serde_json::to_value(fit).unwrap();
        assert_eq!(value.as_str(), Some(fit.as_str()));
        assert_eq!(serde_json::from_value::<FitMode>(value).unwrap(), fit);
    }
    assert_eq!(serde_json::from_str::<ReaderSettings>("{}").unwrap().fit, FitMode::Page);
    assert_eq!(serde_json::from_str::<ReaderSettings>(r#"{"fit":"width"}"#).unwrap().fit, FitMode::Width);
    assert_eq!(FitMode::parse("stretch"), None);
}

#[test]
fn unit_fits_match_independent_geometry_and_centered_pattern_at_two_viewports() {
    for viewport in [Viewport::new(120, 100), Viewport::new(83, 101)] {
        for (width, height) in [(80, 40), (277, 231), (400, 100), (100, 400)] {
            let source = quadrants(width, height);
            for fit in [FitMode::Original, FitMode::Page, FitMode::Height, FitMode::Width] {
                let frame = reader(&source, fit).render_source(&source, viewport).unwrap();
                let bounds = ink_bounds(&frame);
                assert_near_bounds(bounds, ideal_output(width, height, viewport, fit));
                assert_quadrants(&frame, bounds, [17, 68, 136, 204]);
                assert_eq!((frame.crop_index, frame.crop_count), (0, 1));
                if fit == FitMode::Page { assert_eq!(bounds, SourceRect::whole(viewport.width, viewport.height)); }
            }
        }
    }
}

#[test]
fn original_keeps_small_sources_pixel_exact_without_enlarging_them() {
    let source = GrayImage::from_fn(73, 47, |x, y| Luma([((x * 7 + y * 13) % 256) as u8]));
    let viewport = Viewport::new(200, 160);
    let frame = reader(&source, FitMode::Original).render_source(&source, viewport).unwrap();
    assert_eq!(frame.crop, SourceRect::whole(73, 47));
    let left = (200 - 73) / 2;
    let top = (160 - 47) / 2;
    for y in 0..47 { for x in 0..73 {
        assert_eq!(pixel(&frame, left + x, top + y), source.get_pixel(x, y)[0]);
    } }
    assert_eq!(pixel(&frame, left - 1, top), 255);
    assert_eq!(pixel(&frame, left + 73, top), 255);
}

#[test]
fn original_reduces_large_sources_to_the_viewport_without_cropping_or_extra_steps() {
    let source = GrayImage::from_pixel(400, 600, Luma([17]));
    for viewport in [Viewport::new(200, 120), Viewport::new(126, 168)] {
        let frame = reader(&source, FitMode::Original).render_source(&source, viewport).unwrap();
        assert_eq!(frame.crop, SourceRect::whole(400, 600));
        assert_near_bounds(ink_bounds(&frame), ideal_output(400, 600, viewport, FitMode::Original));
        assert_eq!(frame.crop_count, 1);
    }
    let source = GrayImage::from_pixel(64, 64, Luma([17]));
    let frame = reader(&source, FitMode::Original).render_source(&source, Viewport::new(1, 1)).unwrap();
    assert_eq!(frame.rgba, [17, 17, 17, 255]);
    assert_eq!(frame.crop_count, 1);
}

#[test]
fn page_stretches_the_complete_source_across_both_viewport_axes() {
    let source = quadrants(80, 40);
    for viewport in [Viewport::new(120, 100), Viewport::new(90, 150)] {
        let frame = reader(&source, FitMode::Page).render_source(&source, viewport).unwrap();
        assert_eq!(frame.crop, SourceRect::whole(80, 40));
        assert_eq!(ink_bounds(&frame), SourceRect::whole(viewport.width, viewport.height));
        assert_eq!([pixel(&frame, 0, 0), pixel(&frame, viewport.width - 1, 0),
                    pixel(&frame, 0, viewport.height - 1), pixel(&frame, viewport.width - 1, viewport.height - 1)],
                   [17, 68, 136, 204]);
        assert_quadrants(&frame, SourceRect::whole(viewport.width, viewport.height), [17, 68, 136, 204]);
    }
}

#[test]
fn height_and_width_show_the_central_source_area_instead_of_the_first_segment() {
    let wide = GrayImage::from_fn(400, 100, |x, y| Luma([if x < 140 { 17 } else if x >= 260 { 238 }
        else if y < 50 { 68 } else { 170 }]));
    let frame = reader(&wide, FitMode::Height).render_source(&wide, Viewport::new(120, 100)).unwrap();
    assert_eq!(frame.crop, SourceRect { x: 140, y: 0, width: 120, height: 100 });
    assert_eq!([pixel(&frame, 0, 0), pixel(&frame, 119, 0), pixel(&frame, 0, 99), pixel(&frame, 119, 99)],
               [68, 68, 170, 170]);
    let tall = GrayImage::from_fn(100, 400, |x, y| Luma([if y < 140 { 17 } else if y >= 260 { 238 }
        else if x < 50 { 68 } else { 170 }]));
    let frame = reader(&tall, FitMode::Width).render_source(&tall, Viewport::new(100, 120)).unwrap();
    assert_eq!(frame.crop, SourceRect { x: 0, y: 140, width: 100, height: 120 });
    assert_eq!([pixel(&frame, 0, 0), pixel(&frame, 99, 0), pixel(&frame, 0, 119), pixel(&frame, 99, 119)],
               [68, 170, 68, 170]);
}

#[test]
fn short_width_and_narrow_height_fits_are_centered_on_the_other_axis() {
    let wide = GrayImage::from_pixel(400, 100, Luma([17]));
    let frame = reader(&wide, FitMode::Width).render_source(&wide, Viewport::new(200, 300)).unwrap();
    assert_eq!(ink_bounds(&frame), SourceRect { x: 0, y: 125, width: 200, height: 50 });
    let narrow = GrayImage::from_pixel(100, 400, Luma([17]));
    let frame = reader(&narrow, FitMode::Height).render_source(&narrow, Viewport::new(300, 200)).unwrap();
    assert_eq!(ink_bounds(&frame), SourceRect { x: 125, y: 0, width: 50, height: 200 });
}

#[test]
fn fractional_upscale_crops_sample_the_true_center_at_bounded_output_size() {
    let wide = GrayImage::from_fn(4, 2, |x, _| Luma([if x < 2 { 17 } else { 238 }]));
    let mut core = reader(&wide, FitMode::Height);
    let frame = core.render_source(&wide, Viewport::new(3, 8)).unwrap();
    for y in 0..8 { assert_eq!([pixel(&frame, 0, y), pixel(&frame, 1, y), pixel(&frame, 2, y)], [72, 128, 183]); }
    assert_eq!(frame.crop, SourceRect { x: 1, y: 0, width: 2, height: 2 });
    assert!((core.anchor(0).source_x - 0.40625).abs() < 1e-12);
    let tall = GrayImage::from_fn(2, 4, |_, y| Luma([if y < 2 { 17 } else { 238 }]));
    let frame = reader(&tall, FitMode::Width).render_source(&tall, Viewport::new(8, 3)).unwrap();
    for x in 0..8 { assert_eq!([pixel(&frame, x, 0), pixel(&frame, x, 1), pixel(&frame, x, 2)], [72, 128, 183]); }
    let extreme = GrayImage::from_fn(5000, 1, |x, _| Luma([if x < 2500 { 17 } else { 238 }]));
    let mut core = reader(&extreme, FitMode::Height);
    let mut profile = ReaderPreparationProfile::default();
    let frame = core.render_source_identity(&extreme, Viewport::new(9, 4000), None, Some(&mut profile)).unwrap();
    assert_eq!(frame.rgba.len(), 9 * 4000 * 4);
    assert_eq!(profile.resized_pixels, 9 * 4000);
    assert_eq!(pixel(&frame, 4, 2000), 128);
    assert_eq!(frame.crop_count, 1);
}

#[test]
fn manual_rotations_keep_the_identifiable_pattern_and_unit_preset_geometry() {
    let source = quadrants(80, 40);
    for viewport in [Viewport::new(120, 100), Viewport::new(83, 101)] {
        for (rotation, size, colors) in [(0, (80, 40), [17, 68, 136, 204]),
            (90, (40, 80), [136, 17, 204, 68]), (180, (80, 40), [204, 136, 68, 17]),
            (270, (40, 80), [68, 204, 17, 136])] {
            for fit in [FitMode::Original, FitMode::Page, FitMode::Height, FitMode::Width] {
                let mut core = ReaderCore::new(index(80, 40), ReaderSettings { rotation, ..settings(fit) }).unwrap();
                let frame = core.render_source(&source, viewport).unwrap();
                assert_eq!((frame.source_width, frame.source_height), size);
                let bounds = ink_bounds(&frame);
                assert_near_bounds(bounds, ideal_output(size.0, size.1, viewport, fit));
                assert_quadrants(&frame, bounds, colors);
                assert_eq!(frame.crop_count, 1);
            }
        }
    }
}

#[test]
fn unit_presets_never_add_page_steps_or_move_the_center_for_reading_direction() {
    let source = quadrants(277, 231);
    let viewport = Viewport::new(160, 120);
    for fit in [FitMode::Original, FitMode::Page, FitMode::Height, FitMode::Width] {
        let mut forward = reader(&source, fit);
        let first = forward.render_source(&source, viewport).unwrap();
        let mut reverse = ReaderCore::new(index(277, 231), ReaderSettings {
            direction: PageDirection::RightToLeft, ..settings(fit)
        }).unwrap();
        let rtl = reverse.render_source(&source, viewport).unwrap();
        assert_eq!(rtl.crop, first.crop);
        assert_eq!(rtl.rgba, first.rgba);
        assert_eq!(forward.step(true), ReaderMove::Moved);
        assert_eq!(forward.page_index(), 1);
        assert_eq!(forward.step(false), ReaderMove::Moved);
        let back = forward.render_source(&source, viewport).unwrap();
        assert_eq!(forward.page_index(), 0);
        assert_eq!(back.crop, first.crop);
        assert_eq!(back.rgba, first.rgba);
    }
}

#[test]
fn unit_fit_anchors_restore_the_same_page_rotation_and_pixels() {
    let source = quadrants(109, 157);
    let viewport = Viewport::new(83, 101);
    for fit in [FitMode::Original, FitMode::Page, FitMode::Height, FitMode::Width] {
        for rotation in [0, 90, 180, 270] {
            let options = ReaderSettings { rotation, ..settings(fit) };
            let mut core = ReaderCore::new(index(109, 157), options.clone()).unwrap();
            core.set_page(1).unwrap();
            let selected = core.render_source(&source, viewport).unwrap();
            let anchor = core.anchor(123);
            let repeated = core.render_source(&source, viewport).unwrap();
            assert_eq!(repeated.crop, selected.crop);
            assert_eq!(repeated.rgba, selected.rgba);
            assert_eq!(core.anchor(123), anchor);
            let mut restored = ReaderCore::new(index(109, 157), options).unwrap();
            restored.restore(&anchor).unwrap();
            let frame = restored.render_source(&source, viewport).unwrap();
            assert_eq!(frame.crop, selected.crop);
            assert_eq!(frame.rgba, selected.rgba);
            assert_eq!(restored.anchor(123), anchor);
            assert_eq!(restored.page_index(), 1);
        }
    }
}

#[test]
fn internal_zoom_still_changes_pixels_without_adding_reading_regions() {
    let source = GrayImage::from_fn(37, 21, |x, y| Luma([((x * 7 + y * 13) % 256) as u8]));
    let viewport = Viewport::new(83, 101);
    for fit in [FitMode::Original, FitMode::Page, FitMode::Height, FitMode::Width] {
        let mut core = reader(&source, fit);
        let before = core.render_source(&source, viewport).unwrap();
        let mut options = core.settings().clone();
        options.zoom_ratio = 2.0;
        core.update_settings(options).unwrap();
        let after = core.render_source(&source, viewport).unwrap();
        assert_eq!((after.page_index, after.crop_count), (0, 1));
        assert_ne!(after.rgba, before.rgba);
    }
}

#[test]
fn explicit_nonunit_pan_restores_its_visible_source_origin_after_rotation() {
    let source = GrayImage::from_fn(109, 157, |x, y| Luma([((x * 7 + y * 13) % 256) as u8]));
    let viewport = Viewport::new(83, 101);
    for rotation in [0, 90] {
        let options = ReaderSettings { rotation, zoom_ratio: 1.5, ..settings(FitMode::Width) };
        let mut core = ReaderCore::new(index(109, 157), options.clone()).unwrap();
        core.render_source(&source, viewport).unwrap();
        core.pan_to(0.1, 0.2).unwrap();
        let panned = core.render_source(&source, viewport).unwrap();
        let anchor = core.anchor(123);
        let mut restored = ReaderCore::new(index(109, 157), options).unwrap();
        restored.restore(&anchor).unwrap();
        let repeated = restored.render_source(&source, viewport).unwrap();
        assert_eq!(repeated.crop, panned.crop);
        assert_eq!(repeated.rgba, panned.rgba);
        assert!((restored.anchor(123).source_x - anchor.source_x).abs() < 1e-12);
        assert!((restored.anchor(123).source_y - anchor.source_y).abs() < 1e-12);
    }
}

#[test]
fn page_stretch_of_extreme_aspects_never_needs_an_enlarged_intermediate_matrix() {
    let viewport = Viewport::new(93, 124);
    let wide = quadrants(1_000_000, 2);
    let mut core = reader(&wide, FitMode::Page);
    let frame = core.render_source(&wide, viewport).unwrap();
    assert_eq!(ink_bounds(&frame), SourceRect::whole(93, 124));
    assert_eq!([pixel(&frame, 5, 0), pixel(&frame, 87, 0), pixel(&frame, 5, 123), pixel(&frame, 87, 123)],
               [17, 68, 136, 204]);
    assert_eq!(frame.crop_count, 1);
    let cancelled = AtomicBool::new(false);
    let mut cancellable = reader(&wide, FitMode::Page);
    let repeated = cancellable.render_source_identity_cancellable(&wide, viewport, None,
        &RenderCancellation::for_flag(&cancelled), None).unwrap().unwrap();
    assert_eq!(repeated.rgba, frame.rgba);
    let abort = AtomicBool::new(false);
    let probe = |work, completed| {
        if work == RenderWork::ResizeHorizontal && completed > 0 { abort.store(true, Ordering::Release); }
    };
    let mut interrupted = reader(&wide, FitMode::Page);
    let mut profile = ReaderPreparationProfile::default();
    let frame = interrupted.render_source_identity_cancellable(&wide, viewport, None,
        &RenderCancellation::with_probe(&abort, &probe), Some(&mut profile)).unwrap();
    assert!(frame.is_none());
    assert_eq!(profile.rendered_frames, 0);
    let tall = quadrants(2, 1_000_000);
    let frame = reader(&tall, FitMode::Page).render_source(&tall, Viewport::new(124, 93)).unwrap();
    assert_eq!(ink_bounds(&frame), SourceRect::whole(124, 93));
    assert_eq!([pixel(&frame, 0, 5), pixel(&frame, 123, 5), pixel(&frame, 0, 87), pixel(&frame, 123, 87)],
               [17, 68, 136, 204]);
}

#[test]
fn strip_and_panel_region_planning_remain_separate_from_fit_presets() {
    let source = quadrants(100, 400);
    let viewport = Viewport::new(100, 100);
    let mut strip_counts = Vec::new();
    for fit in [FitMode::Original, FitMode::Page, FitMode::Height, FitMode::Width] {
        let mut strip = ReaderCore::new(index(100, 400), ReaderSettings { mode: DisplayMode::Strip, ..settings(fit) }).unwrap();
        let first = strip.render_source(&source, viewport).unwrap();
        assert!(first.crop_count > 1);
        strip_counts.push(first.crop_count);
        assert_eq!(strip.step(true), ReaderMove::Moved);
        assert_eq!(strip.page_index(), 0);
        assert_eq!(strip.crop_index(), 1);
        let mut panel = ReaderCore::new(index(100, 400), ReaderSettings { mode: DisplayMode::Panel, ..settings(fit) }).unwrap();
        panel.set_panel_map("fit-page-0", vec![
            NormalizedRect { x: 0.0, y: 0.0, width: 1.0, height: 0.5 },
            NormalizedRect { x: 0.0, y: 0.5, width: 1.0, height: 0.5 },
        ]).unwrap();
        let first = panel.render_source(&source, viewport).unwrap();
        assert_eq!(first.crop_count, 2);
        assert!(first.crop.y + first.crop.height <= 200);
        assert_eq!(panel.step(true), ReaderMove::Moved);
        let second = panel.render_source(&source, viewport).unwrap();
        assert_eq!(second.crop_count, 2);
        assert!(second.crop.y >= 200);
    }
    assert!(strip_counts.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn fractional_upscale_sampling_obeys_worker_cancellation_without_partial_frames() {
    let source = quadrants(4, 2);
    let viewport = Viewport::new(300, 800);
    let cancelled = AtomicBool::new(false);
    let probe = |work, completed| {
        if work == RenderWork::ResizeHorizontal && completed > 0 { cancelled.store(true, Ordering::Release); }
    };
    let mut core = reader(&source, FitMode::Height);
    let mut profile = ReaderPreparationProfile::default();
    let frame = core.render_source_identity_cancellable(&source, viewport, None,
        &RenderCancellation::with_probe(&cancelled, &probe), Some(&mut profile)).unwrap();
    assert!(frame.is_none());
    assert!(cancelled.load(Ordering::Acquire));
    assert_eq!(profile.rendered_frames, 0);
    assert_eq!((profile.stages["resize"].completed, profile.stages["resize"].partial), (0, 1));
}

#[test]
fn normal_reference_and_cancellable_rendering_agree_on_new_fit_pixels() {
    let source = quadrants(37, 21);
    let viewport = Viewport::new(83, 101);
    let cancelled = AtomicBool::new(false);
    for fit in [FitMode::Original, FitMode::Page, FitMode::Height, FitMode::Width] {
        for rotation in [0, 90, 180, 270] {
            let options = ReaderSettings { rotation, ..settings(fit) };
            let mut reference = ReaderCore::new(index(37, 21), options.clone()).unwrap();
            reference.raster_path = TestRasterPath::Reference;
            let expected = reference.render_source(&source, viewport).unwrap();
            let mut normal = ReaderCore::new(index(37, 21), options.clone()).unwrap();
            let frame = normal.render_source(&source, viewport).unwrap();
            let mut cancellable = ReaderCore::new(index(37, 21), options).unwrap();
            let actual = cancellable.render_source_identity_cancellable(&source, viewport, None,
                &RenderCancellation::for_flag(&cancelled), None).unwrap().unwrap();
            for actual in [frame, actual] {
                assert_eq!(actual.crop, expected.crop);
                assert_eq!(actual.rgba, expected.rgba);
                assert_eq!((actual.crop_index, actual.crop_count), (expected.crop_index, expected.crop_count));
            }
        }
    }
}

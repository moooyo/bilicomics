//! Deterministic work-block cancellation must never publish a partial raster.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use image::{GrayImage, Luma, imageops::FilterType};

use super::*;
use crate::domain::PageDescriptor;

fn source(width: u32, height: u32) -> GrayImage {
    GrayImage::from_fn(width, height, |x, y| Luma([((x * 37 + y * 61) % 256) as u8]))
}

fn index(width: u32, height: u32) -> ImageIndex {
    ImageIndex {
        episode_id: "render-cancellation-episode".into(),
        revision: "render-cancellation-revision".into(),
        images: vec![PageDescriptor {
            id: "render-cancellation-page".into(), index: 0, width, height,
        }],
        ..Default::default()
    }
}

fn assert_frame(actual: &RenderedPage, expected: &RenderedPage) {
    assert_eq!((actual.width, actual.height), (expected.width, expected.height));
    assert_eq!((actual.source_width, actual.source_height), (expected.source_width, expected.source_height));
    assert_eq!((actual.page_index, actual.crop_index, actual.crop_count),
        (expected.page_index, expected.crop_index, expected.crop_count));
    assert_eq!(actual.crop, expected.crop);
    assert_eq!(actual.overlap_from_previous, expected.overlap_from_previous);
    assert_eq!(actual.heuristic_panels, expected.heuristic_panels);
    assert_eq!(actual.dark_fraction, expected.dark_fraction);
    assert_eq!(actual.rgba, expected.rgba);
}

fn assert_partial(profile: &ReaderPreparationProfile, name: &'static str) {
    let stage = &profile.stages[name];
    assert_eq!((stage.attempts, stage.completed, stage.partial), (1, 0, 1));
    assert_eq!(profile.rendered_frames, 0);
    assert_eq!(profile.output_bytes, 0);
}

fn cancel_during_frame(source: &GrayImage, viewport: Viewport,
    target: fn(RenderWork) -> bool, stage: &'static str)
{
    let cancelled = AtomicBool::new(false);
    let saw_start = AtomicBool::new(false);
    let processed = AtomicUsize::new(0);
    let probe = |work, completed| {
        if target(work) {
            if completed == 0 {
                saw_start.store(true, Ordering::Release);
            } else if !cancelled.load(Ordering::Acquire) {
                processed.store(completed, Ordering::Release);
                cancelled.store(true, Ordering::Release);
            }
        }
    };
    let cancellation = RenderCancellation::with_probe(&cancelled, &probe);
    let mut core = ReaderCore::new(index(source.width(), source.height()),
        ReaderSettings { landscape_spread: false, ..Default::default() }).unwrap();
    let mut profile = ReaderPreparationProfile::default();
    let result = core.render_source_identity_cancellable(source, viewport,
        Some("render-cancellation-source"), &cancellation, Some(&mut profile)).unwrap();
    assert!(saw_start.load(Ordering::Acquire));
    assert!(processed.load(Ordering::Acquire) > 0);
    assert!(processed.load(Ordering::Acquire) < source.as_raw().len());
    assert!(cancelled.load(Ordering::Acquire));
    assert!(result.is_none(), "An interrupted stage must not return a partial frame");
    assert_partial(&profile, stage);
    if stage == "resize" {
        assert!(!profile.stages.contains_key("canvas"));
    } else {
        let resize = &profile.stages["resize"];
        assert_eq!((resize.attempts, resize.completed, resize.partial), (1, 1, 0));
    }
}

#[test]
fn cancelling_a_vertical_resize_block_does_not_return_or_complete_a_frame() {
    cancel_during_frame(&source(361, 517), Viewport::new(400, 600),
        |work| matches!(work, RenderWork::ResizeVertical), "resize");
}

#[test]
fn cancelling_a_horizontal_resize_block_does_not_return_or_complete_a_frame() {
    cancel_during_frame(&source(361, 517), Viewport::new(400, 600),
        |work| matches!(work, RenderWork::ResizeHorizontal), "resize");
}

#[test]
fn cancelling_a_canvas_pixel_block_does_not_return_or_complete_a_frame() {
    cancel_during_frame(&source(361, 517), Viewport::new(400, 600),
        |work| matches!(work, RenderWork::CanvasPixels), "canvas");
}

#[test]
fn cancelling_a_canvas_fill_block_does_not_return_or_complete_a_frame() {
    cancel_during_frame(&source(361, 517), Viewport::new(400, 600),
        |work| matches!(work, RenderWork::CanvasFill), "canvas");
}

#[test]
fn cancelling_long_weight_generation_records_partial_resize_work() {
    let source = source(29, 4097);
    let cancelled = AtomicBool::new(false);
    let saw_start = AtomicBool::new(false);
    let coefficients = AtomicUsize::new(0);
    let probe = |work, completed| {
        if matches!(work, RenderWork::ResizeWeights) {
            if completed == 0 {
                saw_start.store(true, Ordering::Release);
            } else if !cancelled.load(Ordering::Acquire) {
                coefficients.store(completed, Ordering::Release);
                cancelled.store(true, Ordering::Release);
            }
        }
    };
    let cancellation = RenderCancellation::with_probe(&cancelled, &probe);
    let mut core = ReaderCore::new(index(source.width(), source.height()),
        ReaderSettings { landscape_spread: false, ..Default::default() }).unwrap();
    let mut profile = ReaderPreparationProfile::default();
    let result = core.render_source_identity_cancellable(&source, Viewport::new(47, 17),
        Some("long-weight-cancellation-source"), &cancellation, Some(&mut profile)).unwrap();
    assert!(result.is_none());
    assert!(saw_start.load(Ordering::Acquire));
    assert!(coefficients.load(Ordering::Acquire) > 0);
    // The first downsampled row needs more coefficients than this lower bound.
    // Cancellation must occur inside that weight computation, not after it.
    assert!(coefficients.load(Ordering::Acquire) < source.height() as usize / 17);
    assert_partial(&profile, "resize");
    assert!(!profile.stages.contains_key("canvas"));
}

#[test]
fn cancelling_a_large_exact_copy_never_returns_a_partially_copied_image() {
    let mut source = source(263, 269);
    source.set_color_space(image::metadata::Cicp::DISPLAY_P3).unwrap();
    let crop = SourceRect { x: 3, y: 5, width: 256, height: 257 };
    let cancelled = AtomicBool::new(false);
    let saw_start = AtomicBool::new(false);
    let copied = AtomicUsize::new(0);
    let probe = |work, completed| {
        if matches!(work, RenderWork::ResizeCopy) {
            if completed == 0 {
                saw_start.store(true, Ordering::Release);
            } else if !cancelled.load(Ordering::Acquire) {
                copied.store(completed, Ordering::Release);
                cancelled.store(true, Ordering::Release);
            }
        }
    };
    let cancellation = RenderCancellation::with_probe(&cancelled, &probe);
    let result = triangle::resize_cancellable(&source, crop, crop.width, crop.height, &cancellation);
    assert!(saw_start.load(Ordering::Acquire));
    assert!(copied.load(Ordering::Acquire) > 0);
    assert!(copied.load(Ordering::Acquire) < crop.width as usize * crop.height as usize);
    assert!(result.is_none());
    assert_eq!(source.color_space(), image::metadata::Cicp::DISPLAY_P3);
}

#[test]
fn uncancelled_empty_exact_copy_and_cropped_resize_preserve_reference_metadata() {
    let cancelled = AtomicBool::new(false);
    let probe = |_: RenderWork, _: usize| {};
    let cancellation = RenderCancellation::with_probe(&cancelled, &probe);
    for color_space in [image::metadata::Cicp::DISPLAY_P3, image::metadata::Cicp::SRGB_LINEAR] {
        for (width, height) in [(0, 0), (0, 7), (9, 0), (9, 7)] {
            let mut source = source(width, height);
            source.set_color_space(color_space).unwrap();
            let crop = SourceRect::whole(width, height);
            for (target_width, target_height) in [(0, 0), (0, 13), (11, 0), (width, height), (11, 13)] {
                let view = image::imageops::crop_imm(&source, crop.x, crop.y, crop.width, crop.height);
                let expected = image::imageops::resize(&*view, target_width, target_height, FilterType::Triangle);
                let actual = triangle::resize_cancellable(&source, crop, target_width, target_height, &cancellation)
                    .expect("An uncancelled resize must return the complete reference image");
                assert_eq!(actual.dimensions(), expected.dimensions());
                assert_eq!(actual.color_space(), expected.color_space());
                assert_eq!(actual.as_raw(), expected.as_raw());
            }
        }
        let mut source = source(29, 41);
        source.set_color_space(color_space).unwrap();
        let crop = SourceRect { x: 3, y: 5, width: 17, height: 23 };
        for (width, height) in [(17, 23), (31, 47), (1, 1), (9, 11)] {
            let view = image::imageops::crop_imm(&source, crop.x, crop.y, crop.width, crop.height);
            let expected = image::imageops::resize(&*view, width, height, FilterType::Triangle);
            let actual = triangle::resize_cancellable(&source, crop, width, height, &cancellation).unwrap();
            assert_eq!(actual.dimensions(), expected.dimensions());
            assert_eq!(actual.color_space(), expected.color_space());
            assert_eq!(actual.as_raw(), expected.as_raw());
        }
    }
    assert!(!cancelled.load(Ordering::Acquire));
}

#[test]
fn uncancelled_long_vertical_and_horizontal_weights_match_both_reference_paths() {
    for color_space in [image::metadata::Cicp::DISPLAY_P3, image::metadata::Cicp::SRGB_LINEAR] {
        for (source_width, source_height, width, height, long_vertical) in [
            (29, 4097, 47, 17, true),
            (4097, 29, 17, 47, false),
        ] {
            let mut source = source(source_width, source_height);
            source.set_color_space(color_space).unwrap();
            let crop = SourceRect::whole(source_width, source_height);
            let view = image::imageops::crop_imm(&source, crop.x, crop.y, crop.width, crop.height);
            let expected = image::imageops::resize(&*view, width, height, FilterType::Triangle);
            let normal = triangle::resize(&source, crop, width, height);
            let cancelled = AtomicBool::new(false);
            let vertical_zero_checks = AtomicUsize::new(0);
            let horizontal_zero_checks = AtomicUsize::new(0);
            let probe = |work, completed| {
                if completed == 0 {
                    match work {
                        RenderWork::ResizeVertical => { vertical_zero_checks.fetch_add(1, Ordering::Relaxed); },
                        RenderWork::ResizeHorizontal => { horizontal_zero_checks.fetch_add(1, Ordering::Relaxed); },
                        _ => {},
                    }
                }
            };
            let cancellation = RenderCancellation::with_probe(&cancelled, &probe);
            let interruptible = triangle::resize_cancellable(&source, crop, width, height, &cancellation)
                .expect("Uncancelled long-weight accumulation must return a complete image");
            for actual in [&normal, &interruptible] {
                assert_eq!(actual.dimensions(), expected.dimensions());
                assert_eq!(actual.color_space(), expected.color_space());
                assert_eq!(actual.as_raw(), expected.as_raw());
            }
            // These downsampling ratios require more than 64 coefficients.
            // Repeated checks at the first pixel also observe the inner long
            // accumulation blocks, beyond the outer output-pixel block check.
            let inner_checks = if long_vertical { &vertical_zero_checks } else { &horizontal_zero_checks };
            assert!(inner_checks.load(Ordering::Acquire) > 1);
            assert!(!cancelled.load(Ordering::Acquire));
        }
    }
}

#[test]
fn uncancelled_preparation_matches_reference_and_normal_frame_geometry_and_anchors() {
    let source = source(109, 157);
    let cancelled = AtomicBool::new(false);
    let probe = |_: RenderWork, _: usize| {};
    let cancellation = RenderCancellation::with_probe(&cancelled, &probe);
    let viewport = Viewport::new(83, 101);
    for (mode, fit, rotation, zoom_ratio, dither) in [
        (DisplayMode::Page, FitMode::Page, 0, 1.0, true),
        (DisplayMode::Page, FitMode::Page, 180, 1.5, false),
        (DisplayMode::Strip, FitMode::Width, 90, 1.0, true),
        (DisplayMode::Panel, FitMode::Page, 270, 1.5, true),
    ] {
        let settings = ReaderSettings { mode, fit, rotation, zoom_ratio, dither,
            landscape_spread: false, ..Default::default() };
        let mut normal = ReaderCore::new(index(source.width(), source.height()), settings).unwrap();
        if mode == DisplayMode::Panel {
            normal.set_panel_map("render-cancellation-page", vec![
                NormalizedRect { x: 0.1, y: 0.15, width: 0.75, height: 0.3 },
                NormalizedRect { x: 0.0, y: 0.5, width: 1.0, height: 0.5 },
            ]).unwrap();
        }
        normal.pan_to(0.35, 0.42).unwrap();
        let mut reference = normal.clone();
        reference.raster_path = TestRasterPath::Reference;
        let mut interruptible = normal.clone();
        let expected = reference.render_source_identity(&source, viewport,
            Some("complete-cancellation-fixture"), None).unwrap();
        let unprofiled = normal.render_source_identity(&source, viewport,
            Some("complete-cancellation-fixture"), None).unwrap();
        let mut profile = ReaderPreparationProfile::default();
        let actual = interruptible.render_source_identity_cancellable(&source, viewport,
            Some("complete-cancellation-fixture"), &cancellation, Some(&mut profile)).unwrap().unwrap();
        assert_frame(&actual, &expected);
        assert_frame(&unprofiled, &expected);
        assert_eq!(interruptible.anchor(123), reference.anchor(123));
        assert_eq!(normal.anchor(123), reference.anchor(123));
        assert_eq!(profile.rendered_frames, 1);
        assert_eq!(profile.output_bytes, u64::from(viewport.width) * u64::from(viewport.height) * 4);
        for name in ["rotation", "layout", "resize", "canvas"] {
            let stage = &profile.stages[name];
            assert_eq!((stage.attempts, stage.completed, stage.partial), (1, 1, 0));
        }
    }
}

#[test]
fn the_public_default_render_api_still_returns_a_complete_uncancelled_frame() {
    let source = source(109, 157);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("default-render.png");
    source.save(&path).unwrap();
    let settings = ReaderSettings { mode: DisplayMode::Strip, fit: FitMode::Width,
        rotation: 90, landscape_spread: false, ..Default::default() };
    let mut normal = ReaderCore::new(index(source.width(), source.height()), settings.clone()).unwrap();
    let mut interruptible = ReaderCore::new(index(source.width(), source.height()), settings).unwrap();
    let viewport = Viewport::new(83, 101);
    let expected = normal.render(&path, viewport).unwrap();
    let cancelled = AtomicBool::new(false);
    let probe = |_: RenderWork, _: usize| {};
    let cancellation = RenderCancellation::with_probe(&cancelled, &probe);
    let actual = interruptible.render_source_identity_cancellable(&source, viewport,
        Some("public-default-cancellation-fixture"), &cancellation, None).unwrap().unwrap();
    assert_frame(&actual, &expected);
    assert_eq!(interruptible.anchor(123), normal.anchor(123));
}

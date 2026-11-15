//! Host-only binary canvas, source-cache, geometry, and cancellation regressions.

use super::*;
use crate::domain::{PageDescriptor, ReadingAnchor};
use image::Luma;
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::atomic::AtomicUsize,
};

const BAYER_RANKS: [[u16; 4]; 4] = [
    [0, 8, 2, 10],
    [12, 4, 14, 6],
    [3, 11, 1, 9],
    [15, 7, 13, 5],
];

fn binary_reference(value: u8, x: usize, y: usize) -> u8 {
    let threshold = BAYER_RANKS[y % 4][x % 4] * 16 + 8;
    if u16::from(value) >= threshold { 255 } else { 0 }
}

fn normal_reference(value: u8, x: usize, y: usize, dither: bool, hardware: bool) -> u8 {
    if dither && hardware { return value; }
    let bias = if dither { BAYER_RANKS[y % 4][x % 4] as i16 - 8 } else { 0 };
    let adjusted = (i16::from(value) + bias).clamp(0, 255) as u16;
    (((adjusted + 8) / 17).min(15) * 17) as u8
}

fn index(width: u32, height: u32) -> ImageIndex {
    ImageIndex { episode_id: "fast-mode-episode".into(), revision: "fast-mode-revision".into(),
        images: (0..2).map(|position| PageDescriptor { id: format!("fast-mode-{position}"),
            index: position, width, height }).collect(), ..Default::default() }
}

fn source(width: u32, height: u32) -> GrayImage {
    GrayImage::from_fn(width, height, |x, y| Luma([(((x + y * width) * 37) % 256) as u8]))
}

fn png_fixture(width: u32, height: u32) -> (tempfile::TempDir, PathBuf, GrayImage) {
    let directory = tempfile::tempdir().expect("Create an isolated source fixture");
    let path = directory.path().join("fast-mode-source.png");
    let pixels = source(width, height);
    pixels.save(&path).expect("Encode the complete synthetic PNG");
    (directory, path, pixels)
}

fn assert_binary(rgba: &[u8]) {
    assert_eq!(rgba.len() % 4, 0);
    for pixel in rgba.chunks_exact(4) {
        assert!(matches!(pixel[0], 0 | 255), "Fast output contains an intermediate gray");
        assert_eq!(pixel, &[pixel[0], pixel[0], pixel[0], 255]);
    }
}

fn assert_geometry(actual: &RenderedPage, expected: &RenderedPage) {
    assert_eq!((actual.width, actual.height), (expected.width, expected.height));
    assert_eq!((actual.source_width, actual.source_height), (expected.source_width, expected.source_height));
    assert_eq!((actual.page_index, actual.crop_index, actual.crop_count),
        (expected.page_index, expected.crop_index, expected.crop_count));
    assert_eq!(actual.crop, expected.crop);
    assert_eq!(actual.overlap_from_previous, expected.overlap_from_previous);
    assert_eq!(actual.heuristic_panels, expected.heuristic_panels);
}

fn assert_anchor(actual: &ReadingAnchor, expected: &ReadingAnchor) {
    assert!((actual.source_x - expected.source_x).abs() <= 1e-12);
    assert!((actual.source_y - expected.source_y).abs() <= 1e-12);
    let mut normalized = actual.clone();
    normalized.source_x = expected.source_x;
    normalized.source_y = expected.source_y;
    assert_eq!(&normalized, expected);
}

#[test]
fn fast_defaults_off_and_suppresses_hardware_without_overwriting_saved_choices() {
    let legacy: ReaderSettings = serde_json::from_value(serde_json::json!({
        "dither": false, "hardware_dither": true
    })).expect("Read a settings record without a fast-mode field");
    assert!(!ReaderSettings::default().fast_mode && !legacy.fast_mode);
    assert!(!legacy.dither && legacy.hardware_dither);
    for dither in [false, true] {
        for hardware_dither in [false, true] {
            let normal = ReaderSettings { dither, hardware_dither, ..Default::default() };
            let fast = ReaderSettings { fast_mode: true, ..normal.clone() };
            assert_eq!(normal.uses_hardware_dither(), dither && hardware_dither);
            assert!(!fast.uses_hardware_dither());
            assert_eq!((fast.dither, fast.hardware_dither), (dither, hardware_dither));
        }
    }
}

#[test]
fn every_original_gray_and_bayer_phase_has_direct_binary_and_unchanged_normal_math() {
    for y in 0..4 {
        for x in 0..4 {
            for value in 0u8..=u8::MAX {
                for dither in [false, true] {
                    for hardware in [false, true] {
                        assert_eq!(canvas_tone(value, x, y, dither, hardware, true),
                            binary_reference(value, x, y),
                            "Direct Gray8 threshold differs for value {value}, phase ({x}, {y})");
                        assert_eq!(canvas_tone(value, x + 12, y + 8, dither, hardware, true),
                            binary_reference(value, x, y), "The Bayer phase must repeat every four pixels");
                        assert_eq!(canvas_tone(value, x, y, dither, hardware, false),
                            normal_reference(value, x, y, dither, hardware),
                            "Normal processing changed for value {value}, phase ({x}, {y})");
                    }
                }
            }
        }
    }
    for dither in [false, true] {
        let intermediate = normal_reference(8, 0, 0, dither, false);
        assert_eq!(intermediate, 0);
        assert_eq!(binary_reference(intermediate, 0, 0), 0);
        assert_eq!(canvas_tone(8, 0, 0, dither, true, true), 255,
            "Fast mode must not quantize to sixteen levels before applying the binary threshold");
    }
}

#[test]
fn both_canvas_paths_keep_binary_pixels_local_phase_white_margins_and_exact_dark_counts() {
    let pixels = GrayImage::from_fn(1024, 4, |x, _| Luma([(x / 4) as u8]));
    let original = pixels.as_raw().clone();
    let viewport = Viewport::new(1031, 11);
    let (left, top) = (3, 5);
    let mut expected = vec![255; viewport.width as usize * viewport.height as usize * 4];
    let mut dark = 0;
    for (x, y, pixel) in pixels.enumerate_pixels() {
        let tone = binary_reference(pixel[0], x as usize, y as usize);
        let offset = (((top + y) * viewport.width + left + x) * 4) as usize;
        expected[offset..offset + 4].copy_from_slice(&[tone, tone, tone, 255]);
        dark += u64::from(tone == 0);
    }
    assert_eq!(dark, 2048);
    let expected = (expected, dark);
    for dither in [false, true] {
        for hardware in [false, true] {
            let actual = paint_quantized_canvas(&pixels, viewport, left, top, dither, hardware, true);
            assert_eq!(actual, expected);
            assert_binary(&actual.0);
            assert_eq!(actual.0.chunks_exact(4).map(|pixel| pixel[0]).collect::<BTreeSet<_>>(),
                BTreeSet::from([0, 255]));
            let cancelled = AtomicBool::new(false);
            let cancellation = RenderCancellation::for_flag(&cancelled);
            assert_eq!(paint_quantized_canvas_cancellable(&pixels, viewport, left, top,
                dither, hardware, true, &cancellation), Some(expected.clone()));
        }
    }
    assert_eq!(pixels.as_raw(), &original);
}

#[test]
fn normal_fast_normal_reuses_the_original_gray8_proof_pixels_and_cache_allocation() {
    let (_directory, path, pixels) = png_fixture(80, 120);
    let resources = ReaderResources::new(1024 * 1024);
    let canonical = load_validated_source_with_resources(&path, &resources).unwrap();
    let proof = canonical.proof().clone();
    let original = canonical.pixels.as_raw().clone();
    let usage = resources.cache_usage();
    let processed_usage = resources.processed_cache_usage();
    assert_eq!(canonical.pixels.dimensions(), pixels.dimensions());
    assert_eq!(canonical.pixels.as_raw(), pixels.as_raw());
    assert_eq!((proof.width(), proof.height()), (80, 120));
    assert_eq!(usage.entries, 1);
    assert_eq!(original.iter().copied().collect::<BTreeSet<_>>().len(), 256);
    let viewport = Viewport::new(64, 80);
    for dither in [false, true] {
        for hardware_dither in [false, true] {
            let mut core = ReaderCore::new(index(80, 120), ReaderSettings {
                dither, hardware_dither, landscape_spread: false, ..Default::default()
            }).unwrap();
            core.set_page(1).unwrap();
            let normal = core.render_validated_source(&canonical, viewport).unwrap();
            let anchor = core.anchor(0);
            let preparation = core.source_preparation(viewport);
            let mut settings = core.settings().clone();
            settings.fast_mode = true;
            core.update_settings(settings).unwrap();
            assert_eq!(core.source_preparation(viewport), preparation);
            let fast = core.render_validated_source(&canonical, viewport).unwrap();
            assert_binary(&fast.rgba);
            assert_ne!(fast.rgba, normal.rgba);
            assert_geometry(&fast, &normal);
            assert_anchor(&core.anchor(0), &anchor);
            let mut settings = core.settings().clone();
            settings.fast_mode = false;
            core.update_settings(settings).unwrap();
            let restored = core.render_validated_source(&canonical, viewport).unwrap();
            assert_eq!(restored.rgba, normal.rgba);
            assert_eq!(restored.dark_fraction, normal.dark_fraction);
            assert_anchor(&core.anchor(0), &anchor);
            assert_eq!((core.settings().dither, core.settings().hardware_dither), (dither, hardware_dither));
            let reused = load_validated_source_with_resources(&path, &resources).unwrap();
            assert!(Arc::ptr_eq(&canonical.pixels, &reused.pixels));
            assert_eq!(reused.proof(), &proof);
            assert_eq!(canonical.pixels.as_raw(), &original);
            assert_eq!(resources.cache_usage(), usage);
            assert_eq!(resources.processed_cache_usage(), processed_usage);
        }
    }
}

#[test]
fn source_preparation_identity_is_shared_before_the_output_mode_is_applied() {
    for media_acceleration in [crate::native_media::Acceleration::Off,
        crate::native_media::Acceleration::DecodeAndResize]
    {
        for fit in [FitMode::Original, FitMode::Page, FitMode::Height, FitMode::Width] {
            let normal = ReaderCore::new(index(80, 120), ReaderSettings {
                fit, media_acceleration, landscape_spread: false, ..Default::default()
            }).unwrap();
            let mut fast = normal.clone();
            let mut settings = fast.settings().clone();
            settings.fast_mode = true;
            fast.update_settings(settings).unwrap();
            assert_ne!(normal.settings(), fast.settings());
            for viewport in [Viewport::new(64, 80), Viewport::new(32, 48)] {
                assert_eq!(normal.source_preparation(viewport), fast.source_preparation(viewport),
                    "Gray8 preparation and processed-source keys must not depend on binary output");
            }
        }
    }
}

#[test]
fn four_fits_rotation_zoom_and_active_source_anchor_survive_fast_round_trips() {
    let (_directory, path, _) = png_fixture(80, 120);
    let resources = ReaderResources::new(1024 * 1024);
    let canonical = load_validated_source_with_resources(&path, &resources).unwrap();
    let viewport = Viewport::new(64, 80);
    let cancelled = AtomicBool::new(false);
    for fit in [FitMode::Original, FitMode::Page, FitMode::Height, FitMode::Width] {
        for rotation in [0, 90, 180, 270] {
            for zoom_ratio in [1.0, 1.5] {
                let mut core = ReaderCore::new(index(80, 120), ReaderSettings {
                    fit, rotation, zoom_ratio, hardware_dither: true,
                    landscape_spread: false, ..Default::default()
                }).unwrap();
                core.set_page(1).unwrap();
                if zoom_ratio > 1.0 { core.pan_to(0.25, 0.125).unwrap(); }
                let baseline = core.render_validated_source(&canonical, viewport).unwrap();
                let anchor = core.anchor(0);
                let preparation = core.source_preparation(viewport);
                for fast_mode in [true, false] {
                    let mut settings = core.settings().clone();
                    settings.fast_mode = fast_mode;
                    core.update_settings(settings).unwrap();
                    let frame = core.render_validated_source_cancellable(&canonical, viewport,
                        &cancelled, None).unwrap().unwrap();
                    assert_geometry(&frame, &baseline);
                    assert_anchor(&core.anchor(0), &anchor);
                    assert_eq!((core.settings().fit, core.settings().rotation, core.settings().zoom_ratio),
                        (fit, rotation, zoom_ratio));
                    assert_eq!(core.source_preparation(viewport), preparation);
                    if fast_mode { assert_binary(&frame.rgba); }
                }
            }
        }
    }
}

#[test]
fn a_pre_cancelled_fast_render_does_not_prepare_or_return_a_frame() {
    let pixels = source(80, 120);
    let original = pixels.as_raw().clone();
    let mut core = ReaderCore::new(index(80, 120), ReaderSettings {
        fast_mode: true, hardware_dither: true, landscape_spread: false, ..Default::default()
    }).unwrap();
    core.set_page(1).unwrap();
    let anchor = core.anchor(0);
    let cancelled = AtomicBool::new(true);
    let mut profile = ReaderPreparationProfile::default();
    let frame = core.render_source_identity_cancellable(&pixels, Viewport::new(64, 80),
        Some("pre-cancelled-fast-source"), &RenderCancellation::for_flag(&cancelled), Some(&mut profile)).unwrap();
    assert!(frame.is_none());
    assert_eq!(profile, ReaderPreparationProfile::default());
    assert_eq!(core.anchor(0), anchor);
    assert_eq!(pixels.as_raw(), &original);
    assert!(paint_quantized_canvas_cancellable(&pixels, Viewport::new(80, 120), 0, 0,
        false, true, true, &RenderCancellation::for_flag(&cancelled)).is_none());
}

#[test]
fn cancellation_after_binary_canvas_work_cannot_publish_partial_pixels() {
    let pixels = source(361, 517);
    let original = pixels.as_raw().clone();
    let cancelled = AtomicBool::new(false);
    let painted = AtomicUsize::new(0);
    let probe = |work, completed| {
        if matches!(work, RenderWork::CanvasPixels) && completed > 0 {
            painted.store(completed, Ordering::Release);
            cancelled.store(true, Ordering::Release);
        }
    };
    let cancellation = RenderCancellation::with_probe(&cancelled, &probe);
    let mut core = ReaderCore::new(index(361, 517), ReaderSettings {
        fast_mode: true, dither: false, hardware_dither: true,
        landscape_spread: false, ..Default::default()
    }).unwrap();
    let mut profile = ReaderPreparationProfile::default();
    let frame = core.render_source_identity_cancellable(&pixels, Viewport::new(361, 517),
        Some("interrupted-fast-source"), &cancellation, Some(&mut profile)).unwrap();
    assert!(cancelled.load(Ordering::Acquire));
    assert!(painted.load(Ordering::Acquire) > 0);
    assert!(painted.load(Ordering::Acquire) < original.len());
    assert!(frame.is_none());
    assert_eq!(profile.rendered_frames, 0);
    assert_eq!(profile.output_bytes, 0);
    let canvas = &profile.stages["canvas"];
    assert_eq!((canvas.attempts, canvas.completed, canvas.partial), (1, 0, 1));
    assert_eq!(pixels.as_raw(), &original);
}

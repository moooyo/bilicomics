//! Production viewport quantization, immutable sources, and complete-frame cancellation.

use super::*;
use crate::domain::PageDescriptor;
use image::Luma;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};

const METHODS: [SpatialDither; 3] = [SpatialDither::Off, SpatialDither::Bayer8, SpatialDither::FloydSteinberg];

fn index(width: u32, height: u32) -> ImageIndex {
    ImageIndex { episode_id: "spatial-render-episode".into(), revision: "spatial-render-revision".into(),
        images: (0..2).map(|position| PageDescriptor { id: format!("spatial-render-{position}"),
            index: position, width, height }).collect(), ..Default::default() }
}

fn source(width: u32, height: u32) -> GrayImage {
    GrayImage::from_fn(width, height, |x, y| Luma([((x * 37 + y * 19 + x * y * 3) % 256) as u8]))
}

fn fixture(name: &str, expected_sha: &str) -> (std::path::PathBuf, GrayImage) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/research/aidoku-spatial-dither-20261116").join(name);
    let encoded = std::fs::read(&path).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(&encoded)), expected_sha, "The frozen reference changed: {name}");
    (path, image::load_from_memory_with_format(&encoded, image::ImageFormat::Png).unwrap().into_luma8())
}

fn rgba_gray(frame: &RenderedPage) -> GrayImage {
    GrayImage::from_raw(frame.width, frame.height, frame.rgba.chunks_exact(4).map(|p| {
        assert_eq!(p, &[p[0], p[0], p[0], 255]);
        p[0]
    }).collect()).unwrap()
}

fn assert_pixels(frame: &RenderedPage, expected: &GrayImage) {
    assert_eq!((frame.width, frame.height), expected.dimensions());
    assert_eq!(rgba_gray(frame).as_raw(), expected.as_raw());
    let dark = expected.as_raw().iter().filter(|&&value| value <= 68).count() as f32
        / (expected.width() as f32 * expected.height() as f32);
    assert!((frame.dark_fraction - dark).abs() <= 1e-7);
}

#[test]
fn default_selection_is_cpu_bayer8_and_disabled_selection_is_nearest_level() {
    let defaults = ReaderSettings::default();
    assert_eq!(defaults.spatial_dither, Some(SpatialDither::Bayer8));
    for method in METHODS {
        let selected = ReaderSettings { spatial_dither: Some(method), hardware_dither: true, ..Default::default() };
        assert_eq!(selected.effective_spatial_dither(), Some(method));
        assert!(!selected.uses_hardware_dither(), "A stale hardware field cannot layer Y4 over CPU output");
        let disabled = ReaderSettings { dither: false, ..selected };
        assert_eq!(disabled.effective_spatial_dither(), Some(SpatialDither::Off));
        assert!(!disabled.uses_hardware_dither());
    }
    let raw = ReaderSettings { spatial_dither: None, hardware_dither: true, ..Default::default() };
    assert_eq!(raw.effective_spatial_dither(), None);
    assert!(raw.uses_hardware_dither(), "The explicit internal raw research path is still available");
    assert!(!ReaderSettings { fast_mode: true, ..raw }.uses_hardware_dither());
    let legacy: ReaderSettings = serde_json::from_value(serde_json::json!({"dither": false, "hardware_dither": true})).unwrap();
    assert_eq!(legacy.effective_spatial_dither(), Some(SpatialDither::Off));
    assert!(!legacy.uses_hardware_dither());
}

#[test]
fn full_resolution_reader_outputs_match_all_three_unchanged_handoff_fixtures() {
    let (path, original) = fixture("input-gray8.png", "21ded69522eb01df3bdbd69839a1ca35c95d1d6025ed51429869ed0e6ea416ff");
    let resources = ReaderResources::new(8 * 1024 * 1024);
    let canonical = load_validated_source_with_resources(&path, &resources).unwrap();
    assert_eq!(canonical.pixels.as_raw(), original.as_raw());
    let proof = canonical.proof().clone();
    let usage = resources.cache_usage();
    let viewport = Viewport::new(original.width(), original.height());
    let cancelled = AtomicBool::new(false);
    for (method, name, sha) in [
        (SpatialDither::Off, "expected-off.png", "54f99ac2d2be1b8fd19fdf1b2b4d812053c80ccc036c2d0032bd798788bf8305"),
        (SpatialDither::Bayer8, "expected-bayer8.png", "cd2fb81827fb8e25a322128e63e069a06c64289b079354075c185d245b6e322c"),
        (SpatialDither::FloydSteinberg, "expected-floyd-steinberg-q8.png", "aa2b01a3553ca595d05d2ba4bad43cbfcd456de81198edd9e992b6663db3ac84"),
    ] {
        let (_, expected) = fixture(name, sha);
        for path in [TestRasterPath::Current, TestRasterPath::Reference] {
            let mut core = ReaderCore::new(index(original.width(), original.height()), ReaderSettings {
                spatial_dither: Some(method), hardware_dither: true, landscape_spread: false, ..Default::default()
            }).unwrap();
            core.raster_path = path;
            let plain = core.render_validated_source(&canonical, viewport).unwrap();
            assert_pixels(&plain, &expected);
            let anchor = core.anchor(0);
            let interrupted = core.render_validated_source_cancellable(&canonical, viewport, &cancelled, None).unwrap().unwrap();
            assert_pixels(&interrupted, &expected);
            assert_eq!(core.anchor(0), anchor);
        }
    }
    assert_eq!(canonical.proof(), &proof);
    assert_eq!(canonical.pixels.as_raw(), original.as_raw());
    assert_eq!(resources.cache_usage(), usage);
}

#[test]
fn quantization_follows_geometry_rotation_crop_and_zoom_on_the_complete_viewport() {
    let pixels = source(37, 19);
    let viewport = Viewport::new(83, 101);
    for fit in [FitMode::Original, FitMode::Page, FitMode::Height, FitMode::Width] {
        for rotation in [0, 90, 180, 270] {
            for zoom_ratio in [1.0, 1.5] {
                // Only this geometry oracle opts into the pre-quantization research path.
                let mut initial = ReaderCore::new(index(37, 19), ReaderSettings {
                    spatial_dither: None, hardware_dither: true, fit, rotation, zoom_ratio,
                    landscape_spread: false, ..Default::default()
                }).unwrap();
                initial.set_page(1).unwrap();
                if zoom_ratio > 1.0 { initial.pan_to(0.25, 0.125).unwrap(); }
                let mut raw = initial.clone();
                let raw_frame = raw.render_source(&pixels, viewport).unwrap();
                let unquantized = rgba_gray(&raw_frame);
                for method in METHODS {
                    let mut expected = unquantized.clone();
                    spatial_dither::apply(&mut expected, method).unwrap();
                    let mut core = initial.clone();
                    let mut settings = core.settings().clone();
                    settings.spatial_dither = Some(method);
                    core.update_settings(settings).unwrap();
                    let frame = core.render_source(&pixels, viewport).unwrap();
                    assert_pixels(&frame, &expected);
                    assert_eq!((frame.source_width, frame.source_height, frame.crop, frame.crop_count),
                        (raw_frame.source_width, raw_frame.source_height, raw_frame.crop, raw_frame.crop_count));
                    assert_eq!(core.anchor(0), raw.anchor(0));
                }
            }
        }
    }
}

#[test]
fn bayer_phase_and_diffusion_row_parity_use_viewport_coordinates_including_white_margins() {
    let pixels = source(67, 41);
    let viewport = Viewport::new(78, 52);
    let left = (viewport.width - pixels.width()) / 2;
    let top = (viewport.height - pixels.height()) / 2;
    assert_eq!((left, top), (5, 5));
    for method in [SpatialDither::Bayer8, SpatialDither::FloydSteinberg] {
        let mut expected = GrayImage::from_pixel(viewport.width, viewport.height, Luma([255]));
        image::imageops::replace(&mut expected, &pixels, i64::from(left), i64::from(top));
        spatial_dither::apply(&mut expected, method).unwrap();
        let mut core = ReaderCore::new(index(67, 41), ReaderSettings {
            spatial_dither: Some(method), fit: FitMode::Original, landscape_spread: false, ..Default::default()
        }).unwrap();
        let frame = core.render_source(&pixels, viewport).unwrap();
        assert_pixels(&frame, &expected);
        let mut wrong_local = pixels.clone();
        spatial_dither::apply(&mut wrong_local, method).unwrap();
        let mut wrong_canvas = GrayImage::from_pixel(viewport.width, viewport.height, Luma([255]));
        image::imageops::replace(&mut wrong_canvas, &wrong_local, i64::from(left), i64::from(top));
        assert_ne!(expected.as_raw(), wrong_canvas.as_raw(), "The fixture must detect local {method:?} processing before placement");
    }
}

#[test]
fn disabled_dithering_overrides_every_saved_method_without_a_second_quantizer() {
    let pixels = source(129, 97);
    let viewport = Viewport::new(137, 107);
    let mut raw = ReaderCore::new(index(129, 97), ReaderSettings {
        spatial_dither: None, hardware_dither: true, fit: FitMode::Original, ..Default::default()
    }).unwrap();
    let mut expected = rgba_gray(&raw.render_source(&pixels, viewport).unwrap());
    spatial_dither::apply(&mut expected, SpatialDither::Off).unwrap();
    for method in METHODS {
        let mut core = ReaderCore::new(index(129, 97), ReaderSettings {
            spatial_dither: Some(method), dither: false, hardware_dither: true,
            fit: FitMode::Original, ..Default::default()
        }).unwrap();
        assert_pixels(&core.render_source(&pixels, viewport).unwrap(), &expected);
    }
}

#[test]
fn fast_binary_frames_bypass_every_grayscale_spatial_method() {
    let pixels = source(67, 41);
    let viewport = Viewport::new(78, 52);
    let mut raw_fast = ReaderCore::new(index(67, 41), ReaderSettings {
        spatial_dither: None, fast_mode: true, fit: FitMode::Original, ..Default::default()
    }).unwrap();
    let expected = raw_fast.render_source(&pixels, viewport).unwrap();
    assert_eq!(expected.rgba.chunks_exact(4).map(|pixel| pixel[0]).collect::<BTreeSet<_>>(), BTreeSet::from([0, 255]));
    for method in METHODS {
        for dither in [false, true] {
            let mut core = ReaderCore::new(index(67, 41), ReaderSettings {
                spatial_dither: Some(method), dither, hardware_dither: true,
                fast_mode: true, fit: FitMode::Original, ..Default::default()
            }).unwrap();
            let frame = core.render_source(&pixels, viewport).unwrap();
            assert_eq!(frame.rgba, expected.rgba);
            assert_eq!(frame.dark_fraction, expected.dark_fraction);
            assert_eq!(core.anchor(0), raw_fast.anchor(0));
        }
    }
}

#[test]
fn spatial_changes_share_gray8_source_and_preparation_but_not_final_settings_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("spatial-render-source.png");
    let original = source(83, 61);
    original.save(&path).unwrap();
    let resources = ReaderResources::new(1024 * 1024);
    let canonical = load_validated_source_with_resources(&path, &resources).unwrap();
    let proof = canonical.proof().clone();
    let bytes = canonical.pixels.as_raw().clone();
    let usage = resources.cache_usage();
    let processed = resources.processed_cache_usage();
    let viewport = Viewport::new(103, 101);
    let mut core = ReaderCore::new(index(83, 61), ReaderSettings { fit: FitMode::Original, ..Default::default() }).unwrap();
    let preparation = core.source_preparation(viewport);
    let mut seen = Vec::new();
    for method in METHODS {
        let mut settings = core.settings().clone();
        settings.spatial_dither = Some(method);
        core.update_settings(settings.clone()).unwrap();
        core.render_validated_source(&canonical, viewport).unwrap();
        assert_eq!(core.source_preparation(viewport), preparation);
        assert!(!seen.contains(&settings));
        seen.push(settings);
        let reused = load_validated_source_with_resources(&path, &resources).unwrap();
        assert!(Arc::ptr_eq(&canonical.pixels, &reused.pixels));
        assert_eq!(reused.proof(), &proof);
        assert_eq!(reused.pixels.as_raw(), &bytes);
        assert_eq!(resources.cache_usage(), usage);
        assert_eq!(resources.processed_cache_usage(), processed);
    }
}

#[test]
fn every_spatial_canvas_phase_cancels_without_returning_a_partial_frame() {
    let pixels = source(67, 41);
    let original = pixels.as_raw().clone();
    for method in METHODS {
        for target in [RenderWork::CanvasFill, RenderWork::CanvasGrayCopy, RenderWork::SpatialDither, RenderWork::CanvasPixels] {
            let cancelled = AtomicBool::new(false);
            let probe = |work, completed| {
                let threshold = if target == RenderWork::SpatialDither { 1 } else { 0 };
                if work == target && completed > threshold { cancelled.store(true, Ordering::Release); }
            };
            let cancellation = RenderCancellation::with_probe(&cancelled, &probe);
            let mut core = ReaderCore::new(index(67, 41), ReaderSettings {
                spatial_dither: Some(method), fit: FitMode::Original, ..Default::default()
            }).unwrap();
            let mut profile = ReaderPreparationProfile::default();
            let result = core.render_source_identity_cancellable(&pixels, Viewport::new(516, 513),
                Some("spatial-cancellation-fixture"), &cancellation, Some(&mut profile)).unwrap();
            assert!(cancelled.load(Ordering::Acquire), "The {target:?} cancellation boundary was not reached for {method:?}");
            assert!(result.is_none());
            assert_eq!(profile.rendered_frames, 0);
            assert_eq!(profile.output_bytes, 0);
            assert_eq!(profile.stages["canvas"].completed, 0);
        }
    }
    assert_eq!(pixels.as_raw(), &original);
}

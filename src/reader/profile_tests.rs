//! Exact-output checks and an explicitly selected, remote-only release probe.

use std::{fs, hint::black_box, path::PathBuf, time::Instant};

use image::{GrayImage, Luma, Rgba, RgbaImage, imageops::FilterType};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::*;
use crate::domain::PageDescriptor;

fn index(width: u32, height: u32) -> ImageIndex {
    ImageIndex {
        episode_id: "reader-profile-fixture".into(),
        revision: "reader-profile-revision".into(),
        images: vec![PageDescriptor { id: "reader-profile-page".into(), index: 0, width, height }],
        ..Default::default()
    }
}

fn assert_same_frame(actual: &RenderedPage, expected: &RenderedPage) {
    assert_eq!((actual.width, actual.height), (expected.width, expected.height));
    assert_eq!((actual.source_width, actual.source_height), (expected.source_width, expected.source_height));
    assert_eq!(actual.crop, expected.crop);
    assert_eq!((actual.page_index, actual.crop_index, actual.crop_count),
        (expected.page_index, expected.crop_index, expected.crop_count));
    assert_eq!(actual.overlap_from_previous, expected.overlap_from_previous);
    assert_eq!(actual.heuristic_panels, expected.heuristic_panels);
    assert_eq!(actual.dark_fraction, expected.dark_fraction);
    assert_eq!(actual.rgba, expected.rgba);
}

fn resize_reference(source: &GrayImage, crop: SourceRect, width: u32, height: u32) -> GrayImage {
    let view = image::imageops::crop_imm(source, crop.x, crop.y, crop.width, crop.height);
    image::imageops::resize(&*view, width, height, FilterType::Triangle)
}

fn resize_whole_source_candidate(source: &GrayImage, crop: SourceRect, width: u32, height: u32) -> GrayImage {
    if crop == SourceRect::whole(source.width(), source.height()) {
        image::imageops::resize(source, width, height, FilterType::Triangle)
    } else {
        resize_reference(source, crop, width, height)
    }
}

pub(super) fn canvas_reference(source: &GrayImage, viewport: Viewport, left: u32, top: u32, dither: bool) -> (Vec<u8>, u64) {
    let mut canvas = RgbaImage::from_pixel(viewport.width, viewport.height, Rgba([255, 255, 255, 255]));
    let mut dark = 0;
    for (x, y, pixel) in source.enumerate_pixels() {
        let tone = quantize(pixel[0], x, y, dither);
        if tone <= 68 { dark += 1; }
        canvas.put_pixel(left + x, top + y, Rgba([tone, tone, tone, 255]));
    }
    (canvas.into_raw(), dark)
}

fn canvas_rows_candidate(source: &GrayImage, viewport: Viewport, left: u32, top: u32,
    dither: bool, table: bool) -> (Vec<u8>, u64)
{
    if table { return paint_quantized_canvas(source, viewport, left, top, dither, false); }
    let mut canvas = vec![255; viewport.width as usize * viewport.height as usize * 4];
    let stride = viewport.width as usize * 4;
    let width = source.width() as usize;
    let mut dark = 0;
    for (y, row) in source.as_raw().chunks_exact(width).enumerate() {
        let start = (top as usize + y) * stride + left as usize * 4;
        let output = &mut canvas[start..start + width * 4];
        for (x, (&value, output)) in row.iter().zip(output.chunks_exact_mut(4)).enumerate() {
            let tone = quantize(value, x as u32, y as u32, dither);
            dark += u64::from(tone <= 68);
            output.copy_from_slice(&[tone, tone, tone, 255]);
        }
    }
    (canvas, dark)
}

#[test]
fn optional_timing_preserves_complete_frames_and_reading_anchors() {
    let source = GrayImage::from_fn(29, 71, |x, y| Luma([((x * 37 + y * 13) % 256) as u8]));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile-fixture.png");
    source.save(&path).unwrap();
    let resources = ReaderResources::new(1024 * 1024);
    let mut cold_profile = ReaderPreparationProfile::default();
    let validated = load_validated_source_with_profile(&path, &resources, Some(&mut cold_profile)).unwrap();
    assert_eq!((cold_profile.decoded_sources, cold_profile.reused_sources, cold_profile.unchanged_sources), (1, 0, 0));
    let mut warm_profile = ReaderPreparationProfile::default();
    let reused = load_validated_source_with_profile(&path, &resources, Some(&mut warm_profile)).unwrap();
    assert!(Arc::ptr_eq(&validated.pixels, &reused.pixels));
    assert_eq!((warm_profile.decoded_sources, warm_profile.reused_sources, warm_profile.unchanged_sources), (0, 1, 0));

    for rotation in [0, 90, 180, 270] {
        for dither in [false, true] {
            for zoom_ratio in [1.0, 8.0] {
                for (mode, fit, explicit) in [
                    (DisplayMode::Page, FitMode::Page, false),
                    (DisplayMode::Page, FitMode::Width, false),
                    (DisplayMode::Strip, FitMode::Width, false),
                    (DisplayMode::Panel, FitMode::Page, false),
                    (DisplayMode::Panel, FitMode::Page, true),
                ] {
                    let settings = ReaderSettings { mode, fit, rotation, zoom_ratio, dither,
                        landscape_spread: false, ..Default::default() };
                    let mut normal = ReaderCore::new(index(source.width(), source.height()), settings).unwrap();
                    if explicit {
                        normal.set_panel_map("reader-profile-page", vec![
                            NormalizedRect { x: 0.1, y: 0.15, width: 0.75, height: 0.3 },
                            NormalizedRect { x: 0.0, y: 0.5, width: 1.0, height: 0.5 },
                        ]).unwrap();
                    }
                    normal.pan_to(0.35, 0.42).unwrap();
                    normal.raster_path = TestRasterPath::Reference;
                    let mut profiled = normal.clone();
                    profiled.raster_path = TestRasterPath::SingleChannel;
                    let mut current = normal.clone();
                    current.raster_path = TestRasterPath::Current;
                    let viewport = Viewport::new(37, 51);
                    let mut profile = ReaderPreparationProfile::default();
                    for _ in 0..2 {
                        let expected = normal.render_validated_source(&validated, viewport).unwrap();
                        let actual = profiled.render_validated_source_profiled(&validated, viewport, &mut profile).unwrap();
                        assert_same_frame(&actual, &expected);
                        assert_eq!(normal.anchor(123), profiled.anchor(123));
                        assert_same_frame(&current.render_validated_source(&validated, viewport).unwrap(), &expected);
                        assert_eq!(normal.anchor(123), current.anchor(123));
                    }
                    assert_eq!(profile.rendered_frames, 2);
                    assert_eq!(profile.layout_rebuilds, 1);
                    assert_eq!(profile.output_bytes, 2 * 37 * 51 * 4);
                }
            }
        }
    }
}

#[test]
fn profiling_cannot_reuse_a_cached_source_after_current_bytes_are_corrupted() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile-integrity-fixture.png");
    GrayImage::from_fn(17, 23, |x, y| Luma([(x + y) as u8])).save(&path).unwrap();
    let original = fs::read(&path).unwrap();
    let resources = ReaderResources::new(1024 * 1024);
    let mut profile = ReaderPreparationProfile::default();
    load_validated_source_with_profile(&path, &resources, Some(&mut profile)).unwrap();
    let mut corrupt = original;
    corrupt[29] ^= 0x40;
    fs::write(&path, corrupt).unwrap();
    assert!(matches!(load_validated_source_with_profile(&path, &resources, Some(&mut profile)), Err(ReaderError::CorruptImage)));
    assert_eq!(profile.reused_sources, 0);
}

#[test]
fn hashed_source_profiling_preserves_unchanged_and_cancelled_outcomes_without_rehashing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile-hashed-fixture.png");
    GrayImage::from_fn(19, 31, |x, y| Luma([((x * 19 + y * 31 + 43) % 256) as u8])).save(&path).unwrap();
    let input = HashedImage::new(fs::read(&path).unwrap()).unwrap();
    let hashes = crate::image_integrity::identity_computation_count();
    let cancelled = AtomicBool::new(false);
    let (proof, source) = load_hashed_source(&input, None, &cancelled).unwrap().unwrap();
    assert!(source.is_some());
    let mut profile = ReaderPreparationProfile::default();
    let (unchanged_proof, unchanged) = load_hashed_source_profiled(&input, Some(&proof), &cancelled, &mut profile).unwrap().unwrap();
    assert_eq!(unchanged_proof, proof);
    assert!(unchanged.is_none());
    assert_eq!((profile.decoded_sources, profile.reused_sources, profile.unchanged_sources), (0, 0, 1));
    assert_eq!(crate::image_integrity::identity_computation_count(), hashes);

    cancelled.store(true, Ordering::Release);
    let mut profile = ReaderPreparationProfile::default();
    assert!(load_hashed_source_profiled(&input, Some(&proof), &cancelled, &mut profile).unwrap().is_none());
    assert_eq!(profile, ReaderPreparationProfile::default());
    assert_eq!(crate::image_integrity::identity_computation_count(), hashes);

    let malformed = HashedImage::new(vec![b'x'; 19]).unwrap();
    let hashes = crate::image_integrity::identity_computation_count();
    assert!(load_hashed_source_profiled(&malformed, None, &cancelled, &mut profile).unwrap().is_none());
    assert_eq!(profile, ReaderPreparationProfile::default());
    assert_eq!(crate::image_integrity::identity_computation_count(), hashes);
}

#[test]
fn candidate_resize_and_canvas_match_reference_pixels_at_all_bayer_phases() {
    for value in 0..=255u8 {
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(TONE_TABLES[(y * 4 + x) as usize][usize::from(value)], quantize(value, x, y, true));
                assert_eq!(TONE_TABLES[16][usize::from(value)], quantize(value, x, y, false));
            }
        }
    }
    for (width, height) in [(1, 17), (19, 1), (29, 37), (256, 17)] {
        let source = GrayImage::from_fn(width, height, |x, y| Luma([((x * 13 + y * 37) % 256) as u8]));
        let partial = SourceRect { x: width / 3, y: height / 3,
            width: (width - width / 3).max(1), height: (height - height / 3).max(1) };
        for crop in [SourceRect::whole(width, height), partial] {
            for (target_width, target_height) in [(1, 1), (17, 23), (width, height), (width * 2 + 1, height * 2 + 1)] {
                let reference = resize_reference(&source, crop, target_width, target_height);
                let candidate = resize_whole_source_candidate(&source, crop, target_width, target_height);
                assert_eq!(reference, candidate);
                for (extra_width, extra_height, left, top) in [(0, 0, 0, 0), (7, 9, 3, 4), (5, 0, 2, 0), (0, 5, 0, 0)] {
                    let viewport = Viewport::new(target_width + extra_width, target_height + extra_height);
                    for dither in [false, true] {
                        let expected = canvas_reference(&reference, viewport, left, top, dither);
                        for table in [false, true] {
                            assert_eq!(canvas_rows_candidate(&candidate, viewport, left, top, dither, table), expected);
                        }
                    }
                }
            }
        }
    }
}

fn next_random(state: &mut u64) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 16) as u32
}

fn assert_triangle_matches(source: &GrayImage, crop: SourceRect, width: u32, height: u32) {
    let expected = resize_reference(source, crop, width, height);
    let actual = triangle::resize(source, crop, width, height);
    assert_eq!(actual.dimensions(), expected.dimensions());
    assert_eq!(actual.color_space(), expected.color_space());
    assert_eq!(actual.as_raw(), expected.as_raw(),
        "Triangle pixels changed for source {:?}, crop {:?}, target {:?}", source.dimensions(), crop, (width, height));
}

#[test]
fn single_channel_triangle_matches_small_exhaustive_geometry_and_all_gray_values() {
    for width in 1..=8 {
        for height in 1..=8 {
            let source = GrayImage::from_fn(width, height, |x, y| Luma([((x * 37 + y * 61) % 256) as u8]));
            for target_width in 1..=8 {
                for target_height in 1..=8 {
                    assert_triangle_matches(&source, SourceRect::whole(width, height), target_width, target_height);
                }
            }
        }
    }
    for value in 0..=255 {
        let source = GrayImage::from_pixel(7, 11, Luma([value]));
        for (width, height) in [(1, 1), (3, 5), (7, 11), (13, 17), (1, 23), (19, 1)] {
            assert_triangle_matches(&source, SourceRect::whole(7, 11), width, height);
        }
    }
}

#[test]
fn single_channel_triangle_matches_random_crops_and_extreme_aspect_ratios() {
    let mut random = 0x7d91_a603_c875_eb42;
    for _ in 0..256 {
        let width = next_random(&mut random) % 191 + 1;
        let height = next_random(&mut random) % 239 + 1;
        let source = GrayImage::from_fn(width, height, |_, _| Luma([next_random(&mut random) as u8]));
        let x = next_random(&mut random) % width;
        let y = next_random(&mut random) % height;
        let crop = SourceRect { x, y, width: next_random(&mut random) % (width - x) + 1,
            height: next_random(&mut random) % (height - y) + 1 };
        for crop in [SourceRect::whole(width, height), crop] {
            assert_triangle_matches(&source, crop,
                next_random(&mut random) % 251 + 1, next_random(&mut random) % 277 + 1);
        }
    }
    for (width, height) in [(1, 4093), (4093, 1), (3, 4093), (4093, 3), (1023, 17), (17, 1023)] {
        let source = GrayImage::from_fn(width, height, |x, y| {
            if (x + y) % 7 == 0 { Luma([255]) } else { Luma([0]) }
        });
        let crop = SourceRect { x: width / 4, y: height / 4, width: width - width / 4, height: height - height / 4 };
        for crop in [SourceRect::whole(width, height), crop] {
            for (width, height) in [(1, 1), (7, 31), (131, 257), (511, 1), (1, 511)] {
                assert_triangle_matches(&source, crop, width, height);
            }
        }
    }
}

#[test]
fn single_channel_triangle_preserves_empty_and_same_size_reference_behavior() {
    for (width, height) in [(0, 0), (0, 7), (9, 0), (9, 7)] {
        let source = GrayImage::from_fn(width, height, |x, y| Luma([((x * 29 + y * 41) % 256) as u8]));
        for (target_width, target_height) in [(0, 0), (0, 13), (11, 0), (width, height), (11, 13)] {
            assert_triangle_matches(&source, SourceRect::whole(width, height), target_width, target_height);
        }
    }
}

#[test]
fn single_channel_triangle_preserves_nondefault_crop_color_metadata() {
    for color_space in [image::metadata::Cicp::DISPLAY_P3, image::metadata::Cicp::SRGB_LINEAR] {
        let mut source = GrayImage::from_fn(29, 41, |x, y| Luma([((x * 29 + y * 41) % 256) as u8]));
        source.set_color_space(color_space).unwrap();
        let crop = SourceRect { x: 3, y: 5, width: 17, height: 23 };
        for (width, height) in [(17, 23), (31, 47), (1, 1), (9, 11)] {
            assert_triangle_matches(&source, crop, width, height);
            assert_eq!(triangle::resize(&source, crop, width, height).color_space(), color_space);
        }
    }
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

/// This ignored probe is intentionally coupled to the release library under
/// test. It does not construct an alternate decoder or bypass integrity checks.
/// Source-cold means a fresh private grayscale cache, not a cold OS file cache.
#[test]
#[ignore = "Run only in the designated remote environment with explicit fictional fixture inputs"]
fn measure_reader_stage_candidates() {
    assert!(!cfg!(debug_assertions), "Use --release for a same-build stage comparison");
    let inputs = std::env::var_os("BILICOMICS_READER_PROFILE_INPUTS")
        .expect("Supply only explicitly selected fictional fixture paths");
    let paths: Vec<PathBuf> = std::env::split_paths(&inputs).collect();
    assert!(!paths.is_empty());
    let mut report = Vec::new();
    for (input_index, path) in paths.iter().enumerate() {
        for viewport in [Viewport::new(1860, 2480), Viewport::new(930, 1240)] {
            let resources = ReaderResources::new(resources::DEFAULT_SOURCE_CACHE_BYTES);
            let mut reader = ReaderCore::new(index(0, 0), ReaderSettings { landscape_spread: false, ..Default::default() }).unwrap();
            let mut samples = Vec::new();
            let mut reference_frame = None;
            let mut reference_anchor = None;
            let mut validated = None;
            for iteration in 0..7 {
                let started = Instant::now();
                let mut profile = ReaderPreparationProfile::default();
                let source = load_validated_source_with_profile(path, &resources, Some(&mut profile)).unwrap();
                let frame = reader.render_validated_source_profiled(&source, viewport, &mut profile).unwrap();
                let total_us = elapsed_us(started);
                if let Some(expected) = &reference_frame { assert_same_frame(&frame, expected); }
                let anchor = reader.anchor(123);
                if let Some(expected) = &reference_anchor { assert_eq!(&anchor, expected); }
                samples.push(json!({ "iteration": iteration, "total_us": total_us, "profile": profile }));
                reference_frame = Some(frame);
                reference_anchor = Some(anchor);
                validated = Some(source);
            }
            let source = validated.unwrap();
            let frame = reference_frame.unwrap();
            let crop = frame.crop;
            let ratio = (f64::from(viewport.width) / f64::from(crop.width))
                .min(f64::from(viewport.height) / f64::from(crop.height));
            let width = (f64::from(crop.width) * ratio).round().clamp(1.0, f64::from(viewport.width)) as u32;
            let height = (f64::from(crop.height) * ratio).round().clamp(1.0, f64::from(viewport.height)) as u32;
            let resized = resize_reference(&source.pixels, crop, width, height);
            let left = (viewport.width - width) / 2;
            let top = (viewport.height - height) / 2;
            let mut resize_samples = Vec::new();
            let mut canvas_samples = Vec::new();
            let mut raster_samples = Vec::new();
            let mut reference_reader = reader.clone();
            reference_reader.raster_path = TestRasterPath::Reference;
            let mut candidate_reader = reader.clone();
            candidate_reader.raster_path = TestRasterPath::SingleChannel;
            for iteration in 0..9 {
                // Alternate order within the same process and exact build. No
                // file hashing, assertions, or digesting is inside these spans.
                for variant in if iteration % 2 == 0 { [0, 1, 2] } else { [2, 1, 0] } {
                    let started = Instant::now();
                    let result = match variant {
                        0 => resize_reference(black_box(&source.pixels), crop, width, height),
                        1 => resize_whole_source_candidate(black_box(&source.pixels), crop, width, height),
                        _ => triangle::resize(black_box(&source.pixels), crop, width, height),
                    };
                    let duration = elapsed_us(started);
                    assert_eq!(black_box(&result), &resized);
                    let variant_name = ["original", "whole_source", "single_channel"][variant];
                    resize_samples.push(json!({ "iteration": iteration, "variant": variant_name, "duration_us": duration }));
                }
                for variant in if iteration % 2 == 0 { [0, 1, 2] } else { [2, 1, 0] } {
                    let started = Instant::now();
                    let result = match variant {
                        0 => canvas_reference(black_box(&resized), viewport, left, top, true),
                        1 => canvas_rows_candidate(black_box(&resized), viewport, left, top, true, false),
                        _ => canvas_rows_candidate(black_box(&resized), viewport, left, top, true, true),
                    };
                    let duration = elapsed_us(started);
                    assert_eq!(black_box(&result.0), &frame.rgba);
                    assert_eq!(result.1 as f32 / (u64::from(viewport.width) * u64::from(viewport.height)) as f32, frame.dark_fraction);
                    let variant_name = ["original", "row_scalar", "row_table"][variant];
                    canvas_samples.push(json!({ "iteration": iteration, "variant": variant_name, "duration_us": duration }));
                }
                for candidate in if iteration % 2 == 0 { [false, true] } else { [true, false] } {
                    let core = if candidate { &mut candidate_reader } else { &mut reference_reader };
                    let mut profile = ReaderPreparationProfile::default();
                    let started = Instant::now();
                    let result = core.render_validated_source_profiled(black_box(&source), viewport, &mut profile).unwrap();
                    let duration = elapsed_us(started);
                    assert_same_frame(&result, &frame);
                    assert_eq!(Some(core.anchor(123)), reference_anchor);
                    raster_samples.push(json!({ "iteration": iteration, "single_channel": candidate,
                        "duration_us": duration, "profile": profile }));
                }
            }
            report.push(json!({
                "input_index": input_index, "viewport": [viewport.width, viewport.height],
                "source": [source.pixels.width(), source.pixels.height()],
                "rgba_sha256": format!("{:x}", Sha256::digest(&frame.rgba)),
                "anchor": reference_anchor, "source_samples": samples,
                "resize_samples": resize_samples, "canvas_samples": canvas_samples,
                "raster_samples": raster_samples,
            }));
        }
    }
    println!("READER_STAGE_PROFILE={}", serde_json::to_string(&json!({
        "build": "release", "environment": "designated remote host; shared load; OS cache not flushed",
        "scope": "reader library only; excludes App admission, UI transfer, compositor, panel, and interaction latency",
        "cases": report,
    })).unwrap());
}

fn full_frame_oracle_proof(frame: &RenderedPage, anchor: &crate::domain::ReadingAnchor) -> serde_json::Value {
    json!({
        "rgba_sha256": format!("{:x}", Sha256::digest(&frame.rgba)),
        "anchor_sha256": format!("{:x}", Sha256::digest(serde_json::to_vec(anchor).unwrap())),
        "frame_size": [frame.width, frame.height],
        "source_size": [frame.source_width, frame.source_height],
        "crop": { "x": frame.crop.x, "y": frame.crop.y,
            "width": frame.crop.width, "height": frame.crop.height },
        "page_index": frame.page_index, "crop_index": frame.crop_index, "crop_count": frame.crop_count,
        "overlap_from_previous": frame.overlap_from_previous,
        "heuristic_panels": frame.heuristic_panels, "dark_fraction": frame.dark_fraction,
    })
}

/// Verify the complete production frame, not only a resized gray buffer. Inputs
/// are explicitly selected frozen fictional PNGs; neither paths nor pixels are
/// emitted. This release-only host proof makes no ARM or interaction claim.
#[test]
#[ignore = "Run only in the designated remote environment with two frozen fictional PNG inputs"]
fn verify_full_frame_cancellable_production_paths() {
    assert!(!cfg!(debug_assertions), "Use --release for the complete production frame oracle");
    let inputs = std::env::var_os("BILICOMICS_READER_PROFILE_INPUTS")
        .expect("Supply exactly two explicitly selected fictional PNG inputs");
    let paths: Vec<PathBuf> = std::env::split_paths(&inputs).collect();
    assert_eq!(paths.len(), 2, "The frozen full-frame oracle requires two fictional inputs");
    let mut report = Vec::new();
    let mut case_count = 0;
    let mut comparison_count = 0;

    for (input_index, path) in paths.iter().enumerate() {
        let encoded = fs::read(path).unwrap_or_else(|_| panic!("Could not read fictional input {input_index}"));
        let input_sha256 = format!("{:x}", Sha256::digest(&encoded));
        let resources = ReaderResources::new(resources::DEFAULT_SOURCE_CACHE_BYTES);
        let source = load_validated_source_with_resources(path, &resources)
            .unwrap_or_else(|_| panic!("Could not validate fictional input {input_index}"));
        assert_eq!(source.proof.sha256(), input_sha256);
        assert_eq!(source.proof.byte_len(), encoded.len() as u64);
        assert_eq!(source.proof.format, image::ImageFormat::Png);
        let mut cases = Vec::new();
        for (case_index, (name, viewport, mode, fit, rotation, zoom_ratio, dither, explicit_panel, pan)) in [
            ("page_fit_full", Viewport::new(1860, 2480), DisplayMode::Page, FitMode::Page,
                0, 1.0, true, false, None),
            ("page_fit_half", Viewport::new(930, 1240), DisplayMode::Page, FitMode::Page,
                0, 1.0, true, false, None),
            ("rotated_zoom_crop", Viewport::new(930, 1240), DisplayMode::Page, FitMode::Page,
                180, 1.75, false, false, Some([0.35, 0.42])),
            ("rotated_explicit_panel", Viewport::new(1860, 2480), DisplayMode::Panel, FitMode::Page,
                90, 1.0, true, true, Some([0.35, 0.42])),
        ].into_iter().enumerate() {
            let settings = ReaderSettings { mode, fit, rotation, zoom_ratio, dither,
                landscape_spread: false, ..Default::default() };
            let mut initial = ReaderCore::new(index(source.proof.width(), source.proof.height()), settings.clone()).unwrap();
            initial.raster_path = TestRasterPath::Current;
            if explicit_panel {
                initial.set_panel_map("reader-profile-page", vec![
                    NormalizedRect { x: 0.1, y: 0.15, width: 0.75, height: 0.3 },
                ]).unwrap();
            }
            if let Some([x, y]) = pan { initial.pan_to(x, y).unwrap(); }
            let mut reference = initial.clone();
            reference.raster_path = TestRasterPath::Reference;
            let expected = reference.render_validated_source(&source, viewport).unwrap();
            let expected_anchor = reference.anchor(123);
            let reference_proof = full_frame_oracle_proof(&expected, &expected_anchor);
            let mut variants = Vec::new();

            for variant in ["current_default", "cancellable_unprofiled", "cancellable_profiled"] {
                let mut reader = initial.clone();
                assert!(matches!(reader.raster_path, TestRasterPath::Current));
                let cancelled = AtomicBool::new(false);
                let mut profile = ReaderPreparationProfile::default();
                let actual = match variant {
                    "current_default" => reader.render_validated_source(&source, viewport).unwrap(),
                    "cancellable_unprofiled" => reader.render_validated_source_cancellable(&source, viewport,
                        &cancelled, None).unwrap().expect("An unset real cancellation flag must return a complete frame"),
                    "cancellable_profiled" => reader.render_validated_source_cancellable(&source, viewport,
                        &cancelled, Some(&mut profile)).unwrap()
                        .expect("An unset real cancellation flag must return a complete profiled frame"),
                    _ => unreachable!(),
                };
                assert!(!cancelled.load(Ordering::Acquire));
                // Avoid dumping a bitmap or full anchor on a failed assertion.
                // Once equality is known, reuse the complete metadata oracle.
                assert!(actual.rgba == expected.rgba,
                    "Complete pixels differ for input {input_index}, case {case_index}, variant {variant}");
                assert_same_frame(&actual, &expected);
                let actual_anchor = reader.anchor(123);
                assert!(actual_anchor == expected_anchor,
                    "Reading anchor differs for input {input_index}, case {case_index}, variant {variant}");
                let proof = full_frame_oracle_proof(&actual, &actual_anchor);
                assert_eq!(proof, reference_proof);
                let renderer_stage_proof = if variant == "cancellable_profiled" {
                    assert_eq!(profile.rendered_frames, 1);
                    assert_eq!(profile.layout_rebuilds, 1);
                    assert_eq!(profile.output_bytes, u64::from(viewport.width) * u64::from(viewport.height) * 4);
                    for stage in ["frame_validation", "rotation", "layout", "resize", "canvas"] {
                        let recorded = profile.stages.get(stage).expect("The renderer stage must be present");
                        assert_eq!((recorded.attempts, recorded.completed, recorded.partial), (1, 1, 0));
                    }
                    assert!(profile.stages.values().all(|stage| stage.partial == 0 && stage.completed == stage.attempts));
                    Some(profile.stages.iter().map(|(name, stage)| (*name, json!({
                        "attempts": stage.attempts, "completed": stage.completed, "partial": stage.partial,
                    }))).collect::<BTreeMap<_, _>>())
                } else {
                    assert_eq!(profile, ReaderPreparationProfile::default());
                    None
                };
                variants.push(json!({
                    "variant": variant,
                    "real_cancellation_flag_supplied": variant != "current_default",
                    "profiling_enabled": variant == "cancellable_profiled",
                    "complete_pixels_equal": true, "complete_frame_metadata_equal": true, "anchor_equal": true,
                    "proof": proof, "renderer_stage_proof": renderer_stage_proof,
                }));
                comparison_count += 1;
            }
            cases.push(json!({
                "case_index": case_index,
                "config": { "name": name, "viewport": [viewport.width, viewport.height], "settings": settings,
                    "pan": pan, "explicit_panel": explicit_panel.then_some([0.1, 0.15, 0.75, 0.3]) },
                "reference_proof": reference_proof, "variants": variants,
            }));
            case_count += 1;
        }
        report.push(json!({
            "input_index": input_index, "input_sha256": input_sha256,
            "source_proof": { "sha256": source.proof.sha256(), "byte_len": source.proof.byte_len(),
                "width": source.proof.width(), "height": source.proof.height(),
                "validation_version": source.proof.validation_version(), "format": "png" },
            "cases": cases,
        }));
    }
    assert_eq!((case_count, comparison_count), (8, 24));
    println!("READER_FULL_FRAME_ORACLE={}", serde_json::to_string(&json!({
        "schema_version": 1, "application_version": env!("CARGO_PKG_VERSION"), "build": "release",
        "scope": "Host release full-frame and anchor correctness proof only; not ARM execution, App/UI timing or physical panel latency",
        "reference": "Pinned image 0.25.10 Triangle crop resize and original canvas arithmetic",
        "input_count": report.len(), "case_count": case_count, "comparison_count": comparison_count,
        "all_passed": true, "inputs": report,
    })).unwrap());
}

//! Stage accounting must describe the same preparation work as the unprofiled API.

use std::{
    fs,
    io::{Cursor, Write},
    path::PathBuf,
    sync::{Arc, atomic::{AtomicBool, Ordering}},
};

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};

use super::*;
use crate::domain::PageDescriptor;

struct SourceFixture {
    _directory: tempfile::TempDir,
    path: PathBuf,
    encoded: Vec<u8>,
}

fn append_chunk(encoded: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
    encoded.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_be_bytes());
    encoded.extend_from_slice(kind);
    encoded.extend_from_slice(payload);
    let mut checksum = crc32fast::Hasher::new();
    checksum.update(kind);
    checksum.update(payload);
    encoded.extend_from_slice(&checksum.finalize().to_be_bytes());
}

fn write_fixture(label: &str, mut encoded: Vec<u8>) -> SourceFixture {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preparation-profile.png");
    // A private ancillary chunk prevents an unrelated parallel test from warming
    // this input's validation memo. No shared cache needs to be cleared.
    let end = encoded.split_off(encoded.len() - 12);
    let unique = format!("preparation-profile\0{label}:{}",
        directory.path().file_name().unwrap().to_string_lossy());
    append_chunk(&mut encoded, b"tEXt", unique.as_bytes());
    encoded.extend_from_slice(&end);
    fs::write(&path, &encoded).unwrap();
    SourceFixture { _directory: directory, path, encoded }
}

fn source_fixture(label: &str) -> SourceFixture {
    let source = RgbaImage::from_fn(29, 43, |x, y| Rgba([
        ((x * 17 + y * 31) % 256) as u8,
        ((x * 43 + y * 11) % 256) as u8,
        ((x * 7 + y * 53) % 256) as u8,
        ((x * 29 + y * 19) % 256) as u8,
    ]));
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(source).write_to(&mut encoded, ImageFormat::Png).unwrap();
    write_fixture(label, encoded.into_inner())
}

fn invalid_filter_fixture() -> SourceFixture {
    let mut encoded = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&1u32.to_be_bytes());
    header.extend_from_slice(&1u32.to_be_bytes());
    header.extend_from_slice(&[8, 0, 0, 0, 0]);
    append_chunk(&mut encoded, b"IHDR", &header);
    let mut compressed = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    // The complete stream has the expected two filtered bytes, but filter 5 is
    // invalid. Container validation succeeds before the pixel decoder rejects it.
    compressed.write_all(&[5, 93]).unwrap();
    append_chunk(&mut encoded, b"IDAT", &compressed.finish().unwrap());
    append_chunk(&mut encoded, b"IEND", &[]);
    write_fixture("invalid-pixel-filter", encoded)
}

fn index() -> ImageIndex {
    ImageIndex {
        episode_id: "preparation-profile-episode".into(),
        revision: "preparation-profile-revision".into(),
        images: vec![PageDescriptor {
            id: "preparation-profile-page".into(), index: 0, width: 29, height: 43,
        }],
        ..Default::default()
    }
}

fn assert_frame(actual: &RenderedPage, expected: &RenderedPage) {
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

fn assert_stage(profile: &ReaderPreparationProfile, name: &'static str,
    attempts: u32, completed: u32, partial: u32)
{
    let stage = profile.stages.get(name).unwrap_or_else(|| panic!("Stage {name} was not attempted"));
    assert_eq!((stage.attempts, stage.completed, stage.partial), (attempts, completed, partial),
        "Unexpected completion accounting for {name}");
}

fn assert_unattempted(profile: &ReaderPreparationProfile, names: &[&str]) {
    for name in names {
        assert!(!profile.stages.contains_key(name), "Unattempted stage {name} was reported");
    }
}

#[test]
fn stage_profiling_preserves_sealed_source_complete_pixels_and_reading_anchor() {
    let fixture = source_fixture("equivalent-output");
    let plain_resources = ReaderResources::new(1024 * 1024);
    let profiled_resources = ReaderResources::new(1024 * 1024);
    let plain = load_validated_source_with_resources(&fixture.path, &plain_resources).unwrap();
    let mut profile = ReaderPreparationProfile::default();
    let profiled = load_validated_source_with_profile(&fixture.path, &profiled_resources, Some(&mut profile)).unwrap();
    assert_eq!(profiled.proof, plain.proof);
    assert_eq!(profiled.pixels.dimensions(), plain.pixels.dimensions());
    assert_eq!(profiled.pixels.as_raw(), plain.pixels.as_raw());

    let settings = ReaderSettings { mode: DisplayMode::Strip, fit: FitMode::Width,
        rotation: 90, zoom_ratio: 1.5, dither: true, landscape_spread: false, ..Default::default() };
    let mut plain_core = ReaderCore::new(index(), settings.clone()).unwrap();
    let mut profiled_core = ReaderCore::new(index(), settings).unwrap();
    plain_core.pan_to(0.35, 0.42).unwrap();
    profiled_core.pan_to(0.35, 0.42).unwrap();
    let viewport = Viewport::new(31, 37);
    let expected = plain_core.render_validated_source(&plain, viewport).unwrap();
    let actual = profiled_core.render_validated_source_profiled(&profiled, viewport, &mut profile).unwrap();
    assert_frame(&actual, &expected);
    assert_eq!(profiled_core.anchor(123), plain_core.anchor(123));
    for name in ["source_read", "source_hash", "aggregate_integrity", "decode", "grayscale",
        "rotation", "layout", "resize", "canvas"] {
        assert_stage(&profile, name, 1, 1, 0);
    }
}

#[test]
fn profiling_hashes_a_path_once_and_never_rehashes_an_owned_input() {
    let fixture = source_fixture("owned-input-identity");
    let hashes = crate::image_integrity::identity_computation_count();
    let mut path_profile = ReaderPreparationProfile::default();
    let source = load_changed_source_profiled(&fixture.path, None, &mut path_profile).unwrap().unwrap();
    assert_eq!(crate::image_integrity::identity_computation_count(), hashes + 1);
    assert_stage(&path_profile, "source_read", 1, 1, 0);
    assert_stage(&path_profile, "source_hash", 1, 1, 0);

    let input = HashedImage::new(fixture.encoded).unwrap();
    let hashes = crate::image_integrity::identity_computation_count();
    let cancelled = AtomicBool::new(false);
    let mut owned_profile = ReaderPreparationProfile::default();
    let (proof, prepared) = load_hashed_source_profiled(&input, None, &cancelled, &mut owned_profile)
        .unwrap().unwrap();
    assert_eq!(proof, source.proof);
    assert_eq!(prepared.unwrap().pixels.as_raw(), source.pixels.as_raw());
    assert_eq!(crate::image_integrity::identity_computation_count(), hashes);
    assert_unattempted(&owned_profile, &["source_read", "source_hash"]);

    let mut unchanged_profile = ReaderPreparationProfile::default();
    let (unchanged_proof, unchanged) = load_hashed_source_profiled(&input, Some(&proof),
        &cancelled, &mut unchanged_profile).unwrap().unwrap();
    assert_eq!(unchanged_proof, proof);
    assert!(unchanged.is_none());
    assert_eq!((unchanged_profile.decoded_sources, unchanged_profile.reused_sources,
        unchanged_profile.unchanged_sources), (0, 0, 1));
    assert_eq!(crate::image_integrity::identity_computation_count(), hashes);
    assert_unattempted(&unchanged_profile, &["source_read", "source_hash", "decode"]);
}

#[test]
fn profiling_keeps_pixel_cache_hits_distinct_from_unchanged_sources() {
    let fixture = source_fixture("cache-outcomes");
    let resources = ReaderResources::new(1024 * 1024);
    let mut decoded_profile = ReaderPreparationProfile::default();
    let decoded = decode_source_bytes_profiled(&fixture.encoded, &resources, None, Some(&mut decoded_profile))
        .unwrap().unwrap();
    assert_eq!((decoded_profile.decoded_sources, decoded_profile.reused_sources,
        decoded_profile.unchanged_sources), (1, 0, 0));
    assert_stage(&decoded_profile, "decode", 1, 1, 0);

    let mut reused_profile = ReaderPreparationProfile::default();
    let reused = decode_source_bytes_profiled(&fixture.encoded, &resources, None, Some(&mut reused_profile))
        .unwrap().unwrap();
    assert_eq!(reused.proof, decoded.proof);
    assert!(Arc::ptr_eq(&reused.pixels, &decoded.pixels));
    assert_eq!((reused_profile.decoded_sources, reused_profile.reused_sources,
        reused_profile.unchanged_sources), (0, 1, 0));
    assert_unattempted(&reused_profile, &["source_read", "decode"]);

    let mut unchanged_profile = ReaderPreparationProfile::default();
    let unchanged = decode_source_bytes_profiled(&fixture.encoded, &resources,
        Some(&decoded.proof), Some(&mut unchanged_profile)).unwrap();
    assert!(unchanged.is_none());
    assert_eq!((unchanged_profile.decoded_sources, unchanged_profile.reused_sources,
        unchanged_profile.unchanged_sources), (0, 0, 1));
    assert_unattempted(&unchanged_profile, &["source_read", "decode"]);
}

#[test]
fn a_failed_source_read_is_partial_and_does_not_invent_later_stages() {
    let directory = tempfile::tempdir().unwrap();
    let absent = directory.path().join("absent-source.png");
    let mut profile = ReaderPreparationProfile::default();
    assert!(matches!(load_changed_source_profiled(&absent, None, &mut profile), Err(ReaderError::Io(_))));
    assert_stage(&profile, "source_read", 1, 0, 1);
    assert_unattempted(&profile, &["source_hash", "aggregate_integrity", "integrity_validation",
        "decode", "grayscale", "rotation", "layout", "resize", "canvas"]);
    assert_eq!((profile.decoded_sources, profile.reused_sources, profile.unchanged_sources), (0, 0, 0));
}

#[test]
fn a_failed_pixel_decode_reports_partial_work_after_complete_container_validation() {
    let fixture = invalid_filter_fixture();
    let mut profile = ReaderPreparationProfile::default();
    assert!(matches!(load_changed_source_profiled(&fixture.path, None, &mut profile), Err(ReaderError::CorruptImage)));
    assert_stage(&profile, "source_read", 1, 1, 0);
    assert_stage(&profile, "source_hash", 1, 1, 0);
    let validation = &profile.stages["integrity_validation"];
    assert!(validation.attempts >= 1);
    assert_eq!(validation.completed, validation.attempts);
    assert_eq!(validation.partial, 0);
    assert_stage(&profile, "decode", 1, 0, 1);
    assert_stage(&profile, "aggregate_integrity", 1, 0, 1);
    assert_unattempted(&profile, &["grayscale", "rotation", "layout", "resize", "canvas"]);
    assert_eq!((profile.decoded_sources, profile.reused_sources, profile.unchanged_sources), (0, 0, 0));
}

#[test]
fn a_pre_cancelled_owned_input_leaves_the_profile_unattempted() {
    let fixture = source_fixture("pre-cancelled-input");
    let cancelled = AtomicBool::new(true);
    for input in [HashedImage::new(fixture.encoded).unwrap(), HashedImage::new(vec![b'x'; 19]).unwrap()] {
        let hashes = crate::image_integrity::identity_computation_count();
        let mut profile = ReaderPreparationProfile::default();
        assert!(load_hashed_source_profiled(&input, None, &cancelled, &mut profile).unwrap().is_none());
        assert_eq!(profile, ReaderPreparationProfile::default());
        assert_eq!(crate::image_integrity::identity_computation_count(), hashes);
    }
    assert!(cancelled.load(Ordering::Acquire));
}

#[test]
fn a_failed_strip_layout_keeps_source_success_and_does_not_invent_raster_work() {
    let height = u32::try_from(MAX_LAYOUT_REGIONS + 1).unwrap();
    let source = image::GrayImage::from_pixel(1, height, image::Luma([0]));
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(source).write_to(&mut encoded, ImageFormat::Png).unwrap();
    let fixture = write_fixture("excessive-strip-layout", encoded.into_inner());
    let resources = ReaderResources::new(1024 * 1024);
    let mut profile = ReaderPreparationProfile::default();
    let source = load_validated_source_with_profile(&fixture.path, &resources, Some(&mut profile)).unwrap();
    let mut index = index();
    index.images[0].width = 1;
    index.images[0].height = height;
    let mut core = ReaderCore::new(index, ReaderSettings {
        mode: DisplayMode::Strip, fit: FitMode::Width, landscape_spread: false, ..Default::default()
    }).unwrap();
    // A one-pixel viewport requires one region per row; the source itself is
    // valid and small, but its strip exceeds the normal layout-region budget.
    assert!(matches!(core.render_validated_source_profiled(&source, Viewport::new(1, 1), &mut profile),
        Err(ReaderError::LayoutTooComplex)));
    for name in ["source_read", "source_hash", "aggregate_integrity", "decode", "grayscale"] {
        assert_stage(&profile, name, 1, 1, 0);
    }
    assert_stage(&profile, "layout", 1, 0, 1);
    assert_unattempted(&profile, &["resize", "canvas"]);
    assert_eq!(profile.rendered_frames, 0);
    assert_eq!(profile.output_bytes, 0);
}

#[test]
fn one_profile_accumulates_complete_frame_stages_without_repeating_source_work() {
    let fixture = source_fixture("accumulated-frames");
    let resources = ReaderResources::new(1024 * 1024);
    let mut profile = ReaderPreparationProfile::default();
    let source = load_validated_source_with_profile(&fixture.path, &resources, Some(&mut profile)).unwrap();
    let mut core = ReaderCore::new(index(), ReaderSettings {
        rotation: 90, landscape_spread: false, ..Default::default()
    }).unwrap();
    let viewport = Viewport::new(37, 51);
    let first = core.render_validated_source_profiled(&source, viewport, &mut profile).unwrap();
    let first_stages = profile.stages.clone();
    let first_anchor = core.anchor(123);
    let second = core.render_validated_source_profiled(&source, viewport, &mut profile).unwrap();
    assert_frame(&second, &first);
    assert_eq!(core.anchor(123), first_anchor);
    assert_eq!(profile.rendered_frames, 2);
    assert_eq!(profile.layout_rebuilds, 1);
    assert_eq!(profile.output_bytes, 2 * 37 * 51 * 4);
    for name in ["source_read", "source_hash", "aggregate_integrity", "decode", "grayscale"] {
        assert_stage(&profile, name, 1, 1, 0);
    }
    for name in ["rotation", "layout", "resize", "canvas"] {
        assert_stage(&profile, name, 2, 2, 0);
        let before = &first_stages[name];
        let after = &profile.stages[name];
        assert!(after.wall_us >= before.wall_us);
        // CPU clocks may be unavailable. A missing value is never replaced with
        // zero, and this test makes no duration or platform-availability claim.
        if let (Some(before), Some(after)) = (before.cpu_us, after.cpu_us) {
            assert!(after >= before);
        }
    }
}

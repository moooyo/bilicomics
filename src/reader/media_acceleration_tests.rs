//! Host-only geometry, proof, and cache tests using an in-memory fake scaler.

use super::*;
use crate::{image_integrity::{IntegrityFailureKind, decode_attempt_count}, native_media::Acceleration};
use image::{DynamicImage, ImageFormat, Luma};
use std::{cell::Cell, io::Cursor};

fn index(width: u32, height: u32) -> ImageIndex {
    ImageIndex { episode_id: "media-fixture-episode".into(), revision: "media-fixture-revision".into(),
        images: vec![crate::domain::PageDescriptor { id: "media-fixture-page".into(), index: 0,
            width, height }], ..Default::default() }
}

fn settings(fit: FitMode) -> ReaderSettings {
    ReaderSettings { fit, landscape_spread: false, hardware_dither: true,
        media_acceleration: Acceleration::DecodeAndResize, ..Default::default() }
}

fn request(fit: FitMode, viewport: Viewport) -> SourcePreparation {
    ReaderCore::new(index(512, 640), settings(fit)).unwrap().source_preparation(viewport)
}

fn jpeg(tone: u8) -> Vec<u8> {
    jpeg_with_dimensions(512, 640, tone)
}

fn jpeg_with_dimensions(width: u32, height: u32, tone: u8) -> Vec<u8> {
    let source = GrayImage::from_fn(width, height, |x, y| Luma([tone.wrapping_add(((x / 8 + y / 8) * 7) as u8)]));
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(source).write_to(&mut encoded, ImageFormat::Jpeg).unwrap();
    encoded.into_inner()
}

fn entropy_start(bytes: &[u8]) -> usize {
    let mut offset = 2;
    loop {
        assert_eq!(bytes[offset], 0xff);
        let marker = bytes[offset + 1];
        let length = usize::from(u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]));
        offset += 2 + length;
        if marker == 0xda { return offset; }
    }
}

fn append_segment(bytes: &mut Vec<u8>, marker: u8, body: &[u8]) {
    bytes.extend_from_slice(&[0xff, marker]);
    bytes.extend_from_slice(&u16::try_from(body.len() + 2).unwrap().to_be_bytes());
    bytes.extend_from_slice(body);
}

fn with_segment(bytes: &[u8], marker: u8, body: &[u8]) -> Vec<u8> {
    let mut result = bytes[..2].to_vec();
    append_segment(&mut result, marker, body);
    result.extend_from_slice(&bytes[2..]);
    result
}

fn with_segment_before_scan(bytes: &[u8], marker: u8, body: &[u8]) -> Vec<u8> {
    let mut offset = 2;
    while bytes[offset + 1] != 0xda {
        offset += 2 + usize::from(u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]));
    }
    let mut result = bytes[..offset].to_vec();
    append_segment(&mut result, marker, body);
    result.extend_from_slice(&bytes[offset..]);
    result
}

fn padded_zero_jpeg() -> Vec<u8> {
    let mut bytes = vec![0xff, 0xd8];
    let mut quantization = vec![0];
    quantization.extend_from_slice(&[1; 64]);
    append_segment(&mut bytes, 0xdb, &quantization);
    append_segment(&mut bytes, 0xc0, &[8, 2, 8, 2, 8, 1, 1, 0x11, 0]);
    for selector in [0x00, 0x10] {
        let mut table = vec![selector, 1];
        table.extend_from_slice(&[0; 15]);
        table.push(0);
        append_segment(&mut bytes, 0xc4, &table);
    }
    append_segment(&mut bytes, 0xda, &[1, 1, 0, 0, 63, 0]);
    // 65 * 65 blocks, each with a one-bit zero DC and one-bit EOB.
    bytes.extend_from_slice(&[0; 1056]);
    bytes.extend_from_slice(&[0x3f, 0xff, 0xd9]);
    bytes
}

fn assert_no_native_proof_or_decode(bytes: Vec<u8>, expected: IntegrityFailureKind) {
    let input = HashedImage::new(bytes).unwrap();
    assert!(crate::image_integrity::validate_native_jpeg(&input).is_none());
    let calls = Cell::new(0);
    assert!(try_prepare_accelerated(&input, request(FitMode::Page, Viewport::new(256, 320)),
        &ReaderResources::new(1024 * 1024), &AtomicBool::new(false), None, true, &|_, _| {
            calls.set(calls.get() + 1); Some(GrayImage::new(256, 320))
        }).is_none());
    assert_eq!(calls.get(), 0);
    let error = match load_hashed_source(&input, None, &AtomicBool::new(false)) {
        Err(error) => error,
        Ok(_) => panic!("The software validator must retain its existing rejection"),
    };
    assert_eq!(error.kind, expected);
    if expected == IntegrityFailureKind::Corrupt {
        let evidence = error.encoding_evidence().expect("Corruption must retain exact-byte evidence");
        assert_eq!(evidence.sha256(), input.sha256());
        assert_eq!(evidence.byte_len(), input.byte_len());
    } else { assert!(error.encoding_evidence().is_none()); }
}

#[test]
fn native_plans_reuse_the_exact_four_fit_geometries() {
    for (width, height) in [(512, 640), (800, 400), (256, 2000), (100, 50)] {
        for fit in [FitMode::Original, FitMode::Page, FitMode::Height, FitMode::Width] {
            let viewport = Viewport::new(256, 320);
            let expected = fit_geometry(SourceRect::whole(width, height), viewport, fit, 1.0, None);
            let actual = request(fit, viewport).geometry(width, height);
            let whole_downscale = expected.crop == SourceRect::whole(width, height)
                && expected.sample.is_none() && expected.width <= width && expected.height <= height
                && (expected.width, expected.height) != (width, height)
                && expected.width >= 16 && expected.height >= 16;
            assert_eq!(actual.is_some(), whole_downscale, "Unexpected {fit:?} admission for {width}x{height}");
            if let Some(actual) = actual { assert_eq!(actual, expected); }
        }
    }
    let page = request(FitMode::Page, Viewport::new(256, 200)).geometry(512, 640).unwrap();
    assert_eq!((page.width, page.height), (256, 200));
    assert_eq!(page.crop, SourceRect::whole(512, 640));
    assert_ne!(u64::from(page.width) * 640, u64::from(page.height) * 512,
        "The page fit must retain its approved independent-axis scaling");
    assert!(request(FitMode::Original, Viewport::new(512, 640)).geometry(512, 640).is_none());
}

#[test]
fn crops_zoom_rotations_modes_and_upscales_never_create_native_plans() {
    let original = request(FitMode::Page, Viewport::new(256, 320));
    for mode in [DisplayMode::Panel, DisplayMode::Strip] {
        assert!(SourcePreparation { mode, ..original }.geometry(512, 640).is_none());
    }
    for rotation in [90, 180, 270] {
        assert!(SourcePreparation { rotation, ..original }.geometry(512, 640).is_none());
    }
    for zoom_ratio in [1.01, 1.5, 2.0] {
        assert!(SourcePreparation { zoom_ratio, ..original }.geometry(512, 640).is_none());
    }
    assert!(SourcePreparation { media_acceleration: Acceleration::Off, ..original }.geometry(512, 640).is_none());
    assert!(request(FitMode::Height, Viewport::new(256, 320)).geometry(800, 400).is_none());
    assert!(request(FitMode::Width, Viewport::new(256, 320)).geometry(256, 2000).is_none());
    assert!(request(FitMode::Page, Viewport::new(1024, 320)).geometry(512, 640).is_none());
}

#[test]
fn entropy_only_proofs_keep_source_dimensions_and_reject_missing_blocks() {
    let encoded = jpeg(17);
    let input = HashedImage::new(encoded.clone()).unwrap();
    let decodes = decode_attempt_count();
    let proof = crate::image_integrity::validate_native_jpeg(&input).unwrap();
    assert_eq!((proof.width(), proof.height()), (512, 640));
    assert_eq!(proof.sha256(), input.sha256());
    assert_eq!(proof.byte_len(), input.byte_len());
    assert_eq!(proof.validation_version(), crate::image_integrity::VALIDATION_VERSION);
    assert_eq!(decode_attempt_count(), decodes, "Entropy validation must not run CPU IDCT");
    let start = entropy_start(&encoded);
    let mut broken = encoded[..start + (encoded.len() - start - 2) / 2].to_vec();
    broken.extend_from_slice(&[0xff, 0xd9]);
    let broken = HashedImage::new(broken).unwrap();
    assert!(crate::image_integrity::validate_native_jpeg(&broken).is_none());
    let calls = Cell::new(0);
    let result = try_prepare_accelerated(&broken, request(FitMode::Page, Viewport::new(256, 320)),
        &ReaderResources::new(1024 * 1024), &AtomicBool::new(false), None, true, &|_, _| {
            calls.set(calls.get() + 1); Some(GrayImage::new(256, 320))
        });
    assert!(result.is_none());
    assert_eq!(calls.get(), 0, "A malformed image must be rejected before the decoder boundary");
}

#[test]
fn native_proofs_retain_strict_terminal_header_and_segment_budget_policy() {
    let encoded = jpeg(17);
    for tail in [&[0_u8][..], &[0, 0, 0, 0][..], &[17][..]] {
        let mut bytes = encoded.clone();
        bytes.extend_from_slice(tail);
        assert_no_native_proof_or_decode(bytes, IntegrityFailureKind::Corrupt);
    }
    assert_no_native_proof_or_decode(with_segment(&encoded, 0xee,
        &[b'A', b'd', b'o', b'b', b'e', 0, 100, 0, 0, 0, 0, 255]), IntegrityFailureKind::Unsupported);
    for transform in [0, 1] {
        assert_no_native_proof_or_decode(with_segment_before_scan(&encoded, 0xee,
            &[b'A', b'd', b'o', b'b', b'e', 0, 100, 0, 0, 0, 0, transform]), IntegrityFailureKind::Unsupported);
    }
    let mut many_segments = encoded[..2].to_vec();
    for _ in 0..65_536 { append_segment(&mut many_segments, 0xfe, &[]); }
    many_segments.extend_from_slice(&encoded[2..]);
    assert_no_native_proof_or_decode(many_segments, IntegrityFailureKind::ResourceLimit);
    for marker in [0xc8, 0xdc, 0xde, 0xdf, 0xf0] {
        assert_no_native_proof_or_decode(with_segment(&encoded, marker, &[0, 0]), IntegrityFailureKind::Unsupported);
    }
}

#[test]
fn native_proofs_require_the_same_one_bit_entropy_alignment_as_software() {
    let mut bytes = padded_zero_jpeg();
    let valid = HashedImage::new(bytes.clone()).unwrap();
    assert!(crate::image_integrity::validate_native_jpeg(&valid).is_some());
    assert!(load_hashed_source(&valid, None, &AtomicBool::new(false)).is_ok());
    let last_entropy = bytes.len() - 3;
    bytes[last_entropy] &= !1;
    assert_no_native_proof_or_decode(bytes, IntegrityFailureKind::Corrupt);
}

#[test]
fn processed_pixels_are_separate_bounded_and_keyed_by_policy_geometry_and_content() {
    let input = HashedImage::new(jpeg_with_dimensions(1920, 2560, 34)).unwrap();
    let resources = ReaderResources::new(16 * 1024 * 1024);
    let plan = request(FitMode::Page, Viewport::new(1860, 2480));
    let calls = Cell::new(0);
    let fake = |_: &[u8], (width, height)| {
        calls.set(calls.get() + 1);
        Some(GrayImage::from_pixel(width, height, Luma([119])))
    };
    let mut profile = ReaderPreparationProfile::default();
    let first = try_prepare_accelerated(&input, plan, &resources, &AtomicBool::new(false),
        Some(&mut profile), true, &fake).unwrap();
    let repeated = try_prepare_accelerated(&input, plan, &resources, &AtomicBool::new(false),
        Some(&mut profile), true, &fake).unwrap();
    assert!(Arc::ptr_eq(&first.pixels, &repeated.pixels));
    assert_eq!((calls.get(), profile.native_sources, profile.processed_source_hits), (1, 1, 1));
    assert_eq!(resources.cache_usage().entries, 0);
    assert!(resources.lookup_digest(input.sha256()).is_none());
    assert_eq!(resources.processed_cache_usage().retained_bytes, 1860 * 2480);
    let mut key = first.key.clone();
    key.preparation.media_acceleration = Acceleration::Off;
    assert!(resources.processed_source(&key).is_none());
    assert!(!resources.insert_processed_source(key, first.pixels.clone()));
    let different_fit = request(FitMode::Original, Viewport::new(1860, 2480));
    assert!(try_prepare_accelerated(&input, different_fit, &resources, &AtomicBool::new(false), None, true, &fake).is_some());
    let different_size = request(FitMode::Page, Viewport::new(930, 1240));
    assert!(try_prepare_accelerated(&input, different_size, &resources, &AtomicBool::new(false), None, true, &fake).is_none());
    let mut unqualified = first.key.clone();
    unqualified.preparation = different_size;
    unqualified.geometry = different_size.geometry(1920, 2560).unwrap();
    assert!(!resources.insert_processed_source(unqualified, Arc::new(GrayImage::new(930, 1240))));
    let mut old_version = first.key.clone();
    old_version.version -= 1;
    assert!(!resources.insert_processed_source(old_version, first.pixels.clone()));
    let other = HashedImage::new(jpeg_with_dimensions(1920, 2560, 204)).unwrap();
    assert!(try_prepare_accelerated(&other, plan, &resources, &AtomicBool::new(false), None, true, &fake).is_some());
    assert_eq!(calls.get(), 3);
    let small = ReaderResources::new(1860 * 2480 - 1);
    assert!(try_prepare_accelerated(&input, plan, &small, &AtomicBool::new(false), None, true, &fake).is_some());
    assert_eq!(small.processed_cache_usage().retained_bytes, 0);
    assert_eq!(small.cache_usage().retained_bytes, 0);
    assert_eq!(resources.remove_digest(input.sha256()), 2);
    assert_eq!(resources.processed_cache_usage().entries, 1);
}

#[test]
fn native_frames_share_software_layout_canvas_y4_and_original_source_anchors() {
    let input = HashedImage::new(jpeg_with_dimensions(1920, 2560, 17)).unwrap();
    let cancelled = AtomicBool::new(false);
    let (_, canonical) = load_hashed_source(&input, None, &cancelled).unwrap().unwrap();
    let canonical = canonical.unwrap();
    let viewport = Viewport::new(1860, 2480);
    let resized = triangle::resize(&canonical.pixels, SourceRect::whole(1920, 2560), 1860, 2480);
    for hardware_dither in [false, true] {
        let mut software = ReaderCore::new(index(1920, 2560), ReaderSettings { hardware_dither, ..settings(FitMode::Page) }).unwrap();
        let expected = software.render_validated_source_cancellable(&canonical, viewport, &cancelled, None).unwrap().unwrap();
        for fit in [FitMode::Original, FitMode::Page, FitMode::Height, FitMode::Width] {
            let settings = ReaderSettings { hardware_dither, ..settings(fit) };
            let mut native = ReaderCore::new(index(1920, 2560), settings).unwrap();
            let scaled = try_prepare_accelerated(&input, native.source_preparation(viewport),
                &ReaderResources::new(16 * 1024 * 1024), &cancelled, None, true,
                &|_, target| { assert_eq!(target, (1860, 2480)); Some(resized.clone()) }).unwrap();
            let actual = native.render_prepared_source_cancellable(&PreparedGraySource::Scaled(scaled),
                viewport, &cancelled, None).unwrap().unwrap();
            assert_eq!(actual.rgba, expected.rgba);
            assert_eq!(actual.crop, expected.crop);
            assert_eq!((actual.source_width, actual.source_height), (1920, 2560));
            assert_eq!(native.anchor(0), software.anchor(0));
            assert_eq!(native.layouts.get(&0).unwrap().width, 1920);
            assert_eq!(native.layouts.get(&0).unwrap().height, 2560);
        }
    }
}

#[test]
fn unsupported_formats_and_cancellation_make_no_native_decode_calls() {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(GrayImage::new(512, 640)).write_to(&mut bytes, ImageFormat::Png).unwrap();
    let png = HashedImage::new(bytes.into_inner()).unwrap();
    let jpeg = HashedImage::new(jpeg(68)).unwrap();
    let resources = ReaderResources::new(1024 * 1024);
    let plan = request(FitMode::Page, Viewport::new(256, 320));
    let calls = Cell::new(0);
    let fake = |_: &[u8], _| { calls.set(calls.get() + 1); Some(GrayImage::new(256, 320)) };
    assert!(try_prepare_accelerated(&png, plan, &resources, &AtomicBool::new(false), None, true, &fake).is_none());
    assert!(try_prepare_accelerated(&jpeg, plan, &resources, &AtomicBool::new(true), None, true, &fake).is_none());
    assert!(try_prepare_accelerated(&jpeg, plan, &resources, &AtomicBool::new(false), None, false, &fake).is_none());
    assert_eq!(calls.get(), 0);
    assert_eq!(resources.processed_cache_usage().entries, 0);
}

#[test]
fn native_output_bounds_and_cancelled_results_never_enter_a_processed_cache() {
    let input = HashedImage::new(jpeg_with_dimensions(1920, 2560, 102)).unwrap();
    let resources = ReaderResources::new(16 * 1024 * 1024);
    let plan = request(FitMode::Page, Viewport::new(1860, 2480));
    let cancelled = AtomicBool::new(false);
    assert!(try_prepare_accelerated(&input, plan, &resources, &cancelled, None, true, &|_, _|
        Some(GrayImage::from_raw(1860, 2480, vec![0; 1860 * 2481])
            .expect("ImageBuffer permits oversized backing storage"))).is_none());
    assert!(try_prepare_accelerated(&input, plan, &resources, &cancelled, None, true, &|_, _| {
        cancelled.store(true, Ordering::Release);
        Some(GrayImage::new(1860, 2480))
    }).is_none());
    assert_eq!(resources.processed_cache_usage().entries, 0);
    assert_eq!(resources.cache_usage().entries, 0);
}

#[test]
fn unmeasured_gray_tuples_keep_software_proofs_and_never_reach_the_decoder() {
    let resources = ReaderResources::new(8 * 1024 * 1024);
    let cancelled = AtomicBool::new(false);
    let calls = Cell::new(0);
    let fake = |_: &[u8], (width, height)| {
        calls.set(calls.get() + 1);
        Some(GrayImage::new(width, height))
    };
    let tall_bytes = jpeg_with_dimensions(1280, 2560, 85);
    let tall = HashedImage::new(tall_bytes.clone()).unwrap();
    assert!(crate::image_integrity::validate_native_jpeg(&tall).is_some(),
        "A complete Gray encoding is not a hardware geometry qualification");
    assert!(try_prepare_accelerated(&tall, request(FitMode::Height, Viewport::new(1860, 2480)),
        &resources, &cancelled, None, true, &fake).is_none());
    let measured_source = HashedImage::new(jpeg_with_dimensions(1920, 2560, 85)).unwrap();
    assert!(try_prepare_accelerated(&measured_source, request(FitMode::Page, Viewport::new(930, 1240)),
        &resources, &cancelled, None, true, &fake).is_none());
    assert_eq!(calls.get(), 0);
    assert_eq!(resources.processed_cache_usage().entries, 0);
    let canonical = decode_source_bytes(&tall_bytes, &resources, None).unwrap().unwrap();
    assert_eq!((canonical.proof().width(), canonical.proof().height()), (1280, 2560));
    let (proof, retained) = resources.lookup_digest(tall.sha256()).unwrap();
    assert_eq!(proof, *canonical.proof());
    assert!(Arc::ptr_eq(&retained, &canonical.pixels));
}

#[test]
fn color_jpeg_keeps_software_proof_grayscale_and_canonical_cache_without_native_submission() {
    let image = image::RgbImage::from_fn(512, 640, |x, y|
        image::Rgb([((x * 7) % 256) as u8, ((y * 11) % 256) as u8, ((x + y * 3) % 256) as u8]));
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image).write_to(&mut encoded, ImageFormat::Jpeg).unwrap();
    let encoded = encoded.into_inner();
    let input = HashedImage::new(encoded.clone()).unwrap();
    let resources = ReaderResources::new(1024 * 1024);
    let calls = Cell::new(0);
    assert!(crate::image_integrity::validate_native_jpeg(&input).is_none());
    assert!(try_prepare_accelerated(&input, request(FitMode::Page, Viewport::new(256, 320)),
        &resources, &AtomicBool::new(false), None, true, &|_, _| {
            calls.set(calls.get() + 1);
            Some(GrayImage::new(256, 320))
        }).is_none());
    assert_eq!(calls.get(), 0, "Color must be rejected before the hardware decoder boundary");
    assert_eq!(resources.processed_cache_usage().entries, 0);
    let canonical = decode_source_bytes(&encoded, &resources, None).unwrap().unwrap();
    assert_eq!((canonical.proof().width(), canonical.proof().height()), (512, 640));
    assert_eq!(canonical.proof().sha256(), input.sha256());
    let (proof, decoded) = crate::image_integrity::decode_image(&encoded, None).unwrap();
    let expected = decoded.to_rgb8().pixels().map(|pixel| {
        ((54 * u32::from(pixel[0]) + 183 * u32::from(pixel[1]) + 19 * u32::from(pixel[2]) + 128) / 256) as u8
    }).collect::<Vec<_>>();
    assert_eq!(canonical.pixels.as_raw(), &expected);
    let (retained_proof, retained) = resources.lookup_digest(input.sha256()).unwrap();
    assert_eq!(retained_proof, proof);
    assert!(Arc::ptr_eq(&retained, &canonical.pixels));
}

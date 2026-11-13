use std::{fs, io::Cursor, sync::Arc};

use image::{DynamicImage, GrayImage, ImageFormat, Luma};

use super::{
    DisplayMode, FitMode, NormalizedRect, ReaderCore, ReaderError, ReaderMove,
    ReaderSettings, RenderedPage, Viewport, decode_bounded_bytes_with_resources,
    load_gray_source_with_resources, load_validated_source_with_resources, resources::ReaderResources,
};
use crate::{
    domain::{ImageIndex, PageDescriptor},
    image_integrity::{self, IntegrityFailureKind, ReusableImage},
};

fn encoded_source(width: u32, height: u32, tone: u8) -> Vec<u8> {
    let source = GrayImage::from_pixel(width, height, Luma([tone]));
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(source).write_to(&mut encoded, ImageFormat::Png)
        .expect("Encode a complete synthetic reader source");
    encoded.into_inner()
}

fn index(width: u32, height: u32) -> ImageIndex {
    ImageIndex { episode_id: "source-reuse-episode".into(), revision: "source-reuse-revision".into(),
        images: (0..2).map(|index| PageDescriptor { id: format!("source-reuse-page-{index}"),
            index, width, height }).collect(), ..Default::default() }
}

fn reader(index: &ImageIndex, settings: &ReaderSettings) -> ReaderCore {
    let mut reader = ReaderCore::new(index.clone(), settings.clone()).unwrap();
    if settings.mode == DisplayMode::Panel {
        reader.set_panel_map("source-reuse-page-1", vec![
            NormalizedRect { x: 0.0, y: 0.0, width: 1.0, height: 0.25 },
            NormalizedRect { x: 0.0, y: 0.25, width: 1.0, height: 0.5 },
            NormalizedRect { x: 0.0, y: 0.75, width: 1.0, height: 0.25 },
        ]).unwrap();
    }
    reader.set_page(1).unwrap();
    reader
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

#[test]
fn repeated_paths_reuse_current_bytes_and_same_geometry_replacements_render_new_pixels() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("replaceable-source.png");
    let original = encoded_source(8, 12, 17);
    fs::write(&path, &original).unwrap();
    let resources = ReaderResources::new(1024 * 1024);
    let first = load_gray_source_with_resources(&path, &resources).unwrap();
    let repeated = load_gray_source_with_resources(&path, &resources).unwrap();
    assert!(Arc::ptr_eq(&first, &repeated));
    let proof = image_integrity::validate_image(&original, None).unwrap();
    let (cached_proof, cached) = resources.lookup_digest(proof.sha256()).unwrap();
    assert_eq!(cached_proof, proof);
    assert!(Arc::ptr_eq(&cached, &first));
    assert_eq!(resources.cache_usage().retained_bytes, first.as_raw().capacity());

    let settings = ReaderSettings { dither: false, landscape_spread: false, ..Default::default() };
    let mut core = reader(&index(8, 12), &settings);
    let before = core.render_source(&first, Viewport::new(8, 12)).unwrap();
    fs::write(&path, encoded_source(8, 12, 204)).unwrap();
    let replacement = load_gray_source_with_resources(&path, &resources).unwrap();
    assert!(!Arc::ptr_eq(&first, &replacement));
    let after = core.render_source(&replacement, Viewport::new(8, 12)).unwrap();
    assert_eq!(first.dimensions(), replacement.dimensions());
    assert_eq!(before.rgba[0], 17);
    assert_eq!(after.rgba[0], 204);
    assert_ne!(before.rgba, after.rgba);
    assert_eq!(resources.cache_usage().retained_bytes,
        first.as_raw().capacity() + replacement.as_raw().capacity());
    assert!(resources.cache_usage().retained_bytes <= resources.cache_usage().byte_budget);
}

#[test]
fn cached_paths_still_reject_truncated_and_corrupt_current_encodings() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("damaged-source.png");
    let original = encoded_source(8, 12, 17);
    fs::write(&path, &original).unwrap();
    let resources = ReaderResources::new(1024 * 1024);
    let cached = load_gray_source_with_resources(&path, &resources).unwrap();
    let truncated = original[..original.len() / 2].to_vec();
    let mut corrupt = original.clone();
    // The first IHDR checksum byte changes without changing source dimensions.
    corrupt[29] ^= 0x40;
    for damaged in [truncated, corrupt] {
        fs::write(&path, damaged).unwrap();
        assert!(matches!(load_gray_source_with_resources(&path, &resources), Err(ReaderError::CorruptImage)));
        assert_eq!(cached.get_pixel(0, 0)[0], 17);
    }
    fs::write(&path, original).unwrap();
    let restored = load_gray_source_with_resources(&path, &resources).unwrap();
    assert!(Arc::ptr_eq(&cached, &restored));
}

#[test]
fn a_source_larger_than_the_cache_budget_is_decoded_without_retaining_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("uncached-source.png");
    fs::write(&path, encoded_source(24, 24, 68)).unwrap();
    let resources = ReaderResources::new(64);
    let first = load_gray_source_with_resources(&path, &resources).unwrap();
    let second = load_gray_source_with_resources(&path, &resources).unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(first.as_raw(), second.as_raw());
    assert_eq!(resources.cache_usage().retained_bytes, 0);
    assert_eq!(resources.cache_usage().entries, 0);
}

#[test]
fn reusable_sources_preserve_mode_regions_and_centered_page_fit_anchors() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tall-source.png");
    let encoded = encoded_source(60, 400, 17);
    fs::write(&path, &encoded).unwrap();
    let resources = ReaderResources::new(1024 * 1024);
    let source = decode_bounded_bytes_with_resources(&encoded, &resources).unwrap();
    let descriptor = index(60, 400);
    let viewport = Viewport::new(60, 100);
    for (mode, fit) in [(DisplayMode::Strip, FitMode::Page),
        (DisplayMode::Panel, FitMode::Page), (DisplayMode::Page, FitMode::Original),
        (DisplayMode::Page, FitMode::Page), (DisplayMode::Page, FitMode::Height),
        (DisplayMode::Page, FitMode::Width)] {
        let settings = ReaderSettings { mode, fit, dither: false, landscape_spread: false, ..Default::default() };
        let mut reused = reader(&descriptor, &settings);
        let mut from_path = reader(&descriptor, &settings);
        let first = reused.render_source(&source, viewport).unwrap();
        assert_same_frame(&first, &from_path.render(&path, viewport).unwrap());
        if mode == DisplayMode::Page {
            assert_eq!((first.crop_index, first.crop_count), (0, 1));
            let anchor = reused.anchor(0);
            assert_eq!(anchor, from_path.anchor(0));
            let mut restored = reader(&descriptor, &settings);
            restored.restore(&anchor).unwrap();
            assert_same_frame(&restored.render_source(&source, viewport).unwrap(), &first);
            assert_eq!(restored.anchor(0), anchor);
            assert_eq!(reused.step(false), ReaderMove::Moved);
            assert_eq!(reused.render_source(&source, viewport).unwrap().page_index, 0);
            assert_eq!(reused.step(true), ReaderMove::Moved);
            assert_same_frame(&reused.render_source(&source, viewport).unwrap(), &first);
            continue;
        }
        assert!(first.crop_count > 2);
        for _ in 0..2 {
            assert_eq!(reused.step(true), ReaderMove::Moved);
            assert_eq!(from_path.step(true), ReaderMove::Moved);
            let selected = reused.render_source(&source, viewport).unwrap();
            assert_same_frame(&selected, &from_path.render(&path, viewport).unwrap());
            let anchor = reused.anchor(0);
            assert_eq!(anchor, from_path.anchor(0));
            assert_eq!(anchor.page_index, 1);
            assert!(anchor.source_y > 0.0);
            assert_same_frame(&reused.render_source(&source, viewport).unwrap(), &selected);
            assert_eq!(reused.anchor(0), anchor);

            let mut restored = reader(&descriptor, &settings);
            restored.restore(&anchor).unwrap();
            assert_same_frame(&restored.render_source(&source, viewport).unwrap(), &selected);
            assert_eq!(restored.anchor(0), anchor);
            let mut restored_path = reader(&descriptor, &settings);
            restored_path.restore(&anchor).unwrap();
            assert_same_frame(&restored_path.render(&path, viewport).unwrap(), &selected);
            assert_eq!(restored_path.anchor(0), anchor);
        }
    }
}

#[test]
fn rotating_a_reusable_source_does_not_mutate_cached_pixels_or_source_geometry() {
    let pixels = GrayImage::from_fn(12, 20, |x, y| Luma([match (x < 6, y < 10) {
        (true, true) => 17, (false, true) => 68,
        (true, false) => 136, (false, false) => 204,
    }]));
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(pixels).write_to(&mut encoded, ImageFormat::Png).unwrap();
    let encoded = encoded.into_inner();
    let resources = ReaderResources::new(1024 * 1024);
    let source = decode_bounded_bytes_with_resources(&encoded, &resources).unwrap();
    let original = source.as_raw().clone();
    let descriptor = index(12, 20);
    for rotation in [0, 90, 180, 270] {
        let settings = ReaderSettings { rotation, dither: false, landscape_spread: false, ..Default::default() };
        let mut core = reader(&descriptor, &settings);
        let rendered = core.render_source(&source, Viewport::new(24, 40)).unwrap();
        let expected = if rotation == 90 || rotation == 270 { (20, 12) } else { (12, 20) };
        assert_eq!((rendered.source_width, rendered.source_height), expected);
        let retained = decode_bounded_bytes_with_resources(&encoded, &resources).unwrap();
        assert!(Arc::ptr_eq(&source, &retained));
        assert_eq!(retained.dimensions(), (12, 20));
        assert_eq!(retained.as_raw(), &original);
    }
}

#[test]
fn a_reuse_callback_cannot_substitute_another_sealed_encoded_identity() {
    let current = encoded_source(8, 12, 17);
    let other = encoded_source(8, 12, 204);
    let wrong = image_integrity::validate_image(&other, None).unwrap();
    let (proof, reused) = image_integrity::decode_image_with_reuse(&current, None, |digest| {
        assert_ne!(digest, wrong.sha256());
        Some((wrong.clone(), "incorrect cached pixels"))
    }).unwrap();
    assert_ne!(proof.sha256(), wrong.sha256());
    match reused {
        ReusableImage::Decoded(decoded) => assert!(decoded.into_luma8().pixels().all(|pixel| pixel[0] == 17)),
        ReusableImage::Reused(_) => panic!("A different encoded identity must not supply the current pixels"),
    }
}

#[test]
fn an_exact_reuse_hit_still_enforces_the_current_pixel_policy() {
    let encoded = encoded_source(8, 12, 17);
    let proof = image_integrity::validate_image(&encoded, None).unwrap();
    let (accepted, pixels) = image_integrity::decode_image_with_reuse(&encoded, Some(96), |digest| {
        assert_eq!(digest, proof.sha256());
        Some((proof.clone(), "validated source"))
    }).unwrap();
    assert_eq!(accepted, proof);
    assert!(matches!(pixels, ReusableImage::Reused("validated source")));
    let error = match image_integrity::decode_image_with_reuse(&encoded, Some(95), |_| {
        Some((proof.clone(), "validated source"))
    }) {
        Err(error) => error,
        Ok(_) => panic!("A cached source must not bypass a stricter pixel policy"),
    };
    assert_eq!(error.kind, IntegrityFailureKind::ResourceLimit);
    assert_eq!(error.code, "image_pixel_limit");
}

#[test]
fn same_geometry_source_replacements_reflow_gutters_around_the_retained_source_anchor() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("changing-gutters.png");
    let original = encoded_source(60, 400, 0);
    let replacement = GrayImage::from_fn(60, 400, |_, y| Luma([if (90..100).contains(&y) { 255 } else { 0 }]));
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(replacement).write_to(&mut encoded, ImageFormat::Png).unwrap();
    let replacement = encoded.into_inner();
    let descriptor = index(60, 400);
    let viewport = Viewport::new(60, 100);
    let resources = ReaderResources::new(1024 * 1024);
    for (mode, fit) in [(DisplayMode::Strip, FitMode::Page), (DisplayMode::Panel, FitMode::Page)] {
        let settings = ReaderSettings { mode, fit, dither: false, landscape_spread: false, ..Default::default() };
        // Use the heuristic panel path, without the explicit fixture panel map.
        let mut current = ReaderCore::new(descriptor.clone(), settings.clone()).unwrap();
        current.set_page(1).unwrap();
        fs::write(&path, &original).unwrap();
        let first_source = load_validated_source_with_resources(&path, &resources).unwrap();
        let first = current.render_validated_source(&first_source, viewport).unwrap();
        assert!(first.crop_count > 2);
        for _ in 0..2 {
            assert_eq!(current.step(true), ReaderMove::Moved);
            current.render_validated_source(&first_source, viewport).unwrap();
        }
        let selected = current.render_validated_source(&first_source, viewport).unwrap();
        let anchor = current.anchor(0);
        assert_eq!(selected.crop_index, 2);
        assert!(anchor.source_y > 0.0);

        fs::write(&path, &replacement).unwrap();
        let changed_source = load_validated_source_with_resources(&path, &resources).unwrap();
        let mut reference = ReaderCore::new(descriptor.clone(), settings).unwrap();
        reference.restore(&anchor).unwrap();
        let expected = reference.render_validated_source(&changed_source, viewport).unwrap();
        assert_eq!(expected.crop_index, 0);
        assert_eq!(expected.crop.y, 0);
        assert!(expected.crop.height < first.crop.height);
        assert_ne!(expected.crop_count, first.crop_count);
        let changed = current.render_validated_source(&changed_source, viewport).unwrap();
        assert_same_frame(&changed, &expected);
        assert_eq!(current.anchor(0), reference.anchor(0));
        assert_ne!(changed.crop, selected.crop);
        assert_same_frame(&current.render_validated_source(&changed_source, viewport).unwrap(), &expected);
    }
}

#[test]
fn same_geometry_source_replacements_refresh_pixels_without_reflowing_centered_page_fits() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("changing-centered-page.png");
    let original = encoded_source(60, 400, 0);
    let changed = GrayImage::from_fn(60, 400, |_, y| Luma([if (190..200).contains(&y) { 255 } else { 0 }]));
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(changed).write_to(&mut encoded, ImageFormat::Png).unwrap();
    let changed = encoded.into_inner();
    let descriptor = index(60, 400);
    let viewport = Viewport::new(60, 100);
    let resources = ReaderResources::new(1024 * 1024);
    for fit in [FitMode::Original, FitMode::Page, FitMode::Height, FitMode::Width] {
        let settings = ReaderSettings { fit, dither: false, landscape_spread: false, ..Default::default() };
        let mut current = reader(&descriptor, &settings);
        fs::write(&path, &original).unwrap();
        let first_source = load_validated_source_with_resources(&path, &resources).unwrap();
        let before = current.render_validated_source(&first_source, viewport).unwrap();
        let anchor = current.anchor(0);

        fs::write(&path, &changed).unwrap();
        let replacement = load_validated_source_with_resources(&path, &resources).unwrap();
        let after = current.render_validated_source(&replacement, viewport).unwrap();
        assert_eq!((after.crop_index, after.crop_count), (0, 1));
        assert_eq!(after.crop, before.crop);
        assert_eq!(current.anchor(0), anchor);
        assert_ne!(after.rgba, before.rgba);
        let mut restored = reader(&descriptor, &settings);
        restored.restore(&anchor).unwrap();
        assert_same_frame(&restored.render_validated_source(&replacement, viewport).unwrap(), &after);
        assert_same_frame(&current.render_validated_source(&first_source, viewport).unwrap(), &before);
    }
}

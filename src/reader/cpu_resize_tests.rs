//! Sealed CPU resize reuse must preserve complete frames and cancellation.

use super::*;
use crate::domain::PageDescriptor;
use image::Luma;
use std::{path::PathBuf, sync::atomic::{AtomicBool, Ordering}};

struct Fixture {
    _directory: tempfile::TempDir,
    path: PathBuf,
}

fn pixels(width: u32, height: u32, salt: u32) -> GrayImage {
    GrayImage::from_fn(width, height, |x, y| Luma([((x * 19 + y * 31 + x * y % 97 + salt) % 256) as u8]))
}

fn fixture(width: u32, height: u32) -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cpu-resize.png");
    pixels(width, height, 0).save(&path).unwrap();
    Fixture { _directory: directory, path }
}

fn settings() -> ReaderSettings {
    ReaderSettings { mode: DisplayMode::Page, fit: FitMode::Page, rotation: 0, zoom_ratio: 1.0,
        spatial_dither: Some(SpatialDither::Bayer8), dither: true, hardware_dither: false,
        landscape_spread: false, ..Default::default() }
}

fn core(source: &ValidatedGraySource, settings: ReaderSettings) -> ReaderCore {
    ReaderCore::new(ImageIndex { episode_id: "cpu-resize-episode".into(), revision: "cpu-resize-v1".into(),
        images: vec![PageDescriptor { id: "cpu-resize-page".into(), index: 0,
            width: source.proof().width(), height: source.proof().height() }], ..Default::default() }, settings).unwrap()
}

fn render(core: &mut ReaderCore, source: &ValidatedGraySource, viewport: Viewport,
    resources: &ReaderResources, cancellation: &RenderCancellation<'_>, profile: &mut ReaderPreparationProfile)
    -> Option<RenderedPage>
{
    core.render_source_identity_cancellable_with_resources(&source.pixels, viewport,
        Some(source.proof().sha256()), Some(source.proof()), resources, cancellation, Some(profile)).unwrap()
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

fn assert_complete(profile: &ReaderPreparationProfile, stage: &'static str) {
    let value = &profile.stages[stage];
    assert_eq!((value.attempts, value.completed, value.partial), (1, 1, 0));
}

#[test]
fn cold_and_warm_sealed_resize_preserve_exact_frame_and_anchor() {
    let input = fixture(113, 157);
    let resources = ReaderResources::new(1024 * 1024);
    let source = load_validated_source_with_resources(&input.path, &resources).unwrap();
    let viewport = Viewport::new(83, 109);
    let mut reference = core(&source, settings());
    let expected = render(&mut reference, &source, viewport, &ReaderResources::new(0),
        &RenderCancellation::never(), &mut ReaderPreparationProfile::default()).unwrap();
    let anchor = reference.anchor(123);
    let mut reader = core(&source, settings());
    for warm in [false, true] {
        let mut profile = ReaderPreparationProfile::default();
        let actual = render(&mut reader, &source, viewport, &resources, &RenderCancellation::never(), &mut profile).unwrap();
        assert_frame(&actual, &expected);
        assert_eq!(reader.anchor(123), anchor);
        assert_eq!(profile.cpu_resize_cache_hits, u32::from(warm));
        assert_eq!(profile.cpu_resize_cache_inserts, u32::from(!warm));
        assert_eq!(profile.processed_source_hits, 0);
        assert_eq!(profile.native_sources, 0);
        assert_complete(&profile, "software_resize_cache_lookup");
        assert_complete(&profile, "canvas");
        assert_eq!(profile.stages.contains_key("resize"), !warm);
        if !warm { assert_complete(&profile, "resize"); assert_complete(&profile, "software_resize_cache_insert"); }
        // This legacy count describes a completed frame, including a cache hit.
        assert_eq!(profile.resized_pixels, u64::from(viewport.width) * u64::from(viewport.height));
        assert_eq!(profile.rendered_frames, 1);
        assert_eq!(resources.processed_cache_usage().entries, 1);
    }
}

#[test]
fn output_algorithms_share_only_the_undithered_resize() {
    let input = fixture(113, 157);
    let resources = ReaderResources::new(1024 * 1024);
    let source = load_validated_source_with_resources(&input.path, &resources).unwrap();
    let viewport = Viewport::new(83, 109);
    let mut outputs = Vec::new();
    for (index, (fast, enabled, method)) in [
        (false, false, SpatialDither::Off), (false, true, SpatialDither::Bayer8),
        (false, true, SpatialDither::FloydSteinberg), (true, true, SpatialDither::Bayer8),
    ].into_iter().enumerate() {
        let settings = ReaderSettings { fast_mode: fast, dither: enabled, spatial_dither: Some(method), ..settings() };
        let mut reference = core(&source, settings.clone());
        let expected = render(&mut reference, &source, viewport, &ReaderResources::new(0),
            &RenderCancellation::never(), &mut ReaderPreparationProfile::default()).unwrap();
        let mut reader = core(&source, settings);
        let mut profile = ReaderPreparationProfile::default();
        let actual = render(&mut reader, &source, viewport, &resources, &RenderCancellation::never(), &mut profile).unwrap();
        assert_frame(&actual, &expected);
        assert_eq!(reader.anchor(0), reference.anchor(0));
        assert_eq!(profile.cpu_resize_cache_hits, u32::from(index != 0));
        assert_eq!(profile.processed_source_hits, 0);
        assert_complete(&profile, "canvas");
        assert_eq!(resources.processed_cache_usage().entries, 1);
        assert!(!outputs.contains(&actual.rgba), "Final algorithms must not reuse one another's output");
        assert!(actual.rgba.chunks_exact(4).all(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2]
            && pixel[3] == 255 && if fast { pixel[0] == 0 || pixel[0] == 255 } else { pixel[0] % 17 == 0 }));
        outputs.push(actual.rgba);
    }
}

#[test]
fn current_content_proof_viewport_and_media_policy_separate_resize_entries() {
    let input = fixture(113, 157);
    let resources = ReaderResources::new(1024 * 1024);
    let original = load_validated_source_with_resources(&input.path, &resources).unwrap();
    let viewport = Viewport::new(83, 109);
    let mut reader = core(&original, settings());
    render(&mut reader, &original, viewport, &resources, &RenderCancellation::never(),
        &mut ReaderPreparationProfile::default()).unwrap();
    pixels(113, 157, 73).save(&input.path).unwrap();
    // Reload the actual bytes: a validated in-memory source is an immutable
    // snapshot, and this renderer does not reopen paths or fabricate new proofs.
    let replacement = load_validated_source_with_resources(&input.path, &resources).unwrap();
    assert_ne!(original.proof(), replacement.proof());
    for (source, viewport, policy) in [
        (&replacement, viewport, crate::native_media::Acceleration::Off),
        (&original, Viewport::new(85, 111), crate::native_media::Acceleration::Off),
        (&original, viewport, crate::native_media::Acceleration::DecodeAndResize),
    ] {
        let settings = ReaderSettings { media_acceleration: policy, ..settings() };
        let mut reference = core(source, settings.clone());
        let expected = render(&mut reference, source, viewport, &ReaderResources::new(0),
            &RenderCancellation::never(), &mut ReaderPreparationProfile::default()).unwrap();
        let mut reader = core(source, settings);
        let mut profile = ReaderPreparationProfile::default();
        let actual = render(&mut reader, source, viewport, &resources, &RenderCancellation::never(), &mut profile).unwrap();
        assert_frame(&actual, &expected);
        assert_eq!(profile.cpu_resize_cache_hits, 0);
        assert_complete(&profile, "resize");
    }
    assert_eq!(resources.processed_cache_usage().entries, 4);
}

#[test]
fn raw_identity_rotated_zoomed_cropped_and_identity_size_paths_do_not_use_the_cache() {
    let input = fixture(113, 157);
    let resources = ReaderResources::new(1024 * 1024);
    let source = load_validated_source_with_resources(&input.path, &resources).unwrap();
    for case in ["raw", "unbound-identity", "rotated", "zoomed", "cropped-width", "cropped-panel", "strip", "same-size"] {
        let mut settings = settings();
        match case {
            "rotated" => settings.rotation = 90,
            "zoomed" => settings.zoom_ratio = 1.5,
            "cropped-width" => settings.fit = FitMode::Width,
            "cropped-panel" => settings.mode = DisplayMode::Panel,
            "strip" => { settings.mode = DisplayMode::Strip; settings.fit = FitMode::Width; },
            _ => {}
        }
        let viewport = if case == "same-size" { Viewport::new(113, 157) } else { Viewport::new(83, 109) };
        let mut reader = core(&source, settings);
        if case == "cropped-panel" {
            reader.set_panel_map("cpu-resize-page", vec![NormalizedRect { x: 0.1, y: 0.2, width: 0.6, height: 0.5 }]).unwrap();
        }
        let mut profile = ReaderPreparationProfile::default();
        let proof = (case != "raw").then_some(source.proof());
        let identity = if case == "unbound-identity" { "untrusted-logical-page" } else { source.proof().sha256() };
        assert!(reader.render_source_identity_cancellable_with_resources(&source.pixels, viewport, Some(identity), proof,
            &resources, &RenderCancellation::never(), Some(&mut profile)).unwrap().is_some());
        assert_eq!((profile.cpu_resize_cache_hits, profile.cpu_resize_cache_inserts), (0, 0), "Unexpected cache for {case}");
        assert!(!profile.stages.contains_key("software_resize_cache_lookup"), "Unexpected lookup for {case}");
        assert!(!profile.stages.contains_key("software_resize_cache_insert"), "Unexpected insertion for {case}");
        assert_eq!(resources.processed_cache_usage().entries, 0);
    }
}

#[test]
fn cancellation_inside_resize_never_inserts_partial_gray_pixels() {
    let input = fixture(361, 517);
    let resources = ReaderResources::new(1024 * 1024);
    let source = load_validated_source_with_resources(&input.path, &resources).unwrap();
    let mut reader = core(&source, settings());
    let viewport = Viewport::new(400, 600);
    let cancelled = AtomicBool::new(false);
    let probe = |work, completed| {
        if work == RenderWork::ResizeVertical && completed > 0 { cancelled.store(true, Ordering::Release); }
    };
    let mut profile = ReaderPreparationProfile::default();
    assert!(render(&mut reader, &source, viewport, &resources,
        &RenderCancellation::with_probe(&cancelled, &probe), &mut profile).is_none());
    assert!(cancelled.load(Ordering::Acquire));
    let resize = &profile.stages["resize"];
    assert_eq!((resize.attempts, resize.completed, resize.partial), (1, 0, 1));
    assert_eq!(profile.cpu_resize_cache_inserts, 0);
    assert!(!profile.stages.contains_key("software_resize_cache_insert"));
    assert_eq!(resources.processed_cache_usage().entries, 0);
    let mut retry = ReaderPreparationProfile::default();
    assert!(render(&mut reader, &source, viewport, &resources, &RenderCancellation::never(), &mut retry).is_some());
    assert_eq!((retry.cpu_resize_cache_hits, retry.cpu_resize_cache_inserts), (0, 1));
    assert_complete(&retry, "resize");
}

#[test]
fn cancelled_canvas_retains_only_complete_gray_and_never_returns_a_frame_on_a_hit() {
    let input = fixture(113, 157);
    let resources = ReaderResources::new(1024 * 1024);
    let source = load_validated_source_with_resources(&input.path, &resources).unwrap();
    let viewport = Viewport::new(83, 109);
    let mut reader = core(&source, settings());
    for warm in [false, true] {
        let cancelled = AtomicBool::new(false);
        let probe = |work, completed| {
            // This small canvas fits in one fill block. Cancel during the
            // subsequent row copy, after the complete resize was retained.
            if work == RenderWork::CanvasGrayCopy && completed > 0 { cancelled.store(true, Ordering::Release); }
        };
        let mut profile = ReaderPreparationProfile::default();
        assert!(render(&mut reader, &source, viewport, &resources,
            &RenderCancellation::with_probe(&cancelled, &probe), &mut profile).is_none());
        assert!(cancelled.load(Ordering::Acquire));
        assert_eq!(profile.cpu_resize_cache_hits, u32::from(warm));
        assert_eq!(profile.cpu_resize_cache_inserts, u32::from(!warm));
        assert_eq!(profile.stages.contains_key("resize"), !warm);
        assert_eq!((profile.stages["canvas"].completed, profile.stages["canvas"].partial), (0, 1));
        assert_eq!((profile.rendered_frames, profile.output_bytes), (0, 0));
        assert_eq!(resources.processed_cache_usage().entries, 1, "A complete undithered resize may survive canvas cancellation");
    }
    let mut reference = core(&source, settings());
    let expected = render(&mut reference, &source, viewport, &ReaderResources::new(0),
        &RenderCancellation::never(), &mut ReaderPreparationProfile::default()).unwrap();
    let actual = render(&mut reader, &source, viewport, &resources, &RenderCancellation::never(),
        &mut ReaderPreparationProfile::default()).unwrap();
    assert_frame(&actual, &expected);
}

#[test]
fn zero_and_subentry_budgets_keep_rendering_without_retention() {
    let input = fixture(113, 157);
    let viewport = Viewport::new(83, 109);
    for budget in [0, 1, (83 * 109) - 1] {
        let resources = ReaderResources::new(budget);
        let source = load_validated_source_with_resources(&input.path, &resources).unwrap();
        let mut reference = core(&source, settings());
        let expected = render(&mut reference, &source, viewport, &ReaderResources::new(0),
            &RenderCancellation::never(), &mut ReaderPreparationProfile::default()).unwrap();
        let mut reader = core(&source, settings());
        for _ in 0..2 {
            let mut profile = ReaderPreparationProfile::default();
            let actual = render(&mut reader, &source, viewport, &resources, &RenderCancellation::never(), &mut profile).unwrap();
            assert_frame(&actual, &expected);
            assert_eq!(reader.anchor(123), reference.anchor(123));
            assert_eq!((profile.cpu_resize_cache_hits, profile.cpu_resize_cache_inserts), (0, 0));
            assert_complete(&profile, "resize");
            assert_eq!(resources.processed_cache_usage().retained_bytes, 0);
        }
    }
}

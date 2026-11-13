//! Gray8 handoff must preserve source precision and existing disabled semantics.

use super::*;
use crate::domain::PageDescriptor;
use image::Luma;

fn index(width: u32, height: u32) -> ImageIndex {
    ImageIndex { episode_id: "hardware-dither-episode".into(), revision: "hardware-dither-revision".into(),
        images: (0..2).map(|position| PageDescriptor { id: format!("hardware-dither-{position}"),
            index: position, width, height }).collect(), ..Default::default() }
}

#[test]
fn hardware_handoff_preserves_every_gray8_value_in_both_canvas_paths() {
    let source = GrayImage::from_fn(256, 8, |x, _| Luma([x as u8]));
    let viewport = Viewport::new(256, 8);
    let expected = source.as_raw().iter().flat_map(|&gray| [gray, gray, gray, 255]).collect::<Vec<_>>();
    let mut core = ReaderCore::new(index(256, 8), ReaderSettings {
        hardware_dither: true, landscape_spread: false, ..Default::default()
    }).unwrap();
    let frame = core.render_source(&source, viewport).unwrap();
    assert_eq!(frame.rgba, expected);
    assert_eq!(frame.rgba.chunks_exact(4).map(|pixel| pixel[0]).collect::<BTreeSet<_>>().len(), 256);

    let cancelled = AtomicBool::new(false);
    let cancellation = RenderCancellation::for_flag(&cancelled);
    let mut cancellable = ReaderCore::new(index(256, 8), core.settings().clone()).unwrap();
    let frame = cancellable.render_source_identity_cancellable(&source, viewport,
        Some("hardware-dither-source"), &cancellation, None).unwrap().unwrap();
    assert_eq!(frame.rgba, expected);
}

#[test]
fn disabling_dither_keeps_sixteen_levels_even_with_a_stale_hardware_field() {
    let source = GrayImage::from_fn(256, 8, |x, _| Luma([x as u8]));
    let viewport = Viewport::new(256, 8);
    let settings = ReaderSettings { dither: false, landscape_spread: false, ..Default::default() };
    let mut software = ReaderCore::new(index(256, 8), settings.clone()).unwrap();
    let mut disabled = ReaderCore::new(index(256, 8), ReaderSettings { hardware_dither: true, ..settings }).unwrap();
    assert!(!disabled.settings().uses_hardware_dither());
    let expected = software.render_source(&source, viewport).unwrap();
    let actual = disabled.render_source(&source, viewport).unwrap();
    assert_eq!(actual.rgba, expected.rgba);
    let values = actual.rgba.chunks_exact(4).map(|pixel| pixel[0]).collect::<BTreeSet<_>>();
    assert_eq!(values, (0..=15).map(|level| level * 17).collect());
}

#[test]
fn switching_the_output_path_preserves_the_active_page_crop_rotation_and_zoom() {
    let source = GrayImage::from_fn(160, 240, |x, y| Luma([((x * 7 + y * 11) % 256) as u8]));
    let viewport = Viewport::new(128, 160);
    let mut core = ReaderCore::new(index(160, 240), ReaderSettings {
        fit: FitMode::Width, rotation: 90, zoom_ratio: 1.5,
        landscape_spread: false, ..Default::default()
    }).unwrap();
    core.set_page(1).unwrap();
    let before = core.render_source(&source, viewport).unwrap();
    let anchor = core.anchor(0);
    let mut settings = core.settings().clone();
    settings.hardware_dither = true;
    core.update_settings(settings).unwrap();
    let after = core.render_source(&source, viewport).unwrap();
    assert_eq!(core.anchor(0), anchor);
    assert_eq!((after.page_index, after.crop_index, after.crop_count, after.crop),
        (before.page_index, before.crop_index, before.crop_count, before.crop));
    assert_eq!(core.settings().rotation, 90);
    assert_eq!(core.settings().zoom_ratio, 1.5);
}

#[test]
fn cancelling_raw_canvas_work_never_publishes_a_partial_frame() {
    let source = GrayImage::from_fn(361, 517, |x, y| Luma([((x + y) % 256) as u8]));
    let cancelled = AtomicBool::new(false);
    let probe = |work, completed| {
        if matches!(work, RenderWork::CanvasPixels) && completed > 0 {
            cancelled.store(true, Ordering::Release);
        }
    };
    let cancellation = RenderCancellation::with_probe(&cancelled, &probe);
    let mut core = ReaderCore::new(index(361, 517), ReaderSettings {
        hardware_dither: true, landscape_spread: false, ..Default::default()
    }).unwrap();
    let mut profile = ReaderPreparationProfile::default();
    let result = core.render_source_identity_cancellable(&source, Viewport::new(400, 600),
        Some("hardware-dither-cancel-source"), &cancellation, Some(&mut profile)).unwrap();
    assert!(cancelled.load(Ordering::Acquire));
    assert!(result.is_none());
    assert_eq!(profile.rendered_frames, 0);
    assert_eq!(profile.output_bytes, 0);
}

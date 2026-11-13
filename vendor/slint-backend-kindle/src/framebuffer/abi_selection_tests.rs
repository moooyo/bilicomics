//! Reviewed ABI selection is independent of the currently displayed dither mode.

use std::cell::Cell;

use super::{Framebuffer, UpdateVariant, mtk_update};
use super::ffi::{EPDC_FLAG_FORCE_MONOCHROME, MTK_EPDC_FLAG_USE_DITHERING_Y4,
    UPDATE_MODE_FULL, UPDATE_MODE_PARTIAL, WAVEFORM_MODE_AUTO, WAVEFORM_MODE_DU,
    WAVEFORM_MODE_GC16, UpdateRect};
use crate::DitheringHandle;

const SENTINEL: u8 = 0x5a;

#[test]
fn a_reviewed_mtk_startup_selects_mtk_before_any_y4_request() {
    let mut framebuffer = Framebuffer::memory_for_test(4, 1, 7, 1, SENTINEL).unwrap();
    let handle = DitheringHandle::for_reviewed_mtk_test();
    assert!(handle.reviewed_mtk_abi());
    assert!(!handle.requested());
    framebuffer.set_dithering(handle.clone(), false);
    let mut calls = Vec::new();

    let result = framebuffer.send_update_using(UpdateRect { left: 0, top: 0, width: 4, height: 1 },
        WAVEFORM_MODE_AUTO, UPDATE_MODE_PARTIAL,
        |driver, _, waveform, mode, marker, y4| {
            calls.push((driver, marker, y4));
            assert_eq!((waveform, mode), (WAVEFORM_MODE_AUTO, UPDATE_MODE_PARTIAL));
            Ok(())
        }, || panic!("A non-Y4 startup must not enter fallback"));

    assert_eq!(calls, [(UpdateVariant::Mtk, 1, false)]);
    assert!(result.accepted);
    assert_eq!(result.ioctl_attempts, 1);
    assert_eq!(result.update_marker, Some(1));
    assert_eq!(result.refresh_abi, Some("mtk"));
    assert!(!result.y4_attempted && !result.software_dither_fallback);
    assert!(handle.reviewed_mtk_abi());
}

#[test]
fn reviewed_mtk_bilevel_feedback_keeps_the_abi_without_a_y4_flag() {
    let mut framebuffer = Framebuffer::memory_for_test(4, 1, 7, 1, SENTINEL).unwrap();
    let handle = DitheringHandle::for_reviewed_mtk_test();
    assert!(handle.set_hardware_y4(true));
    framebuffer.set_dithering(handle.clone(), true);
    let mut calls = Vec::new();

    let result = framebuffer.send_update_using(UpdateRect { left: 0, top: 0, width: 4, height: 1 },
        WAVEFORM_MODE_DU, UPDATE_MODE_PARTIAL,
        |driver, region, waveform, mode, marker, y4| {
            calls.push((driver, marker, y4));
            assert_eq!(waveform, WAVEFORM_MODE_DU);
            let update = mtk_update(region, waveform, mode, marker, y4);
            assert_eq!(update.flags & MTK_EPDC_FLAG_USE_DITHERING_Y4, 0);
            assert_eq!(update.flags, EPDC_FLAG_FORCE_MONOCHROME);
            Ok(())
        }, || panic!("Bilevel feedback must not enter Y4 fallback"));

    assert_eq!(calls, [(UpdateVariant::Mtk, 1, false)]);
    assert!(result.accepted && !result.y4_attempted);
    assert_eq!(result.refresh_abi, Some("mtk"));
    assert!(handle.reviewed_mtk_abi());
}

#[test]
fn known_y4_rejections_preserve_the_reviewed_mtk_abi_for_software_fallback() {
    for errno in [libc::EINVAL, libc::ENOTTY, libc::EOPNOTSUPP] {
        let mut framebuffer = Framebuffer::memory_for_test(4, 1, 7, 1, SENTINEL).unwrap();
        let handle = DitheringHandle::for_reviewed_mtk_test();
        assert!(handle.set_hardware_y4(true));
        framebuffer.set_dithering(handle.clone(), false);
        framebuffer.write_line(0, 0..4, &[110; 4]);
        let waited = Cell::new(false);
        let mut calls = Vec::new();

        let result = framebuffer.send_update_using(UpdateRect { left: 0, top: 0, width: 4, height: 1 },
            WAVEFORM_MODE_AUTO, UPDATE_MODE_PARTIAL,
            |driver, region, waveform, mode, marker, y4| {
                calls.push((driver, marker, y4));
                assert_eq!(driver, UpdateVariant::Mtk);
                assert_eq!(waveform, WAVEFORM_MODE_GC16);
                if y4 {
                    assert_eq!((marker, mode), (1, UPDATE_MODE_PARTIAL));
                    Err(Some(errno))
                } else {
                    assert!(waited.get());
                    assert_eq!((marker, mode), (2, UPDATE_MODE_FULL));
                    assert_eq!((region.left, region.top, region.width, region.height), (0, 0, 4, 1));
                    assert!(framebuffer.mapped_bytes_for_test()[..4].iter().all(|value| value % 17 == 0));
                    assert_eq!(mtk_update(region, waveform, mode, marker, y4).flags & MTK_EPDC_FLAG_USE_DITHERING_Y4, 0);
                    Ok(())
                }
            }, || { waited.set(true); Ok(()) });

        assert_eq!(calls, [(UpdateVariant::Mtk, 1, true), (UpdateVariant::Mtk, 2, false)]);
        assert!(result.accepted && result.y4_attempted && result.software_dither_fallback);
        assert_eq!(result.ioctl_attempts, 2);
        assert_eq!(result.update_marker, Some(2));
        assert_eq!(result.refresh_abi, Some("mtk"));
        assert_eq!(result.last_errno, None);
        assert!(!handle.hardware_y4_available());
        assert!(handle.requested());
        assert!(handle.reviewed_mtk_abi());
        assert!(handle.clone().reviewed_mtk_abi());
        assert_eq!(framebuffer.update_variant.get(), Some(UpdateVariant::Mtk));
    }
}

#[test]
fn unreviewed_handles_preserve_the_ordered_abi_probe() {
    for handle in [DitheringHandle::default(), DitheringHandle::for_test(false), DitheringHandle::for_test(true)] {
        let mut framebuffer = Framebuffer::memory_for_test(4, 1, 7, 1, SENTINEL).unwrap();
        assert!(!handle.reviewed_mtk_abi());
        framebuffer.set_dithering(handle, false);
        let mut calls = Vec::new();

        let result = framebuffer.send_update_using(UpdateRect { left: 0, top: 0, width: 4, height: 1 },
            WAVEFORM_MODE_AUTO, UPDATE_MODE_PARTIAL,
            |driver, _, _, _, marker, y4| {
                calls.push(driver);
                assert_eq!(marker, 1);
                assert!(!y4);
                match driver {
                    UpdateVariant::Legacy => Err(Some(libc::EINVAL)),
                    UpdateVariant::Rex => Err(Some(libc::ENOTTY)),
                    UpdateVariant::Zelda => Ok(()),
                    _ => panic!("The accepted Zelda probe must end this attempt"),
                }
            }, || panic!("An unrequested dither mode must not enter fallback"));

        assert_eq!(calls, [UpdateVariant::Legacy, UpdateVariant::Rex, UpdateVariant::Zelda]);
        assert!(result.accepted && !result.y4_attempted);
        assert_eq!(result.ioctl_attempts, 3);
        assert_eq!(result.refresh_abi, Some("zelda"));
        assert_eq!(result.update_marker, Some(1));
    }
}

#[test]
fn an_interrupted_unknown_abi_probe_stops_at_legacy_and_blocks_future_writes() {
    for handle in [DitheringHandle::default(), DitheringHandle::for_test(false), DitheringHandle::for_test(true)] {
        let mut framebuffer = Framebuffer::memory_for_test(4, 1, 7, 1, SENTINEL).unwrap();
        framebuffer.set_dithering(handle, false);
        let before = framebuffer.mapped_bytes_for_test().to_vec();
        let mut calls = Vec::new();

        let result = framebuffer.send_update_using(UpdateRect { left: 0, top: 0, width: 4, height: 1 },
            WAVEFORM_MODE_AUTO, UPDATE_MODE_PARTIAL,
            |driver, _, _, _, marker, y4| {
                calls.push((driver, marker, y4));
                Err(Some(libc::EINTR))
            }, || panic!("Uncertain submission must not enter fallback"));

        assert_eq!(calls, [(UpdateVariant::Legacy, 1, false)]);
        assert!(!result.accepted && result.dithering_delivery_uncertain);
        assert_eq!(result.ioctl_attempts, 1);
        assert_eq!(result.refresh_abi, Some("legacy"));
        assert_eq!(result.last_errno, Some(libc::EINTR));
        assert_eq!(result.update_marker, None);
        framebuffer.fill(0);
        framebuffer.write_line(0, 0..4, &[0; 4]);
        framebuffer.software_dither_mapping();
        assert_eq!(framebuffer.mapped_bytes_for_test(), before);
        assert!(framebuffer.write_after_pending_update(|_| panic!("An uncertain mapping must remain closed")).is_err());
        let later = framebuffer.send_update_using(UpdateRect { left: 0, top: 0, width: 4, height: 1 },
            WAVEFORM_MODE_GC16, UPDATE_MODE_FULL,
            |_, _, _, _, _, _| panic!("An uncertain attempt must not be retried"),
            || panic!("An uncertain mapping must not run a fallback wait"));
        assert!(!later.accepted && later.dithering_delivery_uncertain);
        assert_eq!(later.ioctl_attempts, 0);
        assert_eq!(later.refresh_abi, None);
    }
}

#[test]
fn a_reviewed_identity_does_not_preselect_mtk_for_a_color_mapping() {
    for requested in [false, true] {
        let mut framebuffer = Framebuffer::memory_for_test(4, 1, 19, 4, SENTINEL).unwrap();
        let handle = DitheringHandle::for_reviewed_mtk_test();
        handle.set_hardware_y4(requested);
        framebuffer.set_dithering(handle.clone(), false);
        assert!(handle.reviewed_mtk_abi());
        assert!(!handle.hardware_y4_available());
        let mut calls = Vec::new();

        let result = framebuffer.send_update_using(UpdateRect { left: 0, top: 0, width: 4, height: 1 },
            WAVEFORM_MODE_AUTO, UPDATE_MODE_PARTIAL,
            |driver, _, _, _, marker, y4| {
                calls.push((driver, marker, y4));
                Ok(())
            }, || panic!("A color mapping cannot enter hardware Y4 fallback"));

        assert_eq!(calls, [(UpdateVariant::Legacy, 1, false)]);
        assert!(result.accepted && !result.y4_attempted);
        assert_eq!(result.refresh_abi, Some("legacy"));
        assert!(handle.reviewed_mtk_abi());
    }
}

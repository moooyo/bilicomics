use std::cell::Cell;

use super::{Framebuffer, UpdateVariant, aligned_dither_region, mtk_update};
use super::ffi::{EPDC_FLAG_FORCE_MONOCHROME, MTK_EPDC_FLAG_USE_DITHERING_Y4,
    UPDATE_MODE_FULL, UPDATE_MODE_PARTIAL, WAVEFORM_MODE_A2_MODERN, WAVEFORM_MODE_AUTO,
    WAVEFORM_MODE_DU, WAVEFORM_MODE_GC16, UpdateRect, UpdateRequestMtk};
use crate::DitheringHandle;
use crate::dithering::software_gray;
use crate::refresh::{PhysicalRect, RefreshWaveform, ResolvedRefresh};

const SENTINEL: u8 = 0x5a;

#[test]
fn mtk_y4_uses_its_own_flag_and_zero_epdcv2_dither_fields() {
    let region = UpdateRect { left: 8, top: 16, width: 24, height: 32 };
    let update = mtk_update(region, WAVEFORM_MODE_GC16, UPDATE_MODE_PARTIAL, 7, true);
    assert_eq!(std::mem::size_of::<UpdateRequestMtk>(), 96);
    assert_eq!(MTK_EPDC_FLAG_USE_DITHERING_Y4, 0x4000);
    assert_eq!(update.flags, 0x4000);
    assert_eq!((update.dither_mode, update.quant_bit), (0, 0));
    assert_eq!(update.update_marker, 7);
    assert_eq!(update.update_mode, UPDATE_MODE_PARTIAL);
    assert_eq!((update.hist_bw_waveform_mode, update.hist_gray_waveform_mode), (1, 2));
    assert_eq!(mtk_update(region, WAVEFORM_MODE_DU, 0, 8, true).flags, EPDC_FLAG_FORCE_MONOCHROME);
    assert_eq!(mtk_update(region, WAVEFORM_MODE_A2_MODERN, 0, 8, true).flags, 0);
    assert_eq!(mtk_update(region, WAVEFORM_MODE_GC16, 0, 8, false).flags, 0);
}

#[test]
fn hardware_regions_expand_to_eight_pixels_and_clip_at_the_panel() {
    let region = aligned_dither_region(UpdateRect { left: 9, top: 17, width: 7, height: 8 }, 19, 27);
    assert_eq!((region.left, region.top, region.width, region.height), (8, 16, 8, 11));
    let edge = aligned_dither_region(UpdateRect { left: 18, top: 26, width: u32::MAX, height: u32::MAX }, 19, 27);
    assert_eq!((edge.left, edge.top, edge.width, edge.height), (16, 24, 3, 3));
    let outside = aligned_dither_region(UpdateRect { left: 100, top: 100, width: 2, height: 2 }, 19, 27);
    assert_eq!((outside.width, outside.height), (0, 0));
}

#[test]
fn a_raw_scene_uses_only_mtk_y4_and_preserves_input_until_the_driver_reads_it() {
    let mut framebuffer = Framebuffer::memory_for_test(19, 27, 24, 1, SENTINEL).unwrap();
    let handle = DitheringHandle::for_test(true);
    assert!(handle.set_hardware_y4(true));
    framebuffer.set_dithering(handle.clone(), false);
    framebuffer.write_line(17, 9..16, &[110; 7]);
    let region = PhysicalRect { x: 9, y: 17, width: 7, height: 8 };
    for waveform in [RefreshWaveform::Auto, RefreshWaveform::A2, RefreshWaveform::Du, RefreshWaveform::Gc16] {
        let resolved = framebuffer.resolve_dithering(ResolvedRefresh { region, waveform, flashing: false });
        assert_eq!(resolved.waveform, RefreshWaveform::Gc16);
        assert_eq!(resolved.region, PhysicalRect { x: 8, y: 16, width: 8, height: 11 });
    }
    let calls = Cell::new(0);
    let result = framebuffer.send_update_using(UpdateRect { left: 9, top: 17, width: 7, height: 8 },
        WAVEFORM_MODE_DU, UPDATE_MODE_PARTIAL,
        |driver, region, waveform, mode, marker, y4| {
            calls.set(calls.get() + 1);
            assert_eq!(driver, UpdateVariant::Mtk);
            assert_eq!((region.left, region.top, region.width, region.height), (8, 16, 8, 11));
            assert_eq!((waveform, mode, marker, y4), (WAVEFORM_MODE_GC16, UPDATE_MODE_PARTIAL, 1, true));
            assert_eq!(&framebuffer.mapped_bytes_for_test()[17 * 24 + 9..17 * 24 + 16], &[110; 7]);
            Ok(())
        }, || panic!("An accepted Y4 update needs no fallback wait"));
    assert_eq!(calls.get(), 1);
    assert!(result.accepted && result.y4_attempted && !result.software_dither_fallback);
    assert!(handle.hardware_y4_available());
    assert_eq!(framebuffer.last_submitted_marker.get(), Some(1));
}

#[test]
fn non_mtk_and_color_mappings_cannot_receive_a_y4_flag() {
    for (bytes_per_pixel, driver) in [(1, UpdateVariant::Legacy), (1, UpdateVariant::Rex),
        (1, UpdateVariant::Zelda), (4, UpdateVariant::Mtk)]
    {
        let mut framebuffer = Framebuffer::memory_for_test(4, 1, 4 * bytes_per_pixel + 3,
            bytes_per_pixel, SENTINEL).unwrap();
        framebuffer.update_variant.set(Some(driver));
        let handle = DitheringHandle::for_test(true);
        assert!(handle.set_hardware_y4(true));
        framebuffer.set_dithering(handle.clone(), false);
        assert!(!handle.hardware_y4_available());
        assert!(handle.requested());
        framebuffer.write_line(0, 0..4, &[110; 4]);
        let result = framebuffer.send_update_using(UpdateRect { left: 0, top: 0, width: 4, height: 1 },
            WAVEFORM_MODE_AUTO, UPDATE_MODE_PARTIAL,
            |actual_driver, _, waveform, _, _, y4| {
                assert_eq!(actual_driver, driver);
                assert_eq!(waveform, WAVEFORM_MODE_GC16);
                assert!(!y4);
                Ok(())
            }, || panic!("An unsupported device must not request Y4"));
        assert!(result.accepted && !result.y4_attempted);
        for x in 0..4 {
            assert_eq!(framebuffer.mapped_bytes_for_test()[x * bytes_per_pixel], software_gray(110, x, 0));
        }
    }
}

#[test]
fn explicit_rejection_waits_then_replaces_the_whole_mapping_with_a_fresh_marker() {
    for inverted in [false, true] {
        let mut framebuffer = Framebuffer::memory_for_test(11, 3, 16, 1, SENTINEL).unwrap();
        let handle = DitheringHandle::for_test(true);
        handle.set_hardware_y4(true);
        framebuffer.set_dithering(handle.clone(), false);
        framebuffer.set_output_inverted(inverted);
        for y in 0..3 { framebuffer.write_line(y, 0..11, &[110; 11]); }
        framebuffer.update_variant.set(Some(UpdateVariant::Mtk));
        assert!(framebuffer.submit_update_with(11, 3, |_, marker| {
            assert_eq!(marker, 1);
            Ok(())
        }).accepted);
        let stage = Cell::new(0);
        let result = framebuffer.send_update_using(UpdateRect { left: 3, top: 1, width: 2, height: 1 },
            WAVEFORM_MODE_GC16, UPDATE_MODE_PARTIAL,
            |driver, region, waveform, mode, marker, y4| {
                assert_eq!(driver, UpdateVariant::Mtk);
                assert_eq!(waveform, WAVEFORM_MODE_GC16);
                if y4 {
                    assert_eq!((stage.get(), marker), (0, 2));
                    stage.set(1);
                    Err(Some(libc::EINVAL))
                } else {
                    assert_eq!((stage.get(), marker, mode), (2, 3, UPDATE_MODE_FULL));
                    stage.set(3);
                    assert_eq!((region.left, region.top, region.width, region.height), (0, 0, 11, 3));
                    let mask = if inverted { 255 } else { 0 };
                    for y in 0..3 {
                        for x in 0..11 {
                            assert_eq!(framebuffer.mapped_bytes_for_test()[y * 16 + x] ^ mask,
                                software_gray(110, x, y));
                        }
                        assert!(framebuffer.mapped_bytes_for_test()[y * 16 + 11..(y + 1) * 16]
                            .iter().all(|&value| value == SENTINEL));
                    }
                    Ok(())
                }
            }, || {
                assert_eq!(stage.get(), 1);
                assert_eq!(framebuffer.last_submitted_marker.get(), Some(1));
                assert_eq!(framebuffer.mapped_bytes_for_test()[0], if inverted { 145 } else { 110 });
                stage.set(2);
                Ok(())
            });
        assert_eq!(stage.get(), 3);
        assert!(result.accepted && result.y4_attempted && result.software_dither_fallback);
        assert_eq!(result.ioctl_attempts, 2);
        assert_eq!(result.last_errno, None);
        assert_eq!(framebuffer.last_issued_marker.get(), 3);
        assert_eq!(framebuffer.last_submitted_marker.get(), Some(3));
        assert!(!handle.hardware_y4_available());
        assert!(handle.requested());
    }
}

#[test]
fn uncertain_driver_errors_never_rewrite_pixels_or_automatically_resubmit() {
    for errno in [Some(libc::EIO), Some(libc::EINTR), Some(libc::ETIMEDOUT), None] {
        let mut framebuffer = Framebuffer::memory_for_test(4, 1, 7, 1, SENTINEL).unwrap();
        let handle = DitheringHandle::for_test(true);
        handle.set_hardware_y4(true);
        framebuffer.set_dithering(handle.clone(), false);
        framebuffer.write_line(0, 0..4, &[110; 4]);
        let before = framebuffer.mapped_bytes_for_test().to_vec();
        let calls = Cell::new(0);
        let result = framebuffer.send_update_using(UpdateRect { left: 0, top: 0, width: 4, height: 1 },
            WAVEFORM_MODE_GC16, UPDATE_MODE_PARTIAL,
            |driver, _, _, _, marker, y4| {
                assert_eq!((driver, marker, y4), (UpdateVariant::Mtk, 1, true));
                calls.set(calls.get() + 1);
                Err(errno)
            }, || panic!("Uncertain delivery must not enter fallback"));
        assert_eq!(calls.get(), 1);
        assert!(!result.accepted && result.y4_attempted && !result.software_dither_fallback);
        assert!(result.dithering_delivery_uncertain);
        assert_eq!(result.last_errno, errno);
        assert_eq!(framebuffer.mapped_bytes_for_test(), before);
        assert!(handle.hardware_y4_available());
        assert_eq!(framebuffer.last_issued_marker.get(), 1);
        assert_eq!(framebuffer.last_submitted_marker.get(), None);
        assert_future_scene_is_blocked(&mut framebuffer, &handle);
    }
}

#[test]
fn a_failed_completion_barrier_preserves_raw_pixels_and_prevents_fallback_submission() {
    let mut framebuffer = Framebuffer::memory_for_test(4, 1, 7, 1, SENTINEL).unwrap();
    let handle = DitheringHandle::for_test(true);
    handle.set_hardware_y4(true);
    framebuffer.set_dithering(handle.clone(), false);
    framebuffer.write_line(0, 0..4, &[110; 4]);
    framebuffer.update_variant.set(Some(UpdateVariant::Mtk));
    assert!(framebuffer.submit_update_with(4, 1, |_, marker| {
        assert_eq!(marker, 1);
        Ok(())
    }).accepted);
    let before = framebuffer.mapped_bytes_for_test().to_vec();
    let calls = Cell::new(0);
    let result = framebuffer.send_update_using(UpdateRect { left: 0, top: 0, width: 4, height: 1 },
        WAVEFORM_MODE_GC16, UPDATE_MODE_PARTIAL,
        |_, _, _, _, _, y4| {
            assert!(y4);
            calls.set(calls.get() + 1);
            Err(Some(libc::EINVAL))
        }, || Err(std::io::Error::from_raw_os_error(libc::EIO)));
    assert_eq!(calls.get(), 1);
    assert!(!result.accepted && !result.software_dither_fallback);
    assert!(result.dithering_delivery_uncertain);
    assert_eq!(result.last_errno, Some(libc::EIO));
    assert_eq!(framebuffer.mapped_bytes_for_test(), before);
    assert!(!handle.hardware_y4_available());
    assert!(handle.requested());
    assert_eq!(framebuffer.last_submitted_marker.get(), Some(1));
    assert_future_scene_is_blocked(&mut framebuffer, &handle);
}

#[test]
fn a_rejected_mtk_abi_can_probe_other_abis_only_after_software_conversion() {
    let mut framebuffer = Framebuffer::memory_for_test(4, 1, 7, 1, SENTINEL).unwrap();
    let handle = DitheringHandle::for_test(true);
    handle.set_hardware_y4(true);
    framebuffer.set_dithering(handle, false);
    framebuffer.write_line(0, 0..4, &[110; 4]);
    let calls = Cell::new(0);
    let result = framebuffer.send_update_using(UpdateRect { left: 0, top: 0, width: 4, height: 1 },
        WAVEFORM_MODE_GC16, UPDATE_MODE_PARTIAL,
        |driver, _, _, _, marker, y4| {
            calls.set(calls.get() + 1);
            if y4 {
                assert_eq!((driver, marker), (UpdateVariant::Mtk, 1));
                Err(Some(libc::ENOTTY))
            } else {
                assert_eq!((driver, marker), (UpdateVariant::Legacy, 2));
                assert!(framebuffer.mapped_bytes_for_test()[..4].iter().all(|value| value % 17 == 0));
                Ok(())
            }
        }, || Ok(()));
    assert_eq!(calls.get(), 2);
    assert!(result.accepted && result.software_dither_fallback);
    assert_eq!(framebuffer.update_variant.get(), Some(UpdateVariant::Legacy));
    assert_eq!(framebuffer.last_submitted_marker.get(), Some(2));
}

#[test]
fn fallback_abi_probe_stops_on_uncertain_delivery_without_reusing_its_marker() {
    for errno in [Some(libc::EIO), Some(libc::EINTR), Some(libc::ETIMEDOUT), None] {
        let mut framebuffer = Framebuffer::memory_for_test(4, 1, 7, 1, SENTINEL).unwrap();
        let handle = DitheringHandle::for_test(true);
        handle.set_hardware_y4(true);
        framebuffer.set_dithering(handle.clone(), false);
        framebuffer.write_line(0, 0..4, &[110; 4]);
        let calls = Cell::new(0);
        let result = framebuffer.send_update_using(UpdateRect { left: 0, top: 0, width: 4, height: 1 },
            WAVEFORM_MODE_GC16, UPDATE_MODE_PARTIAL,
            |driver, _, _, _, marker, y4| {
                calls.set(calls.get() + 1);
                if y4 {
                    assert_eq!((driver, marker, calls.get()), (UpdateVariant::Mtk, 1, 1));
                    Err(Some(libc::ENOTTY))
                } else {
                    assert_eq!((driver, marker, calls.get()), (UpdateVariant::Legacy, 2, 2));
                    Err(errno)
                }
            }, || Ok(()));
        assert_eq!(calls.get(), 2);
        assert_eq!(result.ioctl_attempts, 2);
        assert!(!result.accepted && result.y4_attempted && result.software_dither_fallback);
        assert!(result.dithering_delivery_uncertain);
        assert_eq!(result.last_errno, errno);
        assert_eq!(framebuffer.update_variant.get(), None);
        assert_eq!(framebuffer.last_issued_marker.get(), 2);
        assert_eq!(framebuffer.last_submitted_marker.get(), None);
        assert!(!handle.hardware_y4_available());
        assert_future_scene_is_blocked(&mut framebuffer, &handle);
    }
}

fn assert_future_scene_is_blocked(framebuffer: &mut Framebuffer, handle: &DitheringHandle) {
    let before = framebuffer.mapped_bytes_for_test().to_vec();
    let marker = framebuffer.last_issued_marker.get();
    let revision = handle.revision();
    // Simulate the following event-loop iteration: a capability revision and
    // an application replacement scene can both demand another complete draw.
    handle.set_hardware_y4(false);
    handle.set_hardware_y4(true);
    assert_ne!(handle.revision(), revision);
    framebuffer.set_dithering(handle.clone(), false);
    framebuffer.write_line(0, 0..4, &[0; 4]);
    framebuffer.fill(0);
    framebuffer.software_dither_mapping();
    assert_eq!(framebuffer.mapped_bytes_for_test(), before);
    assert!(framebuffer.require_known_dithering_delivery().is_err());
    assert!(framebuffer.wait_for_pending_update_complete_checked().is_err());
    assert!(framebuffer.wait_for_update_complete_with(|_| {
        panic!("An older marker cannot acknowledge uncertain display delivery")
    }).is_err());
    let second = framebuffer.send_update_using(UpdateRect { left: 0, top: 0, width: 4, height: 1 },
        WAVEFORM_MODE_GC16, UPDATE_MODE_FULL,
        |_, _, _, _, _, _| panic!("A later iteration cannot resubmit an uncertain update"),
        || panic!("A later iteration cannot retry the fallback barrier"));
    assert!(!second.accepted && second.dithering_delivery_uncertain);
    assert_eq!(second.ioctl_attempts, 0);
    let direct = framebuffer.submit_update_with_policy(4, 1, true, |_, _| {
        panic!("The low-level submit barrier must also retain the failure")
    });
    assert!(!direct.accepted && direct.dithering_delivery_uncertain);
    assert_eq!(direct.ioctl_attempts, 0);
    assert_eq!(framebuffer.last_issued_marker.get(), marker);
}

#[test]
fn fallback_fill_and_row_writes_share_the_same_logical_palette() {
    for bytes_per_pixel in [1, 4] {
        for inverted in [false, true] {
            let stride = 11 * bytes_per_pixel + 5;
            let mut filled = Framebuffer::memory_for_test(11, 3, stride, bytes_per_pixel, SENTINEL).unwrap();
            let mut written = Framebuffer::memory_for_test(11, 3, stride, bytes_per_pixel, SENTINEL).unwrap();
            for framebuffer in [&mut filled, &mut written] {
                let handle = DitheringHandle::for_test(false);
                handle.set_hardware_y4(true);
                framebuffer.set_dithering(handle, false);
                framebuffer.set_output_inverted(inverted);
            }
            filled.fill(110);
            for y in 0..3 { written.write_line(y, 0..11, &[110; 11]); }
            assert_eq!(filled.mapped_bytes_for_test(), written.mapped_bytes_for_test());
        }
    }
}

#[test]
fn fallback_dirty_writes_keep_physical_phase_padding_and_polarity() {
    for bytes_per_pixel in [1, 4] {
        for inverted in [false, true] {
            let stride = 11 * bytes_per_pixel + 5;
            let mut whole = Framebuffer::memory_for_test(11, 3, stride, bytes_per_pixel, SENTINEL).unwrap();
            let mut dirty = Framebuffer::memory_for_test(11, 3, stride, bytes_per_pixel, SENTINEL).unwrap();
            for framebuffer in [&mut whole, &mut dirty] {
                let handle = DitheringHandle::for_test(false);
                handle.set_hardware_y4(true);
                framebuffer.set_dithering(handle, false);
                framebuffer.set_output_inverted(inverted);
            }
            for y in 0..3 {
                let source = (0..11).map(|x| (y * 37 + x * 23) as u8).collect::<Vec<_>>();
                whole.write_line(y, 0..11, &source);
                for span in [0..2, 2..7, 7..11, 2..7] {
                    dirty.write_line(y, span.clone(), &source[span]);
                }
            }
            assert_eq!(whole.mapped_bytes_for_test(), dirty.mapped_bytes_for_test());
            for row in dirty.mapped_bytes_for_test().chunks_exact(stride) {
                assert!(row[11 * bytes_per_pixel..].iter().all(|&value| value == SENTINEL));
            }
        }
    }
}

#[test]
fn bilevel_mode_keeps_its_original_fast_path_without_y4() {
    let mut framebuffer = Framebuffer::memory_for_test(4, 1, 7, 1, SENTINEL).unwrap();
    let handle = DitheringHandle::for_test(true);
    handle.set_hardware_y4(true);
    framebuffer.set_dithering(handle.clone(), true);
    let refresh = ResolvedRefresh { region: PhysicalRect { x: 1, y: 0, width: 2, height: 1 },
        waveform: RefreshWaveform::Du, flashing: false };
    assert_eq!(framebuffer.resolve_dithering(refresh), refresh);
    let result = framebuffer.send_update_using(UpdateRect { left: 1, top: 0, width: 2, height: 1 },
        WAVEFORM_MODE_DU, UPDATE_MODE_PARTIAL,
        |_, _, waveform, _, _, y4| {
            assert_eq!(waveform, WAVEFORM_MODE_DU);
            assert!(!y4);
            Ok(())
        }, || panic!("Bilevel mode must not request Y4"));
    assert!(result.accepted && !result.y4_attempted);
    assert!(handle.hardware_y4_available());
}

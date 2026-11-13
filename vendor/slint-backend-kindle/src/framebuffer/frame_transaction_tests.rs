use std::cell::RefCell;

use super::{Framebuffer, RefreshSubmission, UpdateVariant};
use super::ffi::{UpdateRect, UPDATE_MODE_PARTIAL, WAVEFORM_MODE_AUTO};

#[test]
fn a_replacement_settles_writes_submits_and_completes_in_order_for_every_abi() {
    for driver in UpdateVariant::PROBE_ORDER {
        let mut framebuffer = Framebuffer::memory_for_test(2, 1, 4, 1, 0x7f).unwrap();
        framebuffer.update_variant.set(Some(driver));
        let preceding = framebuffer.submit_update_with(2, 1, |_, _| Ok(()));
        assert_eq!(preceding.update_marker, Some(1));
        let events = RefCell::new(Vec::new());
        let replacement = framebuffer.write_after_pending_update_with(
            |framebuffer| framebuffer.wait_for_update_complete_with(|record| {
                assert_eq!(framebuffer.mapped_bytes_for_test(), &[0x7f; 4]);
                events.borrow_mut().push(format!("wait:{}", record.update_marker));
                Ok(())
            }),
            |framebuffer| {
                framebuffer.write_line(0, 0..2, &[0x00, 0xff]);
                events.borrow_mut().push("write".into());
                framebuffer.submit_update_with(2, 1, |variant, marker| {
                    assert_eq!(variant, driver);
                    events.borrow_mut().push(format!("submit:{marker}"));
                    Ok(())
                })
            },
        ).unwrap();
        assert_eq!(replacement.update_marker, Some(2));
        framebuffer.complete_replacement_with(&replacement,
            |framebuffer| framebuffer.wait_for_update_complete_with(|record| {
                assert_eq!(framebuffer.mapped_bytes_for_test(), &[0x00, 0xff, 0x7f, 0x7f]);
                events.borrow_mut().push(format!("wait:{}", record.update_marker));
                Ok(())
            }),
        ).unwrap();
        assert_eq!(*events.borrow(), ["wait:1", "write", "submit:2", "wait:2"]);
        assert_eq!(framebuffer.last_accepted_update_marker(), Some(2));
        assert_eq!(framebuffer.last_completed_update_marker(), Some(2));
    }
}

#[test]
fn a_failed_preceding_wait_preserves_pixels_and_blocks_later_mapping_changes() {
    let mut framebuffer = Framebuffer::memory_for_test(2, 1, 4, 1, 0x7f).unwrap();
    framebuffer.update_variant.set(Some(UpdateVariant::Mtk));
    let preceding = framebuffer.submit_update_with(2, 1, |_, _| Ok(()));
    let result = framebuffer.write_after_pending_update_with(
        |framebuffer| framebuffer.wait_for_update_complete_with(|record| {
            assert_eq!(record.update_marker, 1);
            Err(std::io::Error::from_raw_os_error(libc::EIO))
        }),
        |_| panic!("a failed completion barrier must not enter the writer"),
    );
    assert_eq!(result.unwrap_err().raw_os_error(), Some(libc::EIO));
    framebuffer.write_line(0, 0..2, &[0x00, 0xff]);
    framebuffer.fill(0x00);
    assert_eq!(framebuffer.mapped_bytes_for_test(), &[0x7f; 4]);
    assert!(!framebuffer.submit_update_with(2, 1,
        |_, _| panic!("a failed barrier must not submit another update")).accepted);
    assert!(framebuffer.complete_replacement_with(&preceding,
        |_| panic!("a failed barrier must not acknowledge an older frame")).is_err());
    assert_eq!(framebuffer.last_accepted_update_marker(), Some(1));
    assert_eq!(framebuffer.last_completed_update_marker(), None);
}

#[test]
fn a_rejected_replacement_cannot_complete_by_waiting_for_the_preceding_frame() {
    let mut framebuffer = Framebuffer::memory_for_test(1, 1, 1, 1, 0xff).unwrap();
    framebuffer.update_variant.set(Some(UpdateVariant::Mtk));
    assert!(framebuffer.submit_update_with(1, 1, |_, _| Ok(())).accepted);
    let replacement = framebuffer.write_after_pending_update_with(
        |framebuffer| framebuffer.wait_for_update_complete_with(|record| {
            assert_eq!(record.update_marker, 1);
            Ok(())
        }),
        |framebuffer| {
            framebuffer.write_line(0, 0..1, &[0x00]);
            framebuffer.submit_update_with(1, 1, |_, marker| {
                assert_eq!(marker, 2);
                Err(Some(libc::EINVAL))
            })
        },
    ).unwrap();
    assert!(!replacement.accepted);
    assert_eq!(replacement.update_marker, None);
    let result = framebuffer.complete_replacement_with(&replacement,
        |_| panic!("a rejected replacement must not reuse preceding completion"));
    assert_eq!(result.unwrap_err().raw_os_error(), Some(libc::EINVAL));
    assert_eq!(framebuffer.last_accepted_update_marker(), Some(1));
    assert_eq!(framebuffer.last_completed_update_marker(), Some(1));
    framebuffer.fill(0xff);
    assert_eq!(framebuffer.mapped_bytes_for_test(), &[0x00]);
}

#[test]
fn a_failed_replacement_completion_blocks_future_pixels_and_submissions() {
    let mut framebuffer = Framebuffer::memory_for_test(1, 1, 1, 1, 0xff).unwrap();
    framebuffer.update_variant.set(Some(UpdateVariant::Mtk));
    let replacement = framebuffer.write_after_pending_update_with(
        |_| panic!("the first frame has no preceding update to wait for"),
        |framebuffer| {
            framebuffer.write_line(0, 0..1, &[0x00]);
            framebuffer.submit_update_with(1, 1, |_, _| Ok(()))
        },
    ).unwrap();
    let result = framebuffer.complete_replacement_with(&replacement,
        |framebuffer| framebuffer.wait_for_update_complete_with(|record| {
            assert_eq!(record.update_marker, 1);
            Err(std::io::Error::from_raw_os_error(libc::EINTR))
        }));
    assert_eq!(result.unwrap_err().raw_os_error(), Some(libc::EINTR));
    assert!(framebuffer.write_after_pending_update_with(
        |_| panic!("a failed strict transaction cannot retry a completion wait"),
        |_| panic!("a failed strict transaction cannot write another frame"),
    ).is_err());
    assert!(!framebuffer.submit_update_with(1, 1,
        |_, _| panic!("a failed strict transaction cannot submit another frame")).accepted);
    framebuffer.fill(0xff);
    assert_eq!(framebuffer.mapped_bytes_for_test(), &[0x00]);
    assert_eq!(framebuffer.last_completed_update_marker(), None);
}

#[test]
fn a_stale_or_missing_accepted_marker_cannot_establish_replacement_completion() {
    for marker in [None, Some(0), Some(1)] {
        let framebuffer = Framebuffer::memory_for_test(1, 1, 1, 1, 0xff).unwrap();
        framebuffer.update_variant.set(Some(UpdateVariant::Mtk));
        assert!(framebuffer.submit_update_with(1, 1, |_, _| Ok(())).accepted);
        assert!(framebuffer.submit_update_with(1, 1, |_, _| Ok(())).accepted);
        let stale = RefreshSubmission { accepted: true, update_marker: marker, ..Default::default() };
        assert!(framebuffer.complete_replacement_with(&stale,
            |_| panic!("only the current accepted marker can be completed")).is_err());
        assert_eq!(framebuffer.last_completed_update_marker(), None);
    }
}

#[test]
fn ordinary_scene_uncertain_errors_stop_probing_and_block_future_mapping_changes() {
    for cached in [false, true] {
        for errno in [Some(libc::EIO), Some(libc::EINTR), Some(libc::ETIMEDOUT), None] {
            let mut framebuffer = Framebuffer::memory_for_test(2, 1, 4, 1, 0x7f).unwrap();
            let preceding = if cached {
                framebuffer.update_variant.set(Some(UpdateVariant::Mtk));
                assert!(framebuffer.submit_update_with(2, 1, |_, _| Ok(())).accepted);
                framebuffer.wait_for_update_complete_with(|_| Ok(())).unwrap();
                Some(1)
            } else { None };
            let calls = RefCell::new(Vec::new());
            let result = framebuffer.send_update_using(
                UpdateRect { left: 0, top: 0, width: 2, height: 1 },
                WAVEFORM_MODE_AUTO, UPDATE_MODE_PARTIAL,
                |driver, _, waveform, _, marker, y4| {
                    assert!(!y4);
                    assert_eq!(waveform, WAVEFORM_MODE_AUTO);
                    calls.borrow_mut().push((driver, marker));
                    Err(errno)
                },
                || panic!("an ordinary uncertain result must not enter Y4 fallback"),
            );
            let driver = if cached { UpdateVariant::Mtk } else { UpdateVariant::Legacy };
            assert_eq!(*calls.borrow(), [(driver, if cached { 2 } else { 1 })]);
            assert!(!result.accepted && result.dithering_delivery_uncertain);
            assert_eq!(result.update_marker, None);
            assert_eq!(result.last_errno, errno);
            assert_eq!(framebuffer.last_accepted_update_marker(), preceding);
            framebuffer.write_line(0, 0..2, &[0x00, 0xff]);
            framebuffer.fill(0x00);
            assert_eq!(framebuffer.mapped_bytes_for_test(), &[0x7f; 4]);
            assert!(!framebuffer.send_update_using(
                UpdateRect { left: 0, top: 0, width: 2, height: 1 },
                WAVEFORM_MODE_AUTO, UPDATE_MODE_PARTIAL,
                |_, _, _, _, _, _| panic!("uncertain delivery must prevent another submission"),
                || panic!("uncertain delivery must prevent a fallback wait"),
            ).accepted);
        }
    }
}

#[test]
fn ordinary_scene_explicit_rejections_keep_probe_order_and_one_accepted_marker() {
    let errors = [libc::EINVAL, libc::ENOTTY, libc::EOPNOTSUPP];
    for accepted in 1..UpdateVariant::PROBE_ORDER.len() {
        let framebuffer = Framebuffer::memory_for_test(2, 1, 4, 1, 0x7f).unwrap();
        let calls = RefCell::new(Vec::new());
        let result = framebuffer.send_update_using(
            UpdateRect { left: 0, top: 0, width: 2, height: 1 },
            WAVEFORM_MODE_AUTO, UPDATE_MODE_PARTIAL,
            |driver, _, waveform, _, marker, y4| {
                assert!(!y4);
                assert_eq!((waveform, marker), (WAVEFORM_MODE_AUTO, 1));
                calls.borrow_mut().push(driver);
                if driver == UpdateVariant::PROBE_ORDER[accepted] { Ok(()) }
                else { Err(Some(errors[calls.borrow().len() - 1])) }
            },
            || panic!("ordinary explicit ABI rejection must not enter Y4 fallback"),
        );
        assert_eq!(*calls.borrow(), UpdateVariant::PROBE_ORDER[..=accepted]);
        assert!(result.accepted && !result.dithering_delivery_uncertain);
        assert_eq!(result.update_marker, Some(1));
        assert_eq!(result.ioctl_attempts, accepted as u32 + 1);
        assert_eq!(framebuffer.last_accepted_update_marker(), Some(1));
        assert_eq!(framebuffer.last_completed_update_marker(), None);
        assert!(framebuffer.require_known_dithering_delivery().is_ok());
    }
}

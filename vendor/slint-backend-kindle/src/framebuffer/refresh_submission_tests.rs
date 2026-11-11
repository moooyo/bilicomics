use super::{Framebuffer, RefreshSubmission, UpdateVariant, refresh_submission_with};
use crate::refresh::{PhysicalRect, RefreshWaveform, ResolvedRefresh};

#[test]
fn first_probe_stops_at_the_first_accepted_abi_and_clears_prior_errno() {
    for accepted in 0..UpdateVariant::PROBE_ORDER.len() {
        let mut calls = Vec::new();
        let (selected, submission) = refresh_submission_with(None, 1860, 2480, |variant| {
            calls.push(variant);
            if calls.len() == accepted + 1 { Ok(()) } else { Err(Some(libc::EINVAL)) }
        });
        assert_eq!(calls, UpdateVariant::PROBE_ORDER[..=accepted]);
        assert_eq!(selected, Some(UpdateVariant::PROBE_ORDER[accepted]));
        assert_eq!(submission, RefreshSubmission {
            ioctl_attempts: (accepted + 1) as u32,
            accepted: true,
            last_errno: None,
        });
    }
}

#[test]
fn a_failed_initial_probe_records_the_last_errno_and_caches_unsupported() {
    let mut calls = Vec::new();
    let errors = [libc::EINVAL, libc::ENOTTY, libc::EINVAL, libc::EIO];
    let (selected, submission) = refresh_submission_with(None, 1860, 2480, |variant| {
        calls.push(variant);
        Err(Some(errors[calls.len() - 1]))
    });
    assert_eq!(calls, UpdateVariant::PROBE_ORDER);
    assert_eq!(selected, Some(UpdateVariant::Unsupported));
    assert_eq!(submission, RefreshSubmission {
        ioctl_attempts: 4,
        accepted: false,
        last_errno: Some(libc::EIO),
    });
    let (retained, next) = refresh_submission_with(selected, 1860, 2480, |_| {
        panic!("A cached unsupported driver must not issue another ioctl")
    });
    assert_eq!(retained, selected);
    assert_eq!(next, RefreshSubmission::default());
}

#[test]
fn a_cached_abi_has_exactly_one_attempt_on_success_or_failure() {
    for variant in UpdateVariant::PROBE_ORDER {
        for result in [Ok(()), Err(Some(libc::ENOTTY)), Err(None)] {
            let mut calls = Vec::new();
            let (selected, submission) = refresh_submission_with(Some(variant), 1860, 2480, |requested| {
                calls.push(requested);
                result
            });
            assert_eq!(calls, [variant]);
            assert_eq!(selected, Some(variant));
            assert_eq!(submission, RefreshSubmission {
                ioctl_attempts: 1,
                accepted: result.is_ok(),
                last_errno: result.err().flatten(),
            });
        }
    }
}

#[test]
fn empty_regions_never_probe_or_change_the_cached_abi() {
    for cached in [None, Some(UpdateVariant::Legacy), Some(UpdateVariant::Unsupported)] {
        for (width, height) in [(0, 2480), (1860, 0), (0, 0)] {
            let (selected, submission) = refresh_submission_with(cached, width, height, |_| {
                panic!("An empty region must not issue an ioctl")
            });
            assert_eq!(selected, cached);
            assert_eq!(submission, RefreshSubmission::default());
        }
    }
}

#[test]
fn empty_public_refreshes_preserve_an_unprobed_framebuffer() {
    let framebuffer = Framebuffer::memory_for_test(1, 1, 1, 1, 0xff).unwrap();
    for (width, height) in [(0, 1), (1, 0), (0, 0)] {
        let region = PhysicalRect { x: 0, y: 0, width, height };
        assert_eq!(framebuffer.refresh_region(
            slint::PhysicalPosition::new(0, 0),
            slint::PhysicalSize::new(width, height),
        ), RefreshSubmission::default());
        for waveform in [RefreshWaveform::Auto, RefreshWaveform::Gc16] {
            for flashing in [false, true] {
                assert_eq!(framebuffer.refresh_planned(ResolvedRefresh { region, waveform, flashing }),
                    RefreshSubmission::default());
                assert_eq!(framebuffer.update_variant.get(), None);
            }
        }
    }
}

#[test]
fn checked_completion_waits_for_the_latest_frame_for_every_driver() {
    for driver in UpdateVariant::PROBE_ORDER {
        let framebuffer = Framebuffer::memory_for_test(1, 1, 1, 1, 0xff).unwrap();
        framebuffer.update_variant.set(Some(driver));
        let mut submitted = Vec::new();
        for _ in 0..2 {
            assert!(framebuffer.submit_update_with(1, 1, |variant, marker| {
                assert_eq!(variant, driver);
                submitted.push(marker);
                Ok(())
            }).accepted);
        }
        assert_eq!(submitted, [1, 2]);
        let stale_completion = submitted[0];
        let outcome = framebuffer.wait_for_update_complete_with(|record| {
            assert_eq!(record.update_marker, submitted[1]);
            if record.update_marker == stale_completion { Ok(()) }
            else { Err(std::io::Error::other("the new frame is not complete")) }
        });
        assert!(outcome.is_err());
        assert_eq!(framebuffer.last_completed_marker.get(), None);
        framebuffer.wait_for_update_complete_with(|record| {
            assert_eq!(record.update_marker, submitted[1]);
            Ok(())
        }).unwrap();
        assert_eq!(framebuffer.last_completed_marker.get(), Some(submitted[1]));
    }
}

#[test]
fn failed_submissions_consume_their_identity_and_preserve_the_preceding_update() {
    let framebuffer = Framebuffer::memory_for_test(1, 1, 1, 1, 0xff).unwrap();
    framebuffer.update_variant.set(Some(UpdateVariant::Mtk));
    assert!(framebuffer.submit_update_with(1, 1, |_, marker| {
        assert_eq!(marker, 1);
        Ok(())
    }).accepted);
    assert!(!framebuffer.submit_update_with(1, 1, |_, marker| {
        assert_eq!(marker, 2);
        Err(Some(libc::EIO))
    }).accepted);
    assert_eq!(framebuffer.last_submitted_marker.get(), Some(1));
    framebuffer.wait_for_update_complete_with(|record| {
        assert_eq!(record.update_marker, 1);
        Ok(())
    }).unwrap();
    assert!(framebuffer.submit_update_with(1, 1, |_, marker| {
        assert_eq!(marker, 3);
        Ok(())
    }).accepted);
    framebuffer.wait_for_update_complete_with(|record| {
        assert_eq!(record.update_marker, 3);
        Ok(())
    }).unwrap();
}

#[test]
fn first_probe_shares_one_frame_identity_until_acceptance_then_advances() {
    let framebuffer = Framebuffer::memory_for_test(1, 1, 1, 1, 0xff).unwrap();
    let mut attempted = Vec::new();
    let submission = framebuffer.submit_update_with(1, 1, |driver, marker| {
        attempted.push((driver, marker));
        assert_eq!(framebuffer.last_submitted_marker.get(), None);
        if driver == UpdateVariant::Zelda { Ok(()) } else { Err(Some(libc::EINVAL)) }
    });
    assert!(submission.accepted);
    assert_eq!(attempted, [(UpdateVariant::Legacy, 1), (UpdateVariant::Rex, 1), (UpdateVariant::Zelda, 1)]);
    assert_eq!(framebuffer.last_submitted_marker.get(), Some(1));
    assert!(framebuffer.submit_update_with(1, 1, |driver, marker| {
        assert_eq!((driver, marker), (UpdateVariant::Zelda, 2));
        Ok(())
    }).accepted);
}

#[test]
fn missing_submission_or_marker_exhaustion_cannot_manufacture_completion() {
    let framebuffer = Framebuffer::memory_for_test(1, 1, 1, 1, 0xff).unwrap();
    assert!(framebuffer.wait_for_update_complete_with(|_| panic!("No frame was submitted")).is_err());
    framebuffer.wait_for_pending_update_complete_checked().unwrap();
    framebuffer.update_variant.set(Some(UpdateVariant::Mtk));
    framebuffer.last_issued_marker.set(u32::MAX - 1);
    assert!(framebuffer.submit_update_with(1, 1, |_, marker| {
        assert_eq!(marker, u32::MAX);
        Ok(())
    }).accepted);
    let exhausted = framebuffer.submit_update_with(1, 1, |_, _| panic!("Markers cannot wrap"));
    assert!(!exhausted.accepted);
    assert_eq!(exhausted.last_errno, Some(libc::EOVERFLOW));
    assert!(framebuffer.refresh_full_checked().is_err());
    assert_eq!(framebuffer.last_submitted_marker.get(), Some(u32::MAX));
    framebuffer.wait_for_update_complete_with(|record| {
        assert_eq!(record.update_marker, u32::MAX);
        Ok(())
    }).unwrap();
}

#[test]
fn interrupted_wait_is_retryable_and_new_updates_invalidate_cached_completion() {
    let framebuffer = Framebuffer::memory_for_test(1, 1, 1, 1, 0xff).unwrap();
    framebuffer.update_variant.set(Some(UpdateVariant::Mtk));
    assert!(framebuffer.submit_update_with(1, 1, |_, _| Ok(())).accepted);
    assert!(framebuffer.wait_for_update_complete_with(|_| {
        Err(std::io::Error::from_raw_os_error(libc::EINTR))
    }).is_err());
    assert_eq!(framebuffer.last_completed_marker.get(), None);
    framebuffer.wait_for_update_complete_with(|record| {
        assert_eq!(record.update_marker, 1);
        Ok(())
    }).unwrap();
    framebuffer.wait_for_update_complete_with(|_| panic!("An observed completion is reusable")).unwrap();
    assert!(framebuffer.submit_update_with(1, 1, |_, _| Ok(())).accepted);
    framebuffer.wait_for_update_complete_with(|record| {
        assert_eq!(record.update_marker, 2);
        Ok(())
    }).unwrap();
}

#[test]
fn checked_public_submission_and_wait_report_driver_errors() {
    let framebuffer = Framebuffer::memory_for_test(1, 1, 1, 1, 0xff).unwrap();
    assert!(framebuffer.refresh_full_checked().is_err());
    assert_eq!(framebuffer.last_submitted_marker.get(), None);
    framebuffer.last_submitted_marker.set(Some(7));
    assert!(framebuffer.wait_for_update_complete_checked().is_err());
    assert_eq!(framebuffer.last_completed_marker.get(), None);
    framebuffer.wait_for_update_complete();
}


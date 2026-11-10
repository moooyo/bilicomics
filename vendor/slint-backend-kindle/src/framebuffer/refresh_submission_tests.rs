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


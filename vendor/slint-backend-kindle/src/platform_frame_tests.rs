//! Production frame transactions exercised with anonymous mappings and pipe descriptors.

use std::cell::Cell;
use std::sync::{Arc, Mutex, mpsc};

use crate::first_frame_signal::{FirstFrameNotice, FirstFrameSignalError, FirstFrameSignalHandle};
use crate::framebuffer::{Framebuffer, RefreshSubmission};
use crate::performance::{PerformanceFrameSample, PerformanceSampleKind, PerformanceTraceHandle, Recording};
use crate::refresh::{PendingRefresh, RefreshArea, RefreshRequest, RefreshWaveform};
use super::{DrawOrder, StartupDraw, render_with_settled_mapping};

fn begin_recording() -> (Option<Recording>, Arc<Mutex<Vec<PerformanceFrameSample>>>) {
    let handle = PerformanceTraceHandle::default();
    let samples = Arc::new(Mutex::new(Vec::new()));
    let output = samples.clone();
    handle.set_callback(Arc::new(move |sample| output.lock().unwrap().push(sample)));
    (handle.begin(PerformanceSampleKind::Frame), samples)
}

fn finish_recording(trace: &mut Option<Recording>, samples: &Arc<Mutex<Vec<PerformanceFrameSample>>>)
    -> PerformanceFrameSample
{
    trace.take().unwrap().finish();
    let mut samples = samples.lock().unwrap();
    assert_eq!(samples.len(), 1);
    samples.pop().unwrap()
}

#[test]
fn private_shelf_render_requires_only_full_gc16_and_an_outstanding_known_marker() {
    let mut pending = PendingRefresh::default();
    let framebuffer = Framebuffer::memory_for_test(1, 1, 1, 1, 0x55).unwrap();
    let scene = StartupDraw { current: true, ..StartupDraw::default() };
    framebuffer.fake_accept_update_for_test();
    assert!(!pending.bind_initial_full());
    assert_eq!(scene.order(&pending, &framebuffer), DrawOrder::WaitFirst);
    pending.test_push(RefreshRequest::full());
    assert!(pending.bind_initial_full());
    pending.test_push(RefreshRequest::full());
    assert_eq!(scene.order(&pending, &framebuffer), DrawOrder::WaitFirst);
    assert!(pending.bind_initial_full());
    assert_eq!(scene.order(&pending, &framebuffer), DrawOrder::PrivateRenderFirst);
    assert_eq!(StartupDraw { initial: true, ..scene }.order(&pending, &framebuffer), DrawOrder::InitialFill);
    for protected in [
        StartupDraw { current: false, ..scene }, StartupDraw { lifecycle_forced: true, ..scene },
        StartupDraw { rotation_changed: true, ..scene }, StartupDraw { dithering_changed: true, ..scene },
        StartupDraw { hardware_y4: true, ..scene }, StartupDraw { bilevel: true, ..scene },
    ] {
        assert_eq!(protected.order(&pending, &framebuffer), DrawOrder::WaitFirst);
    }
    for waveform in [RefreshWaveform::Auto, RefreshWaveform::Du, RefreshWaveform::A2] {
        let mut mixed = PendingRefresh::default();
        mixed.test_push(RefreshRequest::full());
        mixed.test_push(RefreshRequest { waveform, flashing: false, area: RefreshArea::Dirty });
        assert!(!mixed.bind_initial_full());
        assert_eq!(scene.order(&mixed, &framebuffer), DrawOrder::WaitFirst);
    }
    let mut overflow = PendingRefresh::default();
    for _ in 0..64 { overflow.test_push(RefreshRequest::full()); }
    assert!(!overflow.bind_initial_full());
    assert_eq!(scene.order(&overflow, &framebuffer), DrawOrder::WaitFirst);
    framebuffer.fake_complete_latest_update_for_test().unwrap();
    assert_eq!(scene.order(&pending, &framebuffer), DrawOrder::WaitFirst);
    let framebuffer = Framebuffer::memory_for_test(1, 1, 1, 1, 0x55).unwrap();
    assert_eq!(scene.order(&pending, &framebuffer), DrawOrder::WaitFirst);
    let accepted = framebuffer.fake_accept_update_for_test();
    assert!(framebuffer.complete_replacement_checked(&accepted).is_err());
    assert_eq!(scene.order(&pending, &framebuffer), DrawOrder::WaitFirst);
}

#[test]
fn failed_preceding_wait_never_writes_or_submits_the_prepared_shelf() {
    for order in [DrawOrder::PrivateRenderFirst, DrawOrder::InitialFill, DrawOrder::WaitFirst] {
        let mut framebuffer = Framebuffer::memory_for_test(2, 1, 3, 1, 0x55).unwrap();
        framebuffer.fake_accept_update_for_test();
        let (mut trace, samples) = begin_recording();
        let mut pixels = [slint::Rgb8Pixel::default(); 2];
        let rendered = Cell::new(false);
        let result = render_with_settled_mapping(order, &mut framebuffer, &mut pixels, &mut trace,
            |pixels, trace| {
                assert!(trace.as_ref().unwrap().sample.stages.preceding_update_wait.is_none());
                rendered.set(true);
                pixels[0].r = 0xff;
            }, |_, _, _, _| panic!("unconfirmed previous marker must block mapping writes"));
        assert!(result.is_err());
        assert_eq!(rendered.get(), order == DrawOrder::PrivateRenderFirst);
        assert_eq!(pixels[0].r, if rendered.get() { 0xff } else { 0 });
        assert_eq!(framebuffer.mapped_bytes_for_test(), &[0x55; 3]);
        assert_eq!(framebuffer.last_accepted_update_marker(), Some(1));
        let sample = finish_recording(&mut trace, &samples);
        assert_eq!(sample.completion_errno, Some(libc::ENOTTY));
        assert_eq!(sample.accepted_update_marker, None);
        assert!(sample.stages.framebuffer_write.is_none() && sample.stages.refresh_submit.is_none());
    }
}

#[test]
fn private_render_changes_only_rgb_order_and_still_requires_both_markers() {
    for order in [DrawOrder::WaitFirst, DrawOrder::PrivateRenderFirst] {
        let mut framebuffer = Framebuffer::memory_for_test(2, 1, 3, 1, 0x55).unwrap();
        framebuffer.fake_accept_update_for_test();
        framebuffer.fake_complete_latest_update_for_test().unwrap();
        let (mut trace, samples) = begin_recording();
        let mut pixels = [slint::Rgb8Pixel::default(); 2];
        render_with_settled_mapping(order, &mut framebuffer, &mut pixels, &mut trace, |pixels, trace| {
            assert_eq!(trace.as_ref().unwrap().sample.stages.preceding_update_wait.is_some(), order == DrawOrder::WaitFirst);
            pixels[1].r = 0x31;
            1
        }, |framebuffer, pixels, index: usize, trace| {
            assert!(trace.as_ref().unwrap().sample.stages.preceding_update_wait.is_some());
            assert_eq!(framebuffer.last_completed_update_marker(), Some(1));
            framebuffer.write_line(0, index..index + 1, &[pixels[index].r]);
        }).unwrap();
        assert_eq!(framebuffer.mapped_bytes_for_test(), &[0x55, 0x31, 0x55]);
        let submission = framebuffer.fake_accept_update_for_test();
        assert_eq!(submission.update_marker, Some(2));
        assert!(super::complete_frame_update(&framebuffer, &submission, &mut trace).is_err());
        let sample = finish_recording(&mut trace, &samples);
        assert!(!sample.driver_completion_observed);
        assert_eq!(framebuffer.last_completed_update_marker(), Some(1));
    }
}

#[test]
fn a_failed_preceding_wait_records_its_marker_and_never_enters_the_writer() {
    let mut framebuffer = Framebuffer::memory_for_test(3, 2, 7, 1, 0x55).unwrap();
    let preceding = framebuffer.fake_accept_update_for_test();
    assert!(preceding.accepted);
    assert_eq!(preceding.update_marker, Some(1));
    let before = framebuffer.mapped_bytes_for_test().to_vec();
    let entered_writer = Cell::new(false);
    let (mut trace, samples) = begin_recording();

    let result = super::with_settled_mapping(&mut framebuffer, &mut trace, |framebuffer, _| {
        entered_writer.set(true);
        framebuffer.fill(0);
    });

    assert!(result.is_err());
    assert!(!entered_writer.get());
    assert_eq!(framebuffer.mapped_bytes_for_test(), before);
    let sample = finish_recording(&mut trace, &samples);
    assert_eq!(sample.preceding_update_marker, Some(1));
    assert_eq!(sample.accepted_update_marker, None);
    assert_eq!(sample.completed_update_marker, None);
    assert!(sample.stages.preceding_update_wait.is_some());
    assert!(sample.stages.refresh_complete_wait.is_none());
    assert_eq!(sample.completion_errno, Some(libc::ENOTTY));
    assert!(!sample.driver_completion_observed);
    assert!(!sample.panel_completion_observed);
}

#[test]
fn a_completed_replacement_records_its_own_driver_receipt_without_claiming_panel_observation() {
    let mut framebuffer = Framebuffer::memory_for_test(3, 2, 7, 1, 0x55).unwrap();
    let preceding = framebuffer.fake_accept_update_for_test();
    assert_eq!(preceding.update_marker, Some(1));
    framebuffer.fake_complete_latest_update_for_test().unwrap();
    let (mut trace, samples) = begin_recording();

    let replacement = super::with_settled_mapping(&mut framebuffer, &mut trace, |framebuffer, trace| {
        let sample = &trace.as_ref().unwrap().sample;
        assert_eq!(sample.preceding_update_marker, Some(1));
        assert_eq!(sample.completed_update_marker, Some(1));
        assert!(sample.stages.preceding_update_wait.is_some());
        framebuffer.fill(0x22);
        framebuffer.fake_accept_update_for_test()
    }).unwrap();
    assert!(replacement.accepted);
    assert_eq!(replacement.update_marker, Some(2));
    assert_eq!(framebuffer.mapped_bytes_for_test(),
        &[0x22, 0x22, 0x22, 0x55, 0x55, 0x55, 0x55,
          0x22, 0x22, 0x22, 0x55, 0x55, 0x55, 0x55]);
    let sample = &mut trace.as_mut().unwrap().sample;
    sample.complete_frame_replacement = true;
    sample.refresh_accepted = Some(replacement.accepted);
    sample.accepted_update_marker = replacement.update_marker;
    framebuffer.fake_complete_latest_update_for_test().unwrap();

    super::complete_frame_update(&framebuffer, &replacement, &mut trace).unwrap();

    let sample = finish_recording(&mut trace, &samples);
    assert!(sample.complete_frame_replacement);
    assert_eq!(sample.preceding_update_marker, Some(1));
    assert_eq!(sample.accepted_update_marker, Some(2));
    assert_eq!(sample.completed_update_marker, Some(2));
    assert_eq!(sample.refresh_accepted, Some(true));
    assert!(sample.stages.preceding_update_wait.is_some());
    assert!(sample.stages.refresh_complete_wait.is_some());
    assert_eq!(sample.completion_errno, None);
    assert!(sample.driver_completion_observed);
    assert!(!sample.panel_completion_observed);
}

#[test]
fn a_rejected_replacement_cannot_reuse_the_preceding_frames_completion() {
    let mut framebuffer = Framebuffer::memory_for_test(3, 2, 7, 1, 0x55).unwrap();
    let preceding = framebuffer.fake_accept_update_for_test();
    assert_eq!(preceding.update_marker, Some(1));
    framebuffer.fake_complete_latest_update_for_test().unwrap();
    let (mut trace, samples) = begin_recording();
    super::with_settled_mapping(&mut framebuffer, &mut trace, |framebuffer, _| {
        framebuffer.fill(0x22);
    }).unwrap();
    let rejected = RefreshSubmission {
        ioctl_attempts: 1, last_errno: Some(libc::EIO), ..Default::default()
    };
    let sample = &mut trace.as_mut().unwrap().sample;
    sample.complete_frame_replacement = true;
    sample.refresh_accepted = Some(rejected.accepted);
    sample.accepted_update_marker = rejected.update_marker;

    let result = super::complete_frame_update(&framebuffer, &rejected, &mut trace);

    assert!(result.is_err());
    let sample = finish_recording(&mut trace, &samples);
    assert_eq!(sample.preceding_update_marker, Some(1));
    assert_eq!(sample.accepted_update_marker, None);
    assert_eq!(sample.completed_update_marker, Some(1));
    assert_eq!(sample.refresh_accepted, Some(false));
    assert!(sample.stages.refresh_complete_wait.is_some());
    assert_eq!(sample.completion_errno, Some(libc::EIO));
    assert!(!sample.driver_completion_observed);
    assert!(!sample.panel_completion_observed);
}

fn initial_frame() -> super::FirstFrameSubmission {
    super::FirstFrameSubmission {
        initial: true, rendered: true, full: true, active: true, sleep_pending: false,
    }
}

fn armed_first_frame_signal() -> (FirstFrameSignalHandle, mpsc::Receiver<FirstFrameNotice>) {
    let signal = FirstFrameSignalHandle::default();
    let (sender, receiver) = mpsc::channel();
    signal.arm(sender).unwrap();
    (signal, receiver)
}

#[test]
fn the_initial_send_signals_after_writes_before_its_checked_completion_even_without_tracing() {
    let mut framebuffer = Framebuffer::memory_for_test(2, 1, 4, 1, 0x7f).unwrap();
    let (signal, receiver) = armed_first_frame_signal();
    let mut trace = None;
    let submission = super::with_settled_mapping(&mut framebuffer, &mut trace, |framebuffer, _| {
        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
        framebuffer.write_line(0, 0..2, &[0x00, 0xff]);
        let submission = framebuffer.fake_accept_update_for_test();
        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
        submission
    }).unwrap();
    assert_eq!(framebuffer.mapped_bytes_for_test(), &[0x00, 0xff, 0x7f, 0x7f]);
    assert_eq!(framebuffer.last_accepted_update_marker(), Some(1));
    assert_eq!(framebuffer.last_completed_update_marker(), None);

    let result = super::finish_frame_submission(&framebuffer, &submission, &mut trace,
        &signal, initial_frame(), true);
    // The real completion ioctl on the pipe fails. The signal has already
    // admitted pure background work, but completion must still fail closed.
    assert!(result.unwrap_err().to_string().contains("did not complete"));
    assert_eq!(receiver.try_recv(), Ok(FirstFrameNotice::default()));
    assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    assert!(trace.is_none());
    assert_eq!(framebuffer.last_completed_update_marker(), None);
    assert_eq!(framebuffer.require_known_dithering_delivery().unwrap_err().raw_os_error(),
        Some(libc::ENOTTY));
    framebuffer.fill(0x55);
    assert_eq!(framebuffer.mapped_bytes_for_test(), &[0x00, 0xff, 0x7f, 0x7f]);
    let (late, _) = mpsc::channel();
    assert_eq!(signal.arm(late), Err(FirstFrameSignalError::Retired));
}

#[test]
fn the_notice_reuses_the_existing_submit_clock_when_profiling() {
    let framebuffer = Framebuffer::memory_for_test(1, 1, 1, 1, 0xff).unwrap();
    let submission = framebuffer.fake_accept_update_for_test();
    let (signal, receiver) = armed_first_frame_signal();
    let (mut trace, _samples) = begin_recording();
    trace.as_mut().unwrap().sample.stages.refresh_submit = Some(crate::PerformanceTiming {
        finished_monotonic_ns: Some(412), ..Default::default()
    });
    super::finish_frame_submission(&framebuffer, &submission, &mut trace,
        &signal, initial_frame(), false).unwrap();
    assert_eq!(receiver.try_recv(), Ok(FirstFrameNotice { accepted_monotonic_ns: Some(412) }));
    assert_eq!(framebuffer.last_completed_update_marker(), None);
}

#[test]
fn an_accepted_current_marker_with_failed_delivery_cannot_admit_background_work() {
    let framebuffer = Framebuffer::memory_for_test(1, 1, 1, 1, 0xff).unwrap();
    let submission = framebuffer.fake_accept_update_for_test();
    assert!(super::complete_frame_update(&framebuffer, &submission, &mut None).is_err());
    assert_eq!(submission.update_marker, framebuffer.last_accepted_update_marker());
    let (signal, receiver) = armed_first_frame_signal();

    assert!(super::finish_frame_submission(&framebuffer, &submission, &mut None,
        &signal, initial_frame(), false).is_err());
    assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
    signal.retire();
    assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Disconnected));
}

#[test]
fn rejected_uncertain_and_stale_submissions_do_not_consume_first_frame_admission() {
    for rejected_marker in [None, Some(0), Some(1)] {
        let framebuffer = Framebuffer::memory_for_test(1, 1, 1, 1, 0xff).unwrap();
        framebuffer.fake_accept_update_for_test();
        framebuffer.fake_complete_latest_update_for_test().unwrap();
        let accepted = framebuffer.fake_accept_update_for_test();
        assert_eq!(accepted.update_marker, Some(2));
        let (signal, receiver) = armed_first_frame_signal();
        let malformed = RefreshSubmission { update_marker: rejected_marker, ..accepted };
        let rejected = RefreshSubmission { accepted: false, update_marker: None,
            last_errno: Some(libc::EINVAL), ..accepted };
        let uncertain = RefreshSubmission { dithering_delivery_uncertain: true, ..accepted };
        for submission in [malformed, rejected, uncertain] {
            super::finish_frame_submission(&framebuffer, &submission, &mut None,
                &signal, initial_frame(), false).unwrap();
            assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
        }
        super::finish_frame_submission(&framebuffer, &accepted, &mut None,
            &signal, initial_frame(), false).unwrap();
        assert_eq!(receiver.try_recv(), Ok(FirstFrameNotice::default()));
        assert_eq!(framebuffer.last_completed_update_marker(), Some(1));
    }
}

#[test]
fn refresh_only_partial_inactive_sleep_and_later_frames_do_not_consume_admission() {
    let eligible = initial_frame();
    for frame in [
        super::FirstFrameSubmission { rendered: false, ..eligible },
        super::FirstFrameSubmission { full: false, ..eligible },
        super::FirstFrameSubmission { active: false, ..eligible },
        super::FirstFrameSubmission { sleep_pending: true, ..eligible },
        super::FirstFrameSubmission { initial: false, ..eligible },
    ] {
        let framebuffer = Framebuffer::memory_for_test(1, 1, 1, 1, 0xff).unwrap();
        let submission = framebuffer.fake_accept_update_for_test();
        let (signal, receiver) = armed_first_frame_signal();
        super::finish_frame_submission(&framebuffer, &submission, &mut None,
            &signal, frame, false).unwrap();
        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
        super::finish_frame_submission(&framebuffer, &submission, &mut None,
            &signal, eligible, false).unwrap();
        assert_eq!(receiver.try_recv(), Ok(FirstFrameNotice::default()));
        assert_eq!(framebuffer.last_completed_update_marker(), None);
    }
}

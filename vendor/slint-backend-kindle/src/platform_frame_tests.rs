//! Production frame transactions exercised with anonymous mappings and pipe descriptors.

use std::cell::Cell;
use std::sync::{Arc, Mutex};

use crate::framebuffer::{Framebuffer, RefreshSubmission};
use crate::performance::{PerformanceFrameSample, PerformanceSampleKind, PerformanceTraceHandle, Recording};

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

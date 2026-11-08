use super::*;
use slint::platform::software_renderer::RepaintBufferType;

fn trace_handle() -> (DisplayLifecycleHandle, crate::wakeup::Wakeup) {
    let wake = crate::wakeup::make_wakeup().unwrap();
    let handle = DisplayLifecycleHandle::new(
        MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer),
        Arc::new(Mutex::new(PendingRefresh::default())),
        wake.write.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    (handle, wake)
}

#[test]
fn a_default_resume_never_invokes_the_optional_trace_writer() {
    let (handle, _wake) = trace_handle();
    handle.suspend().unwrap();
    handle.resume().unwrap();
    assert!(!handle.inner.resume_frame_pending.get());
    handle.complete_full_redraw_with_trace(|_, _| panic!("tracing is disabled by default")).unwrap();
    assert!(!handle.needs_full_redraw());
    assert!(!handle.is_suspended());
}

#[test]
fn only_the_first_full_frame_of_a_successful_resume_emits_current_output_state() {
    let (handle, _wake) = trace_handle();
    let path = PathBuf::from("diagnostics/resume.jsonl");
    handle.set_resume_trace_path(Some(path.clone())).unwrap();
    assert!(handle.inner.framebuffer.borrow().is_none());
    assert!(handle.inner.state.input.borrow().is_none());
    handle.suspend().unwrap();
    handle.resume().unwrap();
    handle.resume().unwrap();
    assert!(handle.inner.resume_frame_pending.get());
    handle.set_output_inverted(true).unwrap();
    let records = RefCell::new(Vec::new());
    handle.complete_full_redraw_with_trace(|destination, inverted| {
        records.borrow_mut().push((destination.to_owned(), inverted));
        Ok(())
    }).unwrap();
    assert_eq!(&*records.borrow(), &[(path, true)]);
    assert!(!handle.inner.resume_frame_pending.get());
    handle.resume().unwrap();
    handle.complete_full_redraw_with_trace(|_, _| panic!("active resume cannot rearm a receipt")).unwrap();
}

#[test]
fn failed_input_reacquisition_does_not_arm_a_rendered_frame_receipt() {
    let (handle, _wake) = trace_handle();
    handle.set_resume_trace_path(Some(PathBuf::from("resume.jsonl"))).unwrap();
    handle.suspend().unwrap();
    assert!(handle.resume_with_input(|| Err(failure("injected input acquisition failure"))).is_err());
    assert!(handle.is_suspended());
    assert!(!handle.inner.resume_frame_pending.get());
    assert!(handle.inner.state.input.borrow().is_none());
    handle.resume().unwrap();
    assert!(handle.inner.resume_frame_pending.get());
}

#[test]
fn another_sleep_cancels_the_unrendered_receipt_before_a_later_resume() {
    let (handle, _wake) = trace_handle();
    handle.set_resume_trace_path(Some(PathBuf::from("resume.jsonl"))).unwrap();
    handle.suspend().unwrap();
    handle.resume().unwrap();
    assert!(handle.inner.resume_frame_pending.get());
    handle.suspend().unwrap();
    assert!(!handle.inner.resume_frame_pending.get());
    assert!(handle.inner.resume_trace_path.borrow().is_some());
    handle.resume().unwrap();
    let calls = Cell::new(0);
    handle.complete_full_redraw_with_trace(|_, _| {
        calls.set(calls.get() + 1);
        Ok(())
    }).unwrap();
    assert_eq!(calls.get(), 1);
}

#[test]
fn disabling_or_changing_the_trace_path_does_not_relabel_a_pending_frame() {
    let (handle, _wake) = trace_handle();
    handle.set_resume_trace_path(Some(PathBuf::from("old.jsonl"))).unwrap();
    handle.suspend().unwrap();
    handle.resume().unwrap();
    handle.set_resume_trace_path(None).unwrap();
    handle.complete_full_redraw_with_trace(|_, _| panic!("the disabled receipt must be canceled")).unwrap();
    handle.suspend().unwrap();
    handle.set_resume_trace_path(Some(PathBuf::from("old.jsonl"))).unwrap();
    handle.resume().unwrap();
    let path = PathBuf::from("new.jsonl");
    handle.set_resume_trace_path(Some(path.clone())).unwrap();
    handle.complete_full_redraw_with_trace(|_, _| panic!("a new path applies to a later resume")).unwrap();
    handle.suspend().unwrap();
    handle.resume().unwrap();
    handle.complete_full_redraw_with_trace(|destination, _| {
        assert_eq!(destination, path.as_path());
        Ok(())
    }).unwrap();
}

#[test]
fn a_trace_writer_error_cannot_fail_redraw_or_retry_on_an_unrelated_frame() {
    let (handle, _wake) = trace_handle();
    handle.set_resume_trace_path(Some(PathBuf::from("resume.jsonl"))).unwrap();
    handle.suspend().unwrap();
    handle.resume().unwrap();
    handle.complete_full_redraw_with_trace(|_, _| {
        Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "injected trace error"))
    }).unwrap();
    assert!(!handle.is_suspended());
    assert!(!handle.needs_full_redraw());
    assert!(!handle.inner.resume_frame_pending.get());
    handle.complete_full_redraw_with_trace(|_, _| panic!("a failed receipt must not be retried")).unwrap();
}

#[test]
fn quit_clears_the_diagnostic_path_and_rejects_both_optional_public_apis() {
    let (handle, _wake) = trace_handle();
    handle.set_resume_trace_path(Some(PathBuf::from("resume.jsonl"))).unwrap();
    handle.suspend().unwrap();
    handle.resume().unwrap();
    handle.inner.quit.store(true, Ordering::SeqCst);
    handle.complete_full_redraw_with_trace(|_, _| panic!("a requested quit cancels the receipt before cleanup")).unwrap();
    handle.release_event_loop_io();
    assert!(handle.is_suspended());
    assert!(!handle.inner.resume_frame_pending.get());
    assert!(handle.inner.resume_trace_path.borrow().is_none());
    assert!(handle.set_resume_trace_path(None).is_err());
    assert!(handle.set_resume_trace_path(Some(PathBuf::from("late.jsonl"))).is_err());
    // The quit guard rejects this before its real read-only probe can run.
    let error = handle.probe_output_inversion().unwrap_err();
    assert!(error.to_string().contains("terminated"));
    handle.complete_full_redraw_with_trace(|_, _| panic!("quit must cancel an unrendered receipt")).unwrap();
}

//! Controlled live-presentation tests; no native device is opened.

use super::*;
use NativePortraitOrientation::{Down, Up};
use slint::{ComponentHandle, Rgb8Pixel};
use slint::platform::software_renderer::{RenderingRotation, RepaintBufferType};
use std::{fs::File, io::Write, os::fd::{AsRawFd, FromRawFd}};

fn handle() -> (DisplayLifecycleHandle, crate::wakeup::Wakeup) {
    with_window(MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer))
}

fn with_window(window: Rc<MinimalSoftwareWindow>) -> (DisplayLifecycleHandle, crate::wakeup::Wakeup) {
    let wake = crate::wakeup::make_wakeup().unwrap();
    let handle = DisplayLifecycleHandle::new(window, Arc::new(Mutex::new(PendingRefresh::default())),
        wake.write.clone(), Arc::new(AtomicBool::new(false)));
    (handle, wake)
}

fn activate(handle: &DisplayLifecycleHandle, width: u32, height: u32, stride: usize) {
    handle.set_native_framebuffer_portrait_calibration(Up, Up, 3).unwrap();
    handle.set_output_inverted(true).unwrap();
    assert_eq!(handle.ensure_framebuffer_with(|| {
        Framebuffer::memory_for_test(width, height, stride, 1, 0x5a)
    }).unwrap(), Some((width, height)));
    handle.complete_full_redraw().unwrap();
}

fn observation(rotation: u32) -> NativeFramebufferObservation {
    NativeFramebufferObservation { rotation, output_inverted: Some(false) }
}

fn observer_pipe(
    initial: Option<NativePortraitOrientation>,
    current: Rc<Cell<i32>>,
) -> (NativePortraitObserver, File) {
    let mut descriptors = [0; 2];
    // SAFETY: The array holds both descriptors from this private nonblocking pipe.
    assert_eq!(unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) }, 0);
    // SAFETY: Successful pipe2 returned two distinct, owned descriptors.
    let read = unsafe { File::from_raw_fd(descriptors[0]) };
    let write = unsafe { File::from_raw_fd(descriptors[1]) };
    (NativePortraitObserver::from_test_file(read, initial, move |_| Ok(current.get())), write)
}

fn queue_source_frame(write: &mut File, raw: i32) {
    // Linux evdev timestamps use two native unsigned longs. This producer is
    // independent of the observer parser and emits one complete ABS/SYN frame.
    let header = std::mem::size_of::<libc::c_ulong>() * 2;
    for (kind, code, value) in [(3u16, 0x18u16, raw), (0, 0, 0)] {
        let mut bytes = vec![0u8; header + 8];
        bytes[header..header + 2].copy_from_slice(&kind.to_ne_bytes());
        bytes[header + 2..header + 4].copy_from_slice(&code.to_ne_bytes());
        bytes[header + 4..header + 8].copy_from_slice(&value.to_ne_bytes());
        write.write_all(&bytes).unwrap();
    }
}

#[test]
fn only_explicit_calibration_opens_an_observer_after_active_geometry() {
    let (handle, _) = handle();
    handle.set_native_framebuffer_portrait_orientation(Up).unwrap();
    handle.ensure_portrait_observer_with(|| panic!("geometry is not ready"), |_| unreachable!()).unwrap();
    handle.ensure_framebuffer_with(|| Framebuffer::memory_for_test(3, 2, 5, 1, 0x5a)).unwrap();
    handle.complete_full_redraw().unwrap();
    handle.ensure_portrait_observer_with(|| panic!("legacy setters do not opt into sensor I/O"), |_| unreachable!()).unwrap();
    assert!(handle.set_native_framebuffer_portrait_calibration(Up, Up, 3).is_err());
    assert_eq!(handle.inner.portrait_calibration.get(), None);
    handle.suspend().unwrap();
    assert!(handle.set_native_framebuffer_portrait_calibration(Up, Up, 4).is_err());
    handle.set_native_framebuffer_portrait_calibration(Up, Up, 3).unwrap();
    handle.ensure_portrait_observer_with(|| panic!("sleep must not open the sensor"), |_| unreachable!()).unwrap();
    assert_eq!(handle.portrait_poll_snapshot().0, -1);
    handle.resume_with_input(|| Ok(None)).unwrap();
    let opened = Cell::new(0);
    handle.ensure_portrait_observer_with(|| { opened.set(opened.get() + 1); Ok(None) }, |_| unreachable!()).unwrap();
    handle.ensure_portrait_observer_with(|| panic!("an absent sensor is attempted only once"), |_| unreachable!()).unwrap();
    assert_eq!(opened.get(), 1);
}

#[test]
fn live_scanout_uses_fresh_zero_or_half_turn_and_same_values_are_a_noop() {
    let (handle, wake) = handle();
    activate(&handle, 3, 2, 5);
    crate::wakeup::drain(&wake.read);
    assert!(handle.apply_live_portrait_with(Down, || Ok(observation(3))).unwrap());
    assert_eq!(handle.native_render_offset(), Some(180));
    assert_eq!(handle.native_touch_offset(), 180);
    assert_eq!(handle.inner.portrait_calibration.get(), Some(PortraitCalibration { scanout: Up, raw_rotation: 3 }));
    handle.complete_full_redraw().unwrap();
    assert!(handle.apply_live_portrait_with(Down, || Ok(observation(1))).unwrap());
    assert_eq!(handle.native_render_offset(), Some(0));
    assert_eq!(handle.inner.portrait_calibration.get(), Some(PortraitCalibration { scanout: Down, raw_rotation: 1 }));
    handle.complete_full_redraw().unwrap();
    crate::wakeup::drain(&wake.read);
    let generation = handle.input_generation();
    let sampled = Cell::new(false);
    assert!(!handle.apply_live_portrait_with(Down, || { sampled.set(true); Ok(observation(1)) }).unwrap());
    assert!(sampled.get());
    assert_eq!(handle.input_generation(), generation);
    assert!(!handle.needs_full_redraw());
    assert!(handle.inner.output_inverted.get());
}

#[test]
fn rejected_live_rotation_layout_and_borrows_keep_the_previous_presentation() {
    let (handle, _) = handle();
    activate(&handle, 3, 2, 5);
    let anchor = handle.inner.portrait_calibration.get();
    let generation = handle.input_generation();
    let bytes = handle.inner.framebuffer.borrow().as_ref().unwrap().mapped_bytes_for_test().to_vec();
    for raw in [0, 2, 4, u32::MAX] {
        assert!(handle.apply_live_portrait_with(Down, || Ok(observation(raw))).is_err());
        assert_eq!(handle.inner.portrait_calibration.get(), anchor);
        assert_eq!(handle.native_render_offset(), Some(0));
        assert_eq!(handle.native_touch_offset(), 0);
        assert_eq!(handle.input_generation(), generation);
        assert!(!handle.needs_full_redraw());
    }
    assert!(handle.apply_live_portrait_with(Down, || {
        handle.probe_native_framebuffer_with(|geometry, layout| {
            assert_eq!(geometry, Some((3, 2)));
            assert_eq!(layout.unwrap().stride, 5);
            Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "fixture layout changed"))
        })
    }).is_err());
    let input_borrow = handle.inner.state.input.borrow_mut();
    assert!(handle.apply_live_portrait_with(Down, || Ok(observation(3))).is_err());
    drop(input_borrow);
    let transition = handle.inner.state.enter_transition().unwrap();
    assert!(handle.apply_live_portrait_with(Down, || Ok(observation(3))).is_err());
    drop(transition);
    assert_eq!(handle.inner.portrait_calibration.get(), anchor);
    assert_eq!(handle.inner.native_portrait_orientation.get(), Some(Up));
    assert_eq!(handle.inner.framebuffer.borrow().as_ref().unwrap().mapped_bytes_for_test(), bytes);
    assert_eq!(handle.input_generation(), generation);
    assert!(!handle.needs_full_redraw());
}

#[test]
fn active_pressed_contact_is_cancelled_drained_and_replaced_by_a_fresh_gesture() {
    use crate::touch::tests::{portrait_input, portrait_window, queue_portrait_contact};

    let (window, component) = portrait_window();
    let (handle, _) = with_window(window.clone());
    activate(&handle, 200, 200, 205);
    let mapping = handle.inner.framebuffer.borrow().as_ref().unwrap().mapped_bytes_for_test().as_ptr();
    let (input, write) = portrait_input();
    let input_fd = input.fd();
    *handle.inner.state.input.borrow_mut() = Some(input);
    queue_portrait_contact(&write, Some((20, 20)), 1);
    handle.poll_touch(0);
    assert!(component.get_top_left_pressed());
    assert_eq!(component.get_top_left_count(), 0);
    let old_generation = handle.input_generation();
    queue_portrait_contact(&write, None, -1);
    queue_portrait_contact(&write, Some((80, 80)), 2);
    queue_portrait_contact(&write, None, -1);
    assert!(handle.apply_live_portrait_with(Down, || Ok(observation(3))).unwrap());
    assert_eq!(component.get_cancel_count(), 1);
    assert!(!component.get_top_left_pressed());
    assert_eq!(component.get_top_left_count(), 0);
    assert_eq!(handle.touch_poll_snapshot().0, input_fd);
    assert_ne!(handle.input_generation(), old_generation);
    assert_eq!(handle.inner.framebuffer.borrow().as_ref().unwrap().mapped_bytes_for_test().as_ptr(), mapping);
    assert!(handle.inner.output_inverted.get());
    queue_portrait_contact(&write, Some((80, 80)), 3);
    handle.poll_touch(handle.native_touch_offset());
    assert!(!component.get_top_left_pressed());
    let mut pixels = vec![Rgb8Pixel::default(); 200 * 200];
    window.draw_if_needed(|renderer| {
        renderer.set_rendering_rotation(RenderingRotation::Rotate180);
        renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
        renderer.set_repaint_buffer_type(RepaintBufferType::ReusedBuffer);
        let _ = renderer.render(&mut pixels, 200);
    });
    assert_eq!(pixels[160 * 200 + 160].r, 17);
    handle.complete_full_redraw().unwrap();
    assert_eq!(component.get_top_left_count(), 0);
    assert!(!component.get_top_left_pressed());
    // The second drain retained current axes, but never replayed the old press.
    queue_portrait_contact(&write, None, 4);
    handle.poll_touch(handle.native_touch_offset());
    assert!(component.get_top_left_pressed());
    queue_portrait_contact(&write, None, -1);
    handle.poll_touch(handle.native_touch_offset());
    assert_eq!(component.get_top_left_count(), 1);
    assert_eq!(component.get_bottom_right_count(), 0);
    component.hide().unwrap();
}

#[test]
fn observer_generation_retires_old_fds_and_does_not_replay_a_resumed_source() {
    let (handle, _) = handle();
    activate(&handle, 3, 2, 5);
    let (observer, write) = observer_pipe(None, Rc::new(Cell::new(16)));
    let fd = observer.fd();
    handle.ensure_portrait_observer_with(|| Ok(Some(observer)), |_| unreachable!()).unwrap();
    let old_generation = handle.portrait_poll_snapshot().1;
    handle.suspend().unwrap();
    assert_eq!(handle.portrait_poll_snapshot().0, -1);
    assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, -1);
    handle.ensure_portrait_observer_with(|| panic!("sleep cannot reopen a source"), |_| unreachable!()).unwrap();
    handle.resume_with_input(|| Ok(None)).unwrap();
    drop(write);
    let (observer, mut write) = observer_pipe(None, Rc::new(Cell::new(16)));
    handle.ensure_portrait_observer_with(|| Ok(Some(observer)), |_| unreachable!()).unwrap();
    let new_generation = handle.portrait_poll_snapshot().1;
    assert_ne!(old_generation, new_generation);
    queue_source_frame(&mut write, 16);
    assert!(!handle.poll_portrait_with(old_generation, |_| panic!("stale poll cannot consume a new source")).unwrap());
    assert!(handle.poll_portrait_with(new_generation, |holding| {
        handle.apply_live_portrait_with(holding, || Ok(observation(3)))
    }).unwrap());
    assert_eq!(handle.inner.native_portrait_orientation.get(), Some(Down));
    let live_fd = handle.portrait_poll_snapshot().0;
    handle.inner.quit.store(true, Ordering::SeqCst);
    handle.ensure_portrait_observer().unwrap();
    assert_eq!(handle.portrait_poll_snapshot().0, -1);
    assert_eq!(unsafe { libc::fcntl(live_fd, libc::F_GETFD) }, -1);
    handle.release_event_loop_io();
    assert!(handle.inner.portrait_observer.borrow().is_none());
}

#[test]
fn observer_open_and_sample_errors_disable_only_the_source_until_resume() {
    let (handle, _) = handle();
    activate(&handle, 3, 2, 5);
    let opened = Cell::new(0);
    handle.ensure_portrait_observer_with(|| {
        opened.set(opened.get() + 1);
        Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "fixture source unavailable"))
    }, |_| unreachable!()).unwrap();
    handle.ensure_portrait_observer_with(|| panic!("idle cannot retry a failed open"), |_| unreachable!()).unwrap();
    assert_eq!(opened.get(), 1);
    assert!(!handle.needs_full_redraw());
    handle.suspend().unwrap();
    handle.resume_with_input(|| Ok(None)).unwrap();
    handle.complete_full_redraw().unwrap();
    let current = Rc::new(Cell::new(0));
    let (observer, mut write) = observer_pipe(Some(Up), current);
    handle.ensure_portrait_observer_with(|| Ok(Some(observer)), |holding| {
        assert_eq!(holding, Up); Ok(false)
    }).unwrap();
    let generation = handle.portrait_poll_snapshot().1;
    let input_generation = handle.input_generation();
    queue_source_frame(&mut write, 16);
    assert!(!handle.poll_portrait_with(generation, |_| panic!("an unknown current pose cannot update the scene")).unwrap());
    assert!(handle.portrait_poll_snapshot().0 >= 0);
    assert_eq!(handle.portrait_poll_snapshot().1, generation);
    assert!(!handle.needs_full_redraw());
    drop(write);
    assert!(handle.poll_portrait_with(generation, |_| panic!("a closed source cannot update the scene")).is_err());
    assert_eq!(handle.portrait_poll_snapshot().0, -1);
    assert!(!handle.is_suspended());
    assert_eq!(handle.inner.native_portrait_orientation.get(), Some(Up));
    assert_eq!(handle.native_render_offset(), Some(0));
    assert_eq!(handle.input_generation(), input_generation);
    assert!(!handle.needs_full_redraw());
    handle.ensure_portrait_observer_with(|| panic!("sample failure also disables reopening"), |_| unreachable!()).unwrap();
}

#[test]
fn submitted_live_frame_waits_for_a_report_without_clicking_or_redrawing() {
    use crate::touch::tests::{
        drain_portrait_snapshot, portrait_input, portrait_window,
        queue_portrait_contact, queue_portrait_sync,
    };

    let (window, component) = portrait_window();
    let (handle, _) = with_window(window.clone());
    activate(&handle, 200, 200, 205);
    let (input, write) = portrait_input();
    *handle.inner.state.input.borrow_mut() = Some(input);
    queue_portrait_contact(&write, Some((20, 20)), 1);
    handle.poll_touch(0);
    assert!(component.get_top_left_pressed());
    queue_portrait_sync(&write, 3);
    assert!(handle.apply_live_portrait_with(Down, || Ok(observation(3))).unwrap());
    assert_eq!(component.get_cancel_count(), 1);
    assert!(!handle.live_input_drain_waiting());
    let submitted_frames = Cell::new(0);
    let mut pixels = vec![Rgb8Pixel::default(); 200 * 200];
    window.draw_if_needed(|renderer| {
        submitted_frames.set(submitted_frames.get() + 1);
        renderer.set_rendering_rotation(RenderingRotation::Rotate180);
        renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
        renderer.set_repaint_buffer_type(RepaintBufferType::ReusedBuffer);
        let _ = renderer.render(&mut pixels, 200);
    });
    handle.complete_full_redraw().unwrap();
    assert!(handle.live_input_drain_waiting());
    assert!(handle.needs_full_redraw());
    assert!(handle.inner.live_portrait_repaint_pending.get());
    handle.poll_touch(180);
    assert_eq!(component.get_top_left_count(), 0);
    // This represents a readiness retry. A missing report still retains the
    // submitted frame and gate, rather than requesting a second render.
    handle.finish_full_redraw_input_with(|input, live| {
        assert!(live); drain_portrait_snapshot(input, (80, 80))
    }).unwrap();
    assert!(handle.live_input_drain_waiting());
    assert_eq!(submitted_frames.get(), 1);
    queue_portrait_sync(&write, 0);
    handle.finish_full_redraw_input_with(|input, live| {
        assert!(live); drain_portrait_snapshot(input, (80, 80))
    }).unwrap();
    assert!(!handle.live_input_drain_waiting());
    assert!(!handle.needs_full_redraw());
    assert_eq!(submitted_frames.get(), 1);
    assert_eq!(component.get_top_left_count(), 0);
    queue_portrait_contact(&write, None, 2);
    handle.poll_touch(180);
    assert!(component.get_top_left_pressed());
    queue_portrait_contact(&write, None, -1);
    handle.poll_touch(180);
    assert_eq!(component.get_top_left_count(), 1);
    assert_eq!(component.get_bottom_right_count(), 0);
    component.hide().unwrap();
}

#[test]
fn new_frames_cancel_input_waiting_and_non_readiness_errors_still_fail() {
    let (window, component) = crate::touch::tests::portrait_window();
    let (handle, _) = with_window(window);
    activate(&handle, 200, 200, 205);
    let (input, _write) = crate::touch::tests::portrait_input();
    *handle.inner.state.input.borrow_mut() = Some(input);
    assert!(handle.apply_live_portrait_with(Down, || Ok(observation(3))).unwrap());
    handle.finish_full_redraw_input_with(|_, _| {
        Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "fixture report missing"))
    }).unwrap();
    assert!(handle.live_input_drain_waiting());
    assert!(handle.apply_live_portrait_with(Up, || Ok(observation(3))).unwrap());
    assert!(!handle.live_input_drain_waiting());
    assert!(handle.needs_full_redraw());
    handle.finish_full_redraw_input_with(|_, _| {
        Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "fixture report missing"))
    }).unwrap();
    handle.set_output_inverted(false).unwrap();
    assert!(!handle.live_input_drain_waiting());
    handle.finish_full_redraw_input_with(|_, _| {
        Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "fixture report missing"))
    }).unwrap();
    assert!(handle.finish_full_redraw_input_with(|_, _| {
        Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "fixture input closed"))
    }).is_err());
    assert!(handle.needs_full_redraw());
    handle.suspend().unwrap();
    assert!(!handle.live_input_drain_waiting());
    handle.resume_with_input(|| Ok(None)).unwrap();
    assert!(!handle.live_input_drain_waiting());
    component.hide().unwrap();
}

#[test]
fn optional_live_trace_rejects_special_files_without_failing_the_transition() {
    let directory = std::env::temp_dir().join(format!("bilicomics-live-trace-{}-{}",
        std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&directory).unwrap();
    let target = directory.join("target.jsonl");
    std::fs::write(&target, b"retained\n").unwrap();
    let link = directory.join("link.jsonl");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let fifo = directory.join("trace.fifo");
    let fifo_c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
    for path in [link.clone(), fifo.clone()] {
        let (handle, _) = handle();
        activate(&handle, 3, 2, 5);
        handle.set_resume_trace_path(Some(path)).unwrap();
        assert!(handle.apply_live_portrait_with(Down, || Ok(observation(3))).unwrap());
        assert_eq!(handle.native_touch_offset(), 180);
    }
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "retained\n");
    std::fs::remove_file(link).unwrap();
    std::fs::remove_file(fifo).unwrap();
    std::fs::remove_file(target).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

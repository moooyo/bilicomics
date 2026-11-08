use super::*;
use slint::{ComponentHandle, Rgb8Pixel};
use slint::platform::software_renderer::{RepaintBufferType, RenderingRotation};
use std::os::fd::AsRawFd;

fn handle() -> (DisplayLifecycleHandle, crate::wakeup::Wakeup) {
    handle_with_window(MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer))
}

fn handle_with_window(
    window: Rc<MinimalSoftwareWindow>,
) -> (DisplayLifecycleHandle, crate::wakeup::Wakeup) {
    let wake = crate::wakeup::make_wakeup().unwrap();
    let handle = DisplayLifecycleHandle::new(
        window,
        Arc::new(Mutex::new(PendingRefresh::default())),
        wake.write.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    (handle, wake)
}

fn take_wake_bytes(wake: &crate::wakeup::Wakeup) -> usize {
    let mut total = 0;
    loop {
        let mut bytes = [0u8; 32];
        // SAFETY: The fixture's ordinary nonblocking pipe and buffer remain valid.
        let count = unsafe {
            libc::read(
                wake.read.as_raw_fd(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
            )
        };
        if count > 0 {
            total += count as usize;
        } else {
            assert_eq!(count, -1);
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::WouldBlock
            );
            return total;
        }
    }
}

fn install_memory_framebuffer(handle: &DisplayLifecycleHandle) {
    assert_eq!(
        handle
            .ensure_framebuffer_with(|| Framebuffer::memory_for_test(3, 2, 5, 1, 0x5a))
            .unwrap(),
        Some((3, 2)),
    );
}

#[test]
fn a_trusted_portrait_direction_is_staged_without_initial_native_io() {
    let (handle, wake) = handle();
    handle.complete_full_redraw().unwrap();
    assert_eq!(handle.native_render_offset(), None);
    handle
        .set_native_portrait_orientation(NativePortraitOrientation::Down)
        .unwrap();
    assert_eq!(handle.native_render_offset(), Some(180));
    assert!(handle.needs_full_redraw());
    assert!(handle.inner.framebuffer.borrow().is_none());
    assert_eq!(handle.inner.geometry.get(), None);
    assert!(handle.inner.state.input.borrow().is_none());
    assert_eq!(handle.input_generation(), 0);
    assert_eq!(take_wake_bytes(&wake), 1);
}

#[test]
fn initially_sleeping_direction_changes_keep_all_native_io_deferred() {
    let (handle, wake) = handle();
    handle.suspend().unwrap();
    let input_generation = handle.input_generation();
    take_wake_bytes(&wake);
    handle
        .set_native_portrait_orientation(NativePortraitOrientation::Up)
        .unwrap();
    assert!(handle.is_suspended());
    assert_eq!(handle.native_render_offset(), Some(0));
    assert_eq!(handle.ensure_active_io().unwrap(), None);
    assert_eq!(
        handle
            .ensure_framebuffer_with(|| panic!(
                "a sleeping direction update must not map the display"
            ))
            .unwrap(),
        None
    );
    assert!(handle.inner.framebuffer.borrow().is_none());
    assert!(handle.inner.state.input.borrow().is_none());
    assert_eq!(handle.touch_poll_snapshot(), (-1, input_generation));
    assert_eq!(take_wake_bytes(&wake), 1);

    handle
        .set_native_framebuffer_portrait_presentation(
            NativePortraitOrientation::Up,
            NativePortraitOrientation::Down,
        )
        .unwrap();
    assert_eq!(handle.native_render_offset(), Some(180));
    assert_eq!(handle.native_touch_offset(), 0);
    assert_eq!(handle.ensure_active_io().unwrap(), None);
    assert!(handle.inner.framebuffer.borrow().is_none());
    assert_eq!(handle.touch_poll_snapshot(), (-1, input_generation));
    assert_eq!(take_wake_bytes(&wake), 1);
}

#[test]
fn wake_direction_changes_reuse_geometry_and_require_a_complete_rewrite() {
    let (handle, wake) = handle();
    handle
        .set_native_portrait_orientation(NativePortraitOrientation::Up)
        .unwrap();
    install_memory_framebuffer(&handle);
    handle.complete_full_redraw().unwrap();
    take_wake_bytes(&wake);
    let mapping = handle
        .inner
        .framebuffer
        .borrow()
        .as_ref()
        .unwrap()
        .mapped_bytes_for_test()
        .as_ptr();
    let before = handle
        .inner
        .framebuffer
        .borrow()
        .as_ref()
        .unwrap()
        .mapped_bytes_for_test()
        .to_vec();

    for (orientation, expected_offset) in [
        (NativePortraitOrientation::Down, 180),
        (NativePortraitOrientation::Up, 0),
    ] {
        drop(
            handle
                .inner
                .state
                .suspend_with(&handle.inner.pending, |_| {
                    panic!("the retained memory fixture has no native input")
                })
                .unwrap(),
        );
        let input_generation = handle.input_generation();
        take_wake_bytes(&wake);
        handle.set_native_portrait_orientation(orientation).unwrap();
        assert!(handle.is_suspended());
        assert_eq!(handle.native_render_offset(), Some(expected_offset));
        assert_eq!(handle.inner.geometry.get(), Some((3, 2)));
        assert_eq!(handle.input_generation(), input_generation);
        assert!(handle.inner.state.input.borrow().is_none());
        assert_eq!(
            handle
                .inner
                .framebuffer
                .borrow()
                .as_ref()
                .unwrap()
                .mapped_bytes_for_test(),
            before
        );
        assert_eq!(take_wake_bytes(&wake), 1);
        drop(
            handle
                .inner
                .state
                .resume_with(&handle.inner.pending, || Ok(None))
                .unwrap(),
        );
        assert!(handle.needs_full_redraw());
        assert_eq!(
            handle
                .ensure_framebuffer_with(|| panic!("wake must reuse the retained mapping"))
                .unwrap(),
            Some((3, 2))
        );
        assert_eq!(
            handle
                .inner
                .framebuffer
                .borrow()
                .as_ref()
                .unwrap()
                .mapped_bytes_for_test()
                .as_ptr(),
            mapping
        );
        handle.complete_full_redraw().unwrap();
        assert!(!handle.needs_full_redraw());
    }
}

#[test]
fn unchanged_direction_is_a_noop_and_active_changes_preserve_the_previous_state() {
    let (handle, wake) = handle();
    handle
        .set_native_portrait_orientation(NativePortraitOrientation::Down)
        .unwrap();
    install_memory_framebuffer(&handle);
    handle.complete_full_redraw().unwrap();
    take_wake_bytes(&wake);
    let borrowed = handle.inner.framebuffer.borrow_mut();
    handle
        .set_native_portrait_orientation(NativePortraitOrientation::Down)
        .unwrap();
    let error = handle
        .set_native_portrait_orientation(NativePortraitOrientation::Up)
        .unwrap_err();
    assert!(error.to_string().contains("suspended display ownership"));
    assert_eq!(handle.native_render_offset(), Some(180));
    assert!(!handle.needs_full_redraw());
    assert!(!handle.inner.state.transition_in_progress.get());
    assert_eq!(take_wake_bytes(&wake), 0);
    drop(borrowed);
}

#[test]
fn portrait_updates_reject_reentry_and_post_quit_calls_without_partial_changes() {
    let (handle, wake) = handle();
    handle.complete_full_redraw().unwrap();
    let transition = handle.inner.state.enter_transition().unwrap();
    assert!(
        handle
            .set_native_portrait_orientation(NativePortraitOrientation::Down)
            .is_err()
    );
    assert_eq!(handle.native_render_offset(), None);
    assert!(!handle.needs_full_redraw());
    drop(transition);
    handle.inner.quit.store(true, Ordering::SeqCst);
    for orientation in [
        NativePortraitOrientation::Up,
        NativePortraitOrientation::Down,
    ] {
        assert!(handle.set_native_portrait_orientation(orientation).is_err());
    }
    assert_eq!(handle.native_render_offset(), None);
    assert!(!handle.needs_full_redraw());
    assert!(handle.inner.framebuffer.borrow().is_none());
    assert!(handle.inner.state.input.borrow().is_none());
    assert_eq!(take_wake_bytes(&wake), 0);
}

#[test]
fn fresh_native_observation_keeps_retained_pixels_and_sleep_gate_untouched() {
    let (handle, wake) = handle();
    handle
        .set_native_portrait_orientation(NativePortraitOrientation::Down)
        .unwrap();
    install_memory_framebuffer(&handle);
    drop(
        handle
            .inner
            .state
            .suspend_with(&handle.inner.pending, |_| {
                panic!("the retained memory fixture has no native input")
            })
            .unwrap(),
    );
    take_wake_bytes(&wake);
    let mapping = handle
        .inner
        .framebuffer
        .borrow()
        .as_ref()
        .unwrap()
        .mapped_bytes_for_test()
        .as_ptr();
    let before = handle
        .inner
        .framebuffer
        .borrow()
        .as_ref()
        .unwrap()
        .mapped_bytes_for_test()
        .to_vec();
    let input_generation = handle.input_generation();
    let redraw = handle.needs_full_redraw();
    for rotation in [3, 1] {
        let observed = handle
            .probe_native_framebuffer_with(|geometry, retained| {
                assert_eq!(geometry, Some((3, 2)));
                assert_eq!(
                    retained,
                    Some(FramebufferLayout {
                        width: 3,
                        height: 2,
                        stride: 5,
                        bytes_per_pixel: 1,
                        bits_per_pixel: 8
                    })
                );
                assert!(handle.inner.framebuffer.try_borrow_mut().is_ok());
                Ok(NativeFramebufferObservation {
                    output_inverted: Some(true),
                    rotation,
                })
            })
            .unwrap();
        assert_eq!(observed.rotation, rotation);
        assert_eq!(observed.output_inverted, Some(true));
    }
    assert!(
        handle
            .probe_native_framebuffer_with(|_, _| Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stride changed"
            )))
            .is_err()
    );
    assert_eq!(
        handle
            .inner
            .framebuffer
            .borrow()
            .as_ref()
            .unwrap()
            .mapped_bytes_for_test()
            .as_ptr(),
        mapping
    );
    assert_eq!(
        handle
            .inner
            .framebuffer
            .borrow()
            .as_ref()
            .unwrap()
            .mapped_bytes_for_test(),
        before
    );
    assert_eq!(handle.native_render_offset(), Some(180));
    assert!(handle.is_suspended());
    assert_eq!(handle.input_generation(), input_generation);
    assert_eq!(handle.needs_full_redraw(), redraw);
    assert_eq!(take_wake_bytes(&wake), 0);
}

#[test]
fn native_observation_rejects_borrowed_mapping_and_terminated_event_loop() {
    let (handle, wake) = handle();
    install_memory_framebuffer(&handle);
    let borrowed = handle.inner.framebuffer.borrow_mut();
    assert!(
        handle
            .probe_native_framebuffer_with(|_, _| panic!(
                "a conflicting mapping borrow must reject before a probe"
            ))
            .is_err()
    );
    drop(borrowed);
    handle.inner.quit.store(true, Ordering::SeqCst);
    assert!(
        handle
            .probe_native_framebuffer_with(|_, _| panic!(
                "the terminated event loop must reject before a probe"
            ))
            .is_err()
    );
    assert_eq!(take_wake_bytes(&wake), 0);
}

#[test]
fn framebuffer_scanout_compensates_raw_touch_without_software_pixel_rotation() {
    let (handle, wake) = handle();
    assert_eq!(handle.native_render_offset(), None);
    assert_eq!(handle.native_touch_offset(), 0);
    assert_eq!(handle.inner.native_scanout_orientation.get(), None);
    for (orientation, portrait_offset) in [
        (NativePortraitOrientation::Up, 0),
        (NativePortraitOrientation::Down, 180),
    ] {
        handle.set_native_portrait_orientation(orientation).unwrap();
        assert_eq!(handle.native_render_offset(), Some(portrait_offset));
        assert_eq!(handle.native_touch_offset(), 0);
        handle
            .set_native_framebuffer_portrait_orientation(orientation)
            .unwrap();
        assert_eq!(handle.native_render_offset(), Some(0));
        assert_eq!(handle.native_touch_offset(), portrait_offset);
        assert!(handle.needs_full_redraw());
        assert!(handle.inner.framebuffer.borrow().is_none());
        assert_eq!(handle.inner.geometry.get(), None);
        assert!(handle.inner.state.input.borrow().is_none());
        assert_eq!(handle.input_generation(), 0);
        assert_eq!(take_wake_bytes(&wake), 2);
    }
}

#[test]
fn holding_and_scanout_pixels_match_production_raw_touch_at_both_corners() {
    use NativePortraitOrientation::{Down, Up};

    let (window, component) = crate::touch::tests::portrait_window();
    let (handle, wake) = handle_with_window(window.clone());
    let mut pixels = vec![Rgb8Pixel::default(); 200 * 200];
    // Fixed memory samples and raw controller vectors keep the expectations
    // independent of the setter's residual calculation. The first two rows
    // model a new Up holding direction before and after scanout catches up.
    let cases = [
        (Up, Down, 180, 0, [204u8, 17u8], [(20, 20), (80, 80)]),
        (Up, Up, 0, 0, [17, 204], [(20, 20), (80, 80)]),
        (Down, Up, 180, 180, [204, 17], [(80, 80), (20, 20)]),
        (Down, Down, 0, 180, [17, 204], [(80, 80), (20, 20)]),
    ];
    for (holding, scanout, render_offset, touch_offset, gray, raw_corners) in cases {
        handle
            .set_native_framebuffer_portrait_presentation(holding, scanout)
            .unwrap();
        assert_eq!(handle.native_render_offset(), Some(render_offset));
        assert_eq!(handle.native_touch_offset(), touch_offset);
        assert!(handle.needs_full_redraw());
        assert!(handle.inner.framebuffer.borrow().is_none());
        assert!(handle.inner.state.input.borrow().is_none());
        assert_eq!(take_wake_bytes(&wake), 1);

        let mut rendered = false;
        window.draw_if_needed(|renderer| {
            rendered = true;
            renderer.set_rendering_rotation(match handle.native_render_offset().unwrap() {
                0 => RenderingRotation::NoRotation,
                180 => RenderingRotation::Rotate180,
                _ => panic!("portrait residual must be zero or a half turn"),
            });
            renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
            renderer.set_repaint_buffer_type(RepaintBufferType::ReusedBuffer);
            let _ = renderer.render(&mut pixels, 200);
        });
        assert!(rendered);
        for (index, expected) in [(40 * 200 + 40, gray[0]), (160 * 200 + 160, gray[1])] {
            let pixel = &pixels[index];
            assert_eq!([pixel.r, pixel.g, pixel.b], [expected; 3]);
        }
        handle.complete_full_redraw().unwrap();
        crate::touch::tests::assert_portrait_corner_hits(
            &window,
            &component,
            handle.native_touch_offset(),
            raw_corners,
        );
    }
    component.hide().unwrap();
}

#[test]
fn independent_portrait_changes_reject_active_reentry_and_quit_without_partial_state() {
    use NativePortraitOrientation::{Down, Up};

    let (handle, wake) = handle();
    handle
        .set_native_framebuffer_portrait_presentation(Up, Down)
        .unwrap();
    install_memory_framebuffer(&handle);
    handle.complete_full_redraw().unwrap();
    take_wake_bytes(&wake);
    let generation = handle.input_generation();
    let before = handle
        .inner
        .framebuffer
        .borrow()
        .as_ref()
        .unwrap()
        .mapped_bytes_for_test()
        .to_vec();
    let borrowed = handle.inner.framebuffer.borrow_mut();
    handle
        .set_native_framebuffer_portrait_presentation(Up, Down)
        .unwrap();
    for (holding, scanout) in [(Down, Down), (Up, Up), (Down, Up)] {
        assert!(
            handle
                .set_native_framebuffer_portrait_presentation(holding, scanout)
                .unwrap_err()
                .to_string()
                .contains("suspended display ownership")
        );
        assert_eq!(handle.inner.native_portrait_orientation.get(), Some(Up));
        assert_eq!(handle.inner.native_scanout_orientation.get(), Some(Down));
        assert_eq!(handle.native_render_offset(), Some(180));
        assert_eq!(handle.native_touch_offset(), 0);
        assert_eq!(handle.input_generation(), generation);
        assert_eq!(borrowed.as_ref().unwrap().mapped_bytes_for_test(), before);
        assert!(!handle.needs_full_redraw());
        assert!(!handle.inner.state.transition_in_progress.get());
        assert_eq!(take_wake_bytes(&wake), 0);
    }
    drop(borrowed);

    let transition = handle.inner.state.enter_transition().unwrap();
    assert!(
        handle
            .set_native_framebuffer_portrait_presentation(Down, Up)
            .is_err()
    );
    assert_eq!(handle.inner.native_portrait_orientation.get(), Some(Up));
    assert_eq!(handle.inner.native_scanout_orientation.get(), Some(Down));
    assert!(!handle.needs_full_redraw());
    assert_eq!(take_wake_bytes(&wake), 0);
    drop(transition);

    handle.inner.quit.store(true, Ordering::SeqCst);
    for (holding, scanout) in [(Up, Down), (Down, Up)] {
        assert!(
            handle
                .set_native_framebuffer_portrait_presentation(holding, scanout)
                .is_err()
        );
    }
    assert_eq!(handle.inner.native_portrait_orientation.get(), Some(Up));
    assert_eq!(handle.inner.native_scanout_orientation.get(), Some(Down));
    assert_eq!(handle.native_render_offset(), Some(180));
    assert_eq!(handle.native_touch_offset(), 0);
    assert!(!handle.needs_full_redraw());
    assert_eq!(take_wake_bytes(&wake), 0);
}

#[test]
fn changing_portrait_mapping_requires_suspend_and_rejects_partial_updates() {
    let (handle, wake) = handle();
    handle
        .set_native_portrait_orientation(NativePortraitOrientation::Down)
        .unwrap();
    install_memory_framebuffer(&handle);
    handle.complete_full_redraw().unwrap();
    take_wake_bytes(&wake);
    assert!(
        handle
            .set_native_framebuffer_portrait_orientation(NativePortraitOrientation::Down)
            .is_err()
    );
    assert_eq!(handle.native_render_offset(), Some(180));
    assert_eq!(handle.native_touch_offset(), 0);
    assert!(!handle.needs_full_redraw());
    assert_eq!(take_wake_bytes(&wake), 0);

    drop(
        handle
            .inner
            .state
            .suspend_with(&handle.inner.pending, |_| {
                panic!("the retained memory fixture has no native input")
            })
            .unwrap(),
    );
    let transition = handle.inner.state.enter_transition().unwrap();
    assert!(
        handle
            .set_native_framebuffer_portrait_orientation(NativePortraitOrientation::Down)
            .is_err()
    );
    assert_eq!(handle.native_render_offset(), Some(180));
    assert_eq!(handle.native_touch_offset(), 0);
    drop(transition);
    handle
        .set_native_framebuffer_portrait_orientation(NativePortraitOrientation::Down)
        .unwrap();
    drop(
        handle
            .inner
            .state
            .resume_with(&handle.inner.pending, || Ok(None))
            .unwrap(),
    );
    handle.complete_full_redraw().unwrap();
    take_wake_bytes(&wake);
    let borrowed = handle.inner.framebuffer.borrow_mut();
    handle
        .set_native_framebuffer_portrait_orientation(NativePortraitOrientation::Down)
        .unwrap();
    assert!(
        handle
            .set_native_portrait_orientation(NativePortraitOrientation::Down)
            .is_err()
    );
    assert_eq!(handle.native_render_offset(), Some(0));
    assert_eq!(handle.native_touch_offset(), 180);
    assert!(!handle.needs_full_redraw());
    assert!(!handle.inner.state.transition_in_progress.get());
    assert_eq!(take_wake_bytes(&wake), 0);
    drop(borrowed);
    handle.inner.quit.store(true, Ordering::SeqCst);
    assert!(
        handle
            .set_native_framebuffer_portrait_orientation(NativePortraitOrientation::Down)
            .is_err()
    );
    assert_eq!(handle.native_render_offset(), Some(0));
    assert_eq!(handle.native_touch_offset(), 180);
}

#[test]
fn framebuffer_portrait_wakes_retain_geometry_polarity_and_the_input_barrier() {
    let (handle, wake) = handle();
    handle
        .set_native_framebuffer_portrait_orientation(NativePortraitOrientation::Up)
        .unwrap();
    handle.set_output_inverted(true).unwrap();
    install_memory_framebuffer(&handle);
    handle.complete_full_redraw().unwrap();
    take_wake_bytes(&wake);
    let mapping = handle
        .inner
        .framebuffer
        .borrow()
        .as_ref()
        .unwrap()
        .mapped_bytes_for_test()
        .as_ptr();
    let before = handle
        .inner
        .framebuffer
        .borrow()
        .as_ref()
        .unwrap()
        .mapped_bytes_for_test()
        .to_vec();
    for (orientation, scanout, render_offset, touch_offset) in [
        (
            NativePortraitOrientation::Down,
            NativePortraitOrientation::Down,
            0,
            180,
        ),
        (
            NativePortraitOrientation::Up,
            NativePortraitOrientation::Down,
            180,
            0,
        ),
        (
            NativePortraitOrientation::Up,
            NativePortraitOrientation::Up,
            0,
            0,
        ),
    ] {
        drop(
            handle
                .inner
                .state
                .suspend_with(&handle.inner.pending, |_| {
                    panic!("the retained memory fixture has no native input")
                })
                .unwrap(),
        );
        let input_generation = handle.input_generation();
        take_wake_bytes(&wake);
        handle
            .set_native_framebuffer_portrait_presentation(orientation, scanout)
            .unwrap();
        assert!(handle.is_suspended());
        assert_eq!(handle.native_render_offset(), Some(render_offset));
        assert_eq!(handle.native_touch_offset(), touch_offset);
        assert!(handle.inner.output_inverted.get());
        assert_eq!(handle.inner.geometry.get(), Some((3, 2)));
        assert_eq!(handle.input_generation(), input_generation);
        assert_eq!(handle.touch_poll_snapshot(), (-1, input_generation));
        assert_eq!(handle.ensure_active_io().unwrap(), None);
        assert!(
            handle
                .with_framebuffer(|_| panic!("sleep must retain the write gate"))
                .is_err()
        );
        assert_eq!(
            handle
                .inner
                .framebuffer
                .borrow()
                .as_ref()
                .unwrap()
                .mapped_bytes_for_test(),
            before
        );
        assert_eq!(take_wake_bytes(&wake), 1);
        drop(
            handle
                .inner
                .state
                .resume_with(&handle.inner.pending, || Ok(None))
                .unwrap(),
        );
        assert!(handle.needs_full_redraw());
        assert_eq!(
            handle
                .ensure_framebuffer_with(|| panic!("wake must reuse the retained mapping"))
                .unwrap(),
            Some((3, 2))
        );
        assert_eq!(
            handle
                .inner
                .framebuffer
                .borrow()
                .as_ref()
                .unwrap()
                .mapped_bytes_for_test()
                .as_ptr(),
            mapping
        );
        handle.complete_full_redraw().unwrap();
        assert!(!handle.needs_full_redraw());
    }
}

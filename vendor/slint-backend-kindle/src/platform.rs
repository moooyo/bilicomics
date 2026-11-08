use std::cell::RefCell;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::Rgb8Pixel;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, RenderingRotation, RepaintBufferType,
};
use slint::platform::{Clipboard, EventLoopProxy, Platform, PlatformError, WindowAdapter};

use crate::clipboard::{ClipboardKind, ClipboardStore};
use crate::lifecycle::DisplayLifecycleHandle;
use crate::power::{arm_wakealarm, find_wakealarm, suspend_to_mem};
use crate::refresh::{PendingRefresh, PhysicalRect, RefreshHandle};
use crate::touch::has_accelerometer;
use crate::wakeup::{self, KindleEventLoopProxy, Queue, Wakeup};
use crate::{OnWakeCallback, WakeSchedule};

// Animations get redrawn at most ~30 fps. E-ink can't keep up with anything
// faster, so quicker wakes would just waste battery.
const ANIMATION_FRAME: Duration = Duration::from_millis(33);

pub(crate) struct KindlePlatform {
    pub(crate) window: Rc<MinimalSoftwareWindow>,
    start: Instant,
    queue: Queue,
    wakeup: Wakeup,
    quit_flag: Arc<AtomicBool>,
    pub(crate) wake_schedule: Arc<Mutex<Option<WakeSchedule>>>,
    pub(crate) on_wake: OnWakeCallback,
    black_and_white: Arc<AtomicBool>,
    rotation: Arc<AtomicU32>,
    refresh_pending: Arc<Mutex<PendingRefresh>>,
    lifecycle: DisplayLifecycleHandle,
    clipboard: RefCell<ClipboardStore>,
}

impl KindlePlatform {
    pub(crate) fn new(
        wake_schedule: Arc<Mutex<Option<WakeSchedule>>>,
        on_wake: OnWakeCallback,
        black_and_white: Arc<AtomicBool>,
        rotation: Arc<AtomicU32>,
    ) -> std::io::Result<Self> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        let wakeup = wakeup::make_wakeup()?;
        let quit_flag = Arc::new(AtomicBool::new(false));
        let refresh_pending = Arc::new(Mutex::new(PendingRefresh::default()));
        let lifecycle = DisplayLifecycleHandle::new(
            window.clone(),
            refresh_pending.clone(),
            wakeup.write.clone(),
            quit_flag.clone(),
        );
        Ok(Self {
            window,
            start: Instant::now(),
            queue: wakeup::new_queue(),
            wakeup,
            quit_flag,
            wake_schedule,
            on_wake,
            black_and_white,
            rotation,
            refresh_pending,
            lifecycle,
            clipboard: RefCell::new(ClipboardStore::default()),
        })
    }

    pub(crate) fn refresh_handle(&self) -> RefreshHandle {
        RefreshHandle {
            pending: self.refresh_pending.clone(),
            wake: self.wakeup.write.clone(),
            quit: self.quit_flag.clone(),
            suspended: self.lifecycle.suspended_flag(),
        }
    }

    pub(crate) fn lifecycle_handle(&self) -> DisplayLifecycleHandle {
        self.lifecycle.clone()
    }

    /// Suspend the device to RAM once it's been idle for `stay_awake` with no
    /// pending work, then arm the wakealarm to bring it back. Returns `true`
    /// if a suspend cycle ran (the caller should restart the event loop).
    fn suspend_if_idle(&self, wakealarm: Option<&Path>, last_interaction: &mut Instant) -> bool {
        let (Some(schedule), Some(wakealarm_path)) = (
            *self.wake_schedule.lock().expect("wake schedule poisoned"),
            wakealarm,
        ) else {
            return false;
        };

        // Pending Slint timers don't block suspend: they'll just fire on
        // resume (a 1 Hz clock timer would otherwise pin the device awake).
        let nothing_pending = !self.window.has_active_animations()
            && wakeup::queue_is_empty(&self.queue, &self.quit_flag);
        let nothing_pending = nothing_pending
            && self
                .refresh_pending
                .lock()
                .expect("refresh queue poisoned")
                .is_empty();
        if self.quit_flag.load(Ordering::SeqCst)
            || last_interaction.elapsed() < schedule.stay_awake
            || !nothing_pending
        {
            return false;
        }

        if self
            .lifecycle
            .with_framebuffer(|framebuffer| framebuffer.wait_for_update_complete())
            .is_err()
        {
            return false;
        }

        // If arming fails we still suspend, sleeping to save battery is better than
        // staying awake.
        if let Err(e) = arm_wakealarm(wakealarm_path, schedule.wake_interval) {
            log::error!(
                "failed to arm RTC wakealarm: {e}; device may only wake on user input this cycle"
            );
        }
        if let Err(e) = suspend_to_mem() {
            log::error!("suspend-to-RAM failed: {e}");
        }

        // Start a fresh stay_awake window so the consumer's app
        // gets at least that long to react.
        *last_interaction = Instant::now();
        // Fire the consumer's on-wake callback (if any) before any rendering
        // this cycle, so e.g. an HTTP poll runs before the next draw shows
        // stale data.
        if let Some(callback) = self.on_wake.borrow_mut().as_mut() {
            callback();
        }
        true
    }
}

impl Platform for KindlePlatform {
    fn set_clipboard_text(&self, text: &str, clipboard: Clipboard) {
        let kind = match clipboard {
            Clipboard::DefaultClipboard => ClipboardKind::Default,
            Clipboard::SelectionClipboard => ClipboardKind::Selection,
            _ => return,
        };
        self.clipboard.borrow_mut().set(kind, text);
    }

    fn clipboard_text(&self, clipboard: Clipboard) -> Option<String> {
        let kind = match clipboard {
            Clipboard::DefaultClipboard => ClipboardKind::Default,
            Clipboard::SelectionClipboard => ClipboardKind::Selection,
            _ => return None,
        };
        self.clipboard.borrow().text(kind)
    }

    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(KindleEventLoopProxy {
            queue: self.queue.clone(),
            _read_fd: self.wakeup.read.clone(),
            write_fd: self.wakeup.write.clone(),
            quit_flag: self.quit_flag.clone(),
        }))
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        struct QuitOnExit {
            quit: Arc<AtomicBool>,
            queue: Queue,
            lifecycle: DisplayLifecycleHandle,
        }
        impl Drop for QuitOnExit {
            fn drop(&mut self) {
                let pending = wakeup::close_and_take_pending(&self.queue, &self.quit);
                self.lifecycle.release_event_loop_io();
                drop(pending);
            }
        }
        let _quit_on_exit = QuitOnExit {
            quit: self.quit_flag.clone(),
            queue: self.queue.clone(),
            lifecycle: self.lifecycle.clone(),
        };
        // Opening native devices is deferred. An initially sleeping process
        // must not grab touch, fill the framebuffer, or flash the system UI.
        let mut geometry = None;
        let mut rgb_buffer = Vec::new();
        let mut gray_buffer = Vec::new();
        let mut render_offset = 0;
        let mut initial_fill = true;
        let mut applied_rotation = self.rotation.load(Ordering::Relaxed);
        let wakeup_read_fd = self.wakeup.read.as_raw_fd();
        let mut wakealarm = None;
        let mut last_interaction = Instant::now();

        loop {
            if self.quit_flag.load(Ordering::SeqCst) {
                break;
            }
            if let Some(size) = self.lifecycle.ensure_active_io()? {
                if geometry.is_none() {
                    geometry = Some(size);
                    rgb_buffer = vec![Rgb8Pixel::default(); size.0 as usize * size.1 as usize];
                    gray_buffer = vec![0u8; size.0 as usize];
                    render_offset = resolved_render_offset(
                        self.lifecycle.native_render_offset(),
                        launch_render_offset,
                    );
                    crate::touch::set_input_diagnostic_render_offset(render_offset);
                    applied_rotation = self.rotation.load(Ordering::Relaxed);
                    self.window
                        .set_size(rotated_size(size, applied_rotation, render_offset));
                    self.window.request_redraw();
                    wakealarm = find_wakealarm().ok();
                }
                self.lifecycle.ensure_portrait_observer()?;
            }
            // External screensaver ownership takes precedence over the
            // optional upstream RTC schedule. It does not program sysfs.
            if !self.lifecycle.is_suspended()
                && !self.lifecycle.needs_full_redraw()
                && self.suspend_if_idle(wakealarm.as_deref(), &mut last_interaction)
            {
                continue;
            }
            if self.quit_flag.load(Ordering::SeqCst) {
                break;
            }

            // Sleep and a submitted frame awaiting input synchronization use
            // descriptor wakeups only. Neither state drives Slint timers.
            let timeout_ms: libc::c_int = if self.lifecycle.is_suspended()
                || self.lifecycle.live_input_drain_waiting()
            {
                -1
            } else if self.lifecycle.needs_full_redraw() {
                1
            } else {
                match (
                    self.window.has_active_animations(),
                    slint::platform::duration_until_next_timer_update(),
                ) {
                    (true, Some(d)) => duration_to_ms(d.min(ANIMATION_FRAME)),
                    (true, None) => duration_to_ms(ANIMATION_FRAME),
                    (false, Some(d)) => duration_to_ms(d),
                    (false, None) => -1,
                }
            };

            let (touch_fd, input_generation) = self.lifecycle.touch_poll_snapshot();
            let (portrait_fd, portrait_generation) = self.lifecycle.portrait_poll_snapshot();
            let mut file_descriptors = [
                libc::pollfd {
                    fd: touch_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: wakeup_read_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: portrait_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];

            // Block until an fd has activity or the timeout expires.
            // Retry on EINTR, bail on any other error.
            // SAFETY: fds is a valid 3-element array while poll runs.
            let poll_result = unsafe {
                libc::poll(
                    file_descriptors.as_mut_ptr(),
                    file_descriptors.len() as libc::nfds_t,
                    timeout_ms,
                )
            };
            if poll_result < 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(PlatformError::Other(format!("poll failed: {err}")));
            }

            let err_bits = libc::POLLERR | libc::POLLHUP | libc::POLLNVAL;
            if file_descriptors[1].revents & err_bits != 0 {
                return Err(PlatformError::Other(format!(
                    "poll: control fd died (revents={:#x})",
                    file_descriptors[1].revents
                )));
            }

            // Empty the pipe before running closures so any new wakeup that arrives
            // while a closure runs still triggers another loop iteration.
            if file_descriptors[1].revents & libc::POLLIN != 0 {
                wakeup::drain(&self.wakeup.read);
                let pending = wakeup::take_pending(&self.queue, &self.quit_flag);
                for c in pending {
                    if self.quit_flag.load(Ordering::SeqCst) {
                        break;
                    }
                    c();
                }
            }

            // Check early for quit before doing more work
            if self.quit_flag.load(Ordering::SeqCst) {
                break;
            }

            // A control closure may have synchronously dropped/replaced the
            // polled touch fd. Its old revents must not terminate the app or
            // be replayed into the freshly grabbed input lease.
            if self.lifecycle.is_suspended() {
                continue;
            }
            if geometry.is_none() {
                continue;
            }
            // A holding-direction change precedes touch dispatch. Its contact
            // cancellation and full-frame barrier invalidate the old poll snapshot.
            if file_descriptors[2].revents & err_bits != 0 {
                self.lifecycle.retire_portrait_observer(portrait_generation);
            } else if file_descriptors[2].revents & libc::POLLIN != 0 {
                match self.lifecycle.poll_portrait(portrait_generation) {
                    Ok(true) => last_interaction = Instant::now(),
                    Ok(false) => {},
                    Err(error) => {
                        log::warn!("live portrait observation disabled until resume: {error}");
                    }
                }
            }
            if self.lifecycle.is_suspended() || self.quit_flag.load(Ordering::SeqCst) {
                continue;
            }
            if input_generation == self.lifecycle.input_generation()
                && file_descriptors[0].revents & err_bits != 0
            {
                return Err(PlatformError::Other(format!(
                    "poll: touch fd died (revents={:#x})",
                    file_descriptors[0].revents
                )));
            }

            // The full frame was already written and refreshed. Only a fresh
            // touch readiness retries its final drain; never dispatch that
            // same queued contact or repaint while the report is missing.
            // A control/gyro transition above can reset waiting for a new frame.
            if self.lifecycle.live_input_drain_waiting() {
                if input_generation == self.lifecycle.input_generation()
                    && file_descriptors[0].revents & libc::POLLIN != 0
                {
                    self.lifecycle.complete_full_redraw()?;
                    if !self.lifecycle.live_input_drain_waiting() {
                        last_interaction = Instant::now();
                    }
                }
                continue;
            }

            let force_full = self.lifecycle.needs_full_redraw();
            if !force_full && file_descriptors[0].revents & libc::POLLIN != 0 {
                last_interaction = Instant::now();
            }
            if !force_full {
                let native_touch_offset = self.lifecycle.native_touch_offset();
                crate::touch::set_input_diagnostic_native_touch_offset(native_touch_offset);
                self.lifecycle
                    .poll_touch((applied_rotation + native_touch_offset) % 360);
                if self.lifecycle.is_suspended() {
                    continue;
                }
                slint::platform::update_timers_and_animations();
                if self.lifecycle.is_suspended() {
                    continue;
                }
            }

            // A timer or input callback may have changed output polarity. Such
            // a frame must use the same complete rewrite as a control callback.
            let force_full = force_full || self.lifecycle.needs_full_redraw();
            let black_and_white = self.black_and_white.load(Ordering::Relaxed);
            let app_rotation = self.rotation.load(Ordering::Relaxed);
            let native_render_offset = self
                .lifecycle
                .native_render_offset()
                .unwrap_or(render_offset);
            let rotation_changed =
                app_rotation != applied_rotation || native_render_offset != render_offset;
            let (panel_width, panel_height) = geometry.expect("active framebuffer has geometry");
            if rotation_changed {
                applied_rotation = app_rotation;
                render_offset = native_render_offset;
                crate::touch::set_input_diagnostic_render_offset(render_offset);
                self.window.set_size(rotated_size(
                    (panel_width, panel_height),
                    app_rotation,
                    render_offset,
                ));
                self.window.request_redraw();
            }
            // Snapshot before drawing. Requests produced concurrently during
            // this render stay queued for the next awakened iteration.
            let pending = self
                .refresh_pending
                .lock()
                .expect("refresh queue poisoned")
                .take();
            if !pending.is_empty() || force_full {
                self.window.request_redraw();
            }
            let mut dirty_region = None;
            let mut did_draw = false;
            self.lifecycle.with_framebuffer(|frame_buffer| {
                self.window.draw_if_needed(|renderer| {
                    did_draw = true;
                    if initial_fill {
                        frame_buffer.fill(0xff);
                        initial_fill = false;
                    }
                    renderer.set_rendering_rotation(
                        match (applied_rotation + render_offset) % 360 {
                            90 => RenderingRotation::Rotate90,
                            180 => RenderingRotation::Rotate180,
                            270 => RenderingRotation::Rotate270,
                            _ => RenderingRotation::NoRotation,
                        },
                    );
                    if rotation_changed || force_full {
                        renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
                        renderer.set_repaint_buffer_type(RepaintBufferType::ReusedBuffer);
                    }
                    let width = panel_width as usize;
                    let dirty = renderer.render(&mut rgb_buffer, width);
                    let origin = dirty.bounding_box_origin();
                    let size = dirty.bounding_box_size();
                    let (x0, y0) = (origin.x as usize, origin.y as usize);
                    let (w, h) = (size.width as usize, size.height as usize);

                    let gray = &mut gray_buffer[..w];
                    for row in 0..h {
                        let start = (y0 + row) * width + x0;
                        let rgb = &rgb_buffer[start..start + w];
                        for (g, p) in gray.iter_mut().zip(rgb.iter()) {
                            let value =
                                ((77 * p.r as u32 + 150 * p.g as u32 + 29 * p.b as u32) >> 8) as u8;
                            // Black-and-white mode forces pure black/white based on threshold
                            *g = if black_and_white {
                                if value < 128 { 0x00 } else { 0xff }
                            } else {
                                value
                            };
                        }
                        frame_buffer.write_line(y0 + row, x0..x0 + w, gray);
                    }

                    dirty_region = Some(PhysicalRect {
                        x: origin.x,
                        y: origin.y,
                        width: size.width,
                        height: size.height,
                    });
                });
                // All framebuffer writes above are complete before an ioctl is
                // issued. The existing descriptor and detected ABI are reused.
                if did_draw && (rotation_changed || force_full) {
                    frame_buffer.refresh_full();
                } else if let Some(refresh) = crate::refresh::resolve(
                    pending,
                    dirty_region,
                    panel_width,
                    panel_height,
                    (applied_rotation + render_offset) % 360,
                ) {
                    frame_buffer.refresh_planned(refresh);
                }
            })?;
            if force_full && did_draw {
                self.lifecycle.complete_full_redraw()?;
                last_interaction = Instant::now();
            }
        }

        match wakeup::failure(&self.queue, &self.quit_flag) {
            Some(message) => Err(PlatformError::Other(message)),
            None => Ok(()),
        }
    }
}

fn rotated_size(size: (u32, u32), rotation: u32, launch_offset: u32) -> slint::PhysicalSize {
    if (rotation + launch_offset).is_multiple_of(180) {
        slint::PhysicalSize::new(size.0, size.1)
    } else {
        slint::PhysicalSize::new(size.1, size.0)
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::sync::mpsc;

    fn sleeping_platform() -> KindlePlatform {
        let platform = KindlePlatform::new(
            Arc::new(Mutex::new(None)),
            Rc::new(RefCell::new(None)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU32::new(0)),
        )
        .unwrap();
        platform.lifecycle_handle().suspend().unwrap();
        platform
    }

    #[test]
    fn a_sleeping_loop_executes_control_and_quit_without_opening_native_devices() {
        let platform = KindlePlatform::new(
            Arc::new(Mutex::new(None)),
            Rc::new(RefCell::new(None)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU32::new(0)),
        )
        .unwrap();
        let lifecycle = platform.lifecycle_handle();
        lifecycle.suspend().unwrap();
        let observed = Arc::new(AtomicBool::new(false));
        let ran = observed.clone();
        let quit = platform.quit_flag.clone();
        platform
            .new_event_loop_proxy()
            .unwrap()
            .invoke_from_event_loop(Box::new(move || {
                ran.store(true, Ordering::SeqCst);
                quit.store(true, Ordering::SeqCst);
            }))
            .unwrap();
        platform.run_event_loop().unwrap();
        assert!(observed.load(Ordering::SeqCst));
        assert!(lifecycle.is_suspended());
    }

    #[test]
    fn quit_cancels_the_rest_of_a_control_batch_before_any_native_work() {
        let platform = sleeping_platform();
        let proxy = platform.new_event_loop_proxy().unwrap();
        let quitter = platform.new_event_loop_proxy().unwrap();
        proxy
            .invoke_from_event_loop(Box::new(move || quitter.quit_event_loop().unwrap()))
            .unwrap();
        let ran = Arc::new(AtomicBool::new(false));
        let second = ran.clone();
        let (sender, receiver) = mpsc::channel::<()>();
        proxy
            .invoke_from_event_loop(Box::new(move || {
                second.store(true, Ordering::SeqCst);
                drop(sender);
            }))
            .unwrap();
        platform.run_event_loop().unwrap();
        assert!(!ran.load(Ordering::SeqCst));
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
        assert!(platform.lifecycle_handle().is_suspended());
        assert!(platform.lifecycle_handle().resume().is_err());
    }

    #[test]
    fn queue_capacity_failure_returns_through_native_lifecycle_cleanup() {
        let platform = sleeping_platform();
        let proxy = platform.new_event_loop_proxy().unwrap();
        for _ in 0..wakeup::MAX_QUEUED_CLOSURES {
            proxy.invoke_from_event_loop(Box::new(|| {})).unwrap();
        }
        assert!(proxy.invoke_from_event_loop(Box::new(|| {})).is_err());
        let error = platform.run_event_loop().unwrap_err();
        assert!(format!("{error}").contains("capacity"));
        assert!(platform.lifecycle_handle().is_suspended());
        assert!(wakeup::queue_is_empty(&platform.queue, &platform.quit_flag));
        assert!(platform.lifecycle_handle().resume().is_err());
    }

    #[test]
    fn dropping_the_platform_releases_pending_work_even_with_live_proxies() {
        let platform = sleeping_platform();
        let lifecycle = platform.lifecycle_handle();
        let proxy = platform.new_event_loop_proxy().unwrap();
        let (sender, receiver) = mpsc::channel::<()>();
        proxy
            .invoke_from_event_loop(Box::new(move || drop(sender)))
            .unwrap();
        drop(platform);
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
        assert!(lifecycle.is_suspended());
        assert!(lifecycle.resume().is_err());
        assert!(proxy.invoke_from_event_loop(Box::new(|| {})).is_err());
        proxy.quit_event_loop().unwrap();
    }

    #[test]
    fn a_panicking_control_callback_still_closes_admission_and_releases_captures() {
        let platform = sleeping_platform();
        let proxy = platform.new_event_loop_proxy().unwrap();
        proxy
            .invoke_from_event_loop(Box::new(|| panic!("controlled callback failure")))
            .unwrap();
        let (sender, receiver) = mpsc::channel::<()>();
        proxy
            .invoke_from_event_loop(Box::new(move || drop(sender)))
            .unwrap();
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| platform.run_event_loop()));
        assert!(result.is_err());
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
        assert!(wakeup::queue_is_empty(&platform.queue, &platform.quit_flag));
        assert!(platform.lifecycle_handle().resume().is_err());
        assert!(proxy.invoke_from_event_loop(Box::new(|| {})).is_err());
    }
}

fn duration_to_ms(d: Duration) -> libc::c_int {
    // Round up to at least 1 ms. A timeout of 0 makes poll skip the wait
    // entirely, which would spin the CPU if a tiny timer kept re-firing.
    d.as_millis().clamp(1, libc::c_int::MAX as u128) as libc::c_int
}

impl Drop for KindlePlatform {
    fn drop(&mut self) {
        let pending = wakeup::close_and_take_pending(&self.queue, &self.quit_flag);
        self.lifecycle.release_event_loop_io();
        drop(pending);
    }
}

/// Devices with an accelerometer let the framework flip its UI 180° when
/// held the other way up. Touch input then already arrives flipped, but the
/// framebuffer does not, so rendering has to compensate by whatever the
/// framework thinks is up when the app starts.
fn launch_render_offset() -> u32 {
    launch_render_offset_with(has_accelerometer(), crate::command::run_bounded_command)
}

fn resolved_render_offset(staged: Option<u32>, legacy_query: impl FnOnce() -> u32) -> u32 {
    staged.unwrap_or_else(legacy_query)
}

fn launch_render_offset_with(
    accelerometer: bool,
    query: impl FnOnce(&Path, &[&str], Duration, usize) -> std::io::Result<String>,
) -> u32 {
    if !accelerometer {
        return 0;
    }
    let flipped = match query(
        Path::new("lipc-get-prop"),
        &["com.lab126.winmgr", "accelerometer"],
        Duration::from_secs(2),
        64,
    ) {
        Ok(orientation) => orientation.trim() == "D",
        Err(error) => {
            log::warn!("bounded startup orientation query failed: {error}");
            false
        }
    };
    if flipped { 180 } else { 0 }
}

#[cfg(test)]
mod orientation_tests {
    use super::*;

    #[test]
    fn explicitly_staged_portrait_offsets_never_execute_the_legacy_query() {
        for orientation in [
            crate::NativePortraitOrientation::Up,
            crate::NativePortraitOrientation::Down,
        ] {
            assert_eq!(
                resolved_render_offset(Some(orientation.render_offset()), || panic!(
                    "the owning launcher already observed orientation"
                )),
                orientation.render_offset(),
            );
        }
        assert_eq!(resolved_render_offset(None, || 180), 180);
    }

    #[test]
    fn startup_orientation_query_has_a_deadline_and_small_output_budget() {
        let rotation =
            launch_render_offset_with(true, |program, arguments, timeout, output_limit| {
                assert_eq!(program, Path::new("lipc-get-prop"));
                assert_eq!(arguments, ["com.lab126.winmgr", "accelerometer"]);
                assert_eq!(timeout, Duration::from_secs(2));
                assert_eq!(output_limit, 64);
                Ok("D\n".to_owned())
            });
        assert_eq!(rotation, 180);
    }

    #[test]
    fn startup_orientation_failure_preserves_the_upright_fallback() {
        let rotation = launch_render_offset_with(true, |_, _, _, _| {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "controlled query timeout",
            ))
        });
        assert_eq!(rotation, 0);
    }

    #[test]
    fn a_device_without_an_accelerometer_does_not_spawn_a_query() {
        assert_eq!(
            launch_render_offset_with(false, |_, _, _, _| panic!("query must be skipped")),
            0
        );
    }
}

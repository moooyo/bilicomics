use std::cell::RefCell;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, RawFd};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{
    MinimalSoftwareWindow, RenderingRotation, RepaintBufferType, SoftwareRenderer,
};
use slint::platform::{Clipboard, EventLoopProxy, Platform, PlatformError, WindowAdapter};

use crate::clipboard::{ClipboardKind, ClipboardStore};
use crate::dithering::DitheringHandle;
use crate::frame_observer::{
    FrameObserverState, NativeCheckpointPhase, NativeFrameCheckpoint, NativeFrameUnavailable,
    reader_output_eligible,
};
use crate::framebuffer::Framebuffer;
use crate::gray_target::RetainedScene;
use crate::lifecycle::{LifecycleHandle, LifecycleState};
use crate::portrait::{InputEpoch, NativePortraitBinding};
use crate::portrait_runtime::LivePortrait;
use crate::power::{arm_wakealarm, find_wakealarm, suspend_to_mem};
use crate::refresh::{PendingRefresh, PhysicalRect, RefreshHandle};
use crate::render_control::{GrayMode, GrayRenderer, RenderHandle};
use crate::touch::{TouchInput, has_accelerometer, synchronization_retryable};
use crate::wakeup::{self, KindleEventLoopProxy, Queue, Wakeup};
use crate::{ControlCallback, InputObserver, OnWakeCallback, WakeSchedule};

// Animations get redrawn at most ~30 fps. E-ink can't keep up with anything
// faster, so quicker wakes would just waste battery.
const ANIMATION_FRAME: Duration = Duration::from_millis(33);
const ORIENTATION_TIMEOUT: Duration = Duration::from_millis(500);
const ORIENTATION_POLL: Duration = Duration::from_millis(10);

// Slint snapshots use the unrotated window dimensions and stride. Apply the
// physical panel rotation only while drawing the retained scanout buffer.
// Changing rendering rotation does not invalidate Slint's logical dirty cache.
struct ScanoutRotation<'a> {
    renderer: &'a SoftwareRenderer,
    previous: RenderingRotation,
}

impl<'a> ScanoutRotation<'a> {
    fn new(renderer: &'a SoftwareRenderer, rotation: RenderingRotation) -> Self {
        let previous = renderer.rendering_rotation();
        renderer.set_rendering_rotation(rotation);
        Self { renderer, previous }
    }
}

impl Drop for ScanoutRotation<'_> {
    fn drop(&mut self) {
        self.renderer.set_rendering_rotation(self.previous);
    }
}

/// A matching image may need a full write when its dithering policy changes.
/// Native presentation barriers remain separate from that image processing.
#[derive(Clone, Copy)]
struct RepaintReason {
    presentation: bool,
    dithering: bool,
    sleep: bool,
}

impl RepaintReason {
    fn force_full(self) -> bool {
        self.presentation || self.dithering || self.sleep
    }

    fn reader_sample_eligible(
        self,
        sequence: u64,
        resume_pending: bool,
        rotation_changed: bool,
        output_mode_changed: bool,
    ) -> bool {
        !self.sleep
            && reader_output_eligible(
                sequence,
                resume_pending,
                self.presentation || rotation_changed || output_mode_changed,
            )
    }
}

pub(crate) struct KindlePlatform {
    pub(crate) window: Rc<MinimalSoftwareWindow>,
    start: Instant,
    queue: Queue,
    wakeup: Wakeup,
    quit_flag: Arc<AtomicBool>,
    pub(crate) wake_schedule: Arc<Mutex<Option<WakeSchedule>>>,
    pub(crate) on_wake: OnWakeCallback,
    input_observer: InputObserver,
    control_callback: ControlCallback,
    frame_observer: Rc<FrameObserverState>,
    lifecycle: Arc<LifecycleState>,
    black_and_white: Arc<AtomicBool>,
    dark_mode: Arc<AtomicBool>,
    dithering: DitheringHandle,
    rotation: Arc<AtomicU32>,
    refresh_pending: Arc<Mutex<PendingRefresh>>,
    clipboard: RefCell<ClipboardStore>,
}

impl KindlePlatform {
    fn portrait_binding(&self) -> (Option<NativePortraitBinding>, u64) {
        let binding = self
            .lifecycle
            .portrait_calibration
            .lock()
            .expect("portrait calibration poisoned");
        (
            *binding,
            self.lifecycle.portrait_revision.load(Ordering::Acquire),
        )
    }

    fn begin_input_boundary(
        &self,
        touch: &mut TouchInput,
        epoch: &mut InputEpoch,
    ) -> Result<(), PlatformError> {
        self.lifecycle
            .input_generation
            .store(epoch.begin(), Ordering::Release);
        if let Err(error) = touch.cancel_and_synchronize(&self.window, &self.input_observer)
            && !synchronization_retryable(&error)
        {
            return Err(PlatformError::Other(format!(
                "synchronize native touch at full-frame boundary: {error}"
            )));
        }
        self.lifecycle
            .full_repaint_requested
            .store(true, Ordering::Release);
        self.window.request_redraw();
        Ok(())
    }

    fn observe_portrait(
        &self,
        framebuffer: &Framebuffer,
        touch: &mut TouchInput,
        portrait: &mut LivePortrait,
        epoch: &mut InputEpoch,
    ) -> Result<bool, PlatformError> {
        let previously_blocked = portrait.is_blocked();
        match portrait.observe_framebuffer(framebuffer) {
            Ok(true) => {
                self.begin_input_boundary(touch, epoch)?;
                Ok(true)
            }
            Ok(false) => Ok(false),
            Err(error) => {
                // Optional native metadata failure must preserve this reading
                // session. Retire the observer and keep I/O gated until resume.
                if !previously_blocked {
                    log::warn!(
                        "native portrait presentation is gated until a safe observation: {error}"
                    );
                }
                if !epoch.is_blocked() {
                    self.begin_input_boundary(touch, epoch)?;
                }
                Ok(false)
            }
        }
    }

    fn synchronize_portrait_ownership(
        &self,
        touch: &mut TouchInput,
        framebuffer: &Framebuffer,
        portrait: &mut LivePortrait,
        epoch: &mut InputEpoch,
    ) -> Result<bool, PlatformError> {
        let (binding, revision) = self.portrait_binding();
        let calibration_changed = portrait.synchronize_binding(binding, revision);
        let resumed = self.apply_external_ownership(touch, framebuffer)?;
        let paused = self.lifecycle.applied_paused.load(Ordering::Acquire);
        let ownership_changed = portrait.set_paused(paused);
        if calibration_changed || ownership_changed {
            if paused {
                self.lifecycle
                    .input_generation
                    .store(epoch.begin(), Ordering::Release);
            } else {
                self.begin_input_boundary(touch, epoch)?;
            }
        }
        if !paused {
            match portrait.ensure_observer() {
                Ok(true) => {
                    self.observe_portrait(framebuffer, touch, portrait, epoch)?;
                }
                Ok(false) => {}
                Err(error) => {
                    log::warn!(
                        "native portrait observer is gated until the next ownership generation: {error}"
                    );
                    if !epoch.is_blocked() {
                        self.begin_input_boundary(touch, epoch)?;
                    }
                }
            }
        }
        Ok(resumed)
    }

    fn apply_requested_rotation(
        &self,
        touch: &mut TouchInput,
        epoch: &mut InputEpoch,
        applied_rotation: &mut u32,
        panel_size: (u32, u32),
    ) -> Result<(), PlatformError> {
        let requested = self.rotation.load(Ordering::Relaxed);
        if requested == *applied_rotation {
            return Ok(());
        }
        self.begin_input_boundary(touch, epoch)?;
        *applied_rotation = requested;
        let size = if requested.is_multiple_of(180) {
            slint::PhysicalSize::new(panel_size.0, panel_size.1)
        } else {
            slint::PhysicalSize::new(panel_size.1, panel_size.0)
        };
        if self.window.size() != size {
            self.window.set_size(size);
        }
        Ok(())
    }

    pub(crate) fn new(
        wake_schedule: Arc<Mutex<Option<WakeSchedule>>>,
        on_wake: OnWakeCallback,
        black_and_white: Arc<AtomicBool>,
        rotation: Arc<AtomicU32>,
        input_observer: InputObserver,
        control_callback: ControlCallback,
        frame_observer: Rc<FrameObserverState>,
    ) -> std::io::Result<Self> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        let wakeup = wakeup::make_wakeup()?;
        let dithering = DitheringHandle::native(wakeup.write.clone());
        Ok(Self {
            window,
            start: Instant::now(),
            queue: wakeup::new_queue(),
            wakeup,
            quit_flag: Arc::new(AtomicBool::new(false)),
            wake_schedule,
            on_wake,
            input_observer,
            control_callback,
            frame_observer,
            lifecycle: Arc::new(LifecycleState::default()),
            black_and_white,
            dark_mode: Arc::new(AtomicBool::new(false)),
            dithering,
            rotation,
            refresh_pending: Arc::new(Mutex::new(PendingRefresh::default())),
            clipboard: RefCell::new(ClipboardStore::default()),
        })
    }

    pub(crate) fn refresh_handle(&self) -> RefreshHandle {
        RefreshHandle {
            pending: self.refresh_pending.clone(),
            wake: self.wakeup.write.clone(),
            quit: self.quit_flag.clone(),
            lifecycle: self.lifecycle.clone(),
        }
    }

    pub(crate) fn render_handle(&self) -> RenderHandle {
        RenderHandle {
            dark_mode: self.dark_mode.clone(),
            wake: self.wakeup.write.clone(),
            quit: self.quit_flag.clone(),
        }
    }

    pub(crate) fn dithering_handle(&self) -> DitheringHandle {
        self.dithering.clone()
    }

    pub(crate) fn lifecycle_handle(&self) -> LifecycleHandle {
        LifecycleHandle {
            state: self.lifecycle.clone(),
            wake: self.wakeup.write.clone(),
            quit: self.quit_flag.clone(),
        }
    }

    fn drive_control(&self) {
        if let Some(callback) = self.control_callback.borrow_mut().as_mut() {
            callback();
        }
    }

    fn event_loop_result(&self) -> Result<(), PlatformError> {
        match wakeup::failure(&self.queue, &self.quit_flag) {
            Some(message) => Err(PlatformError::Other(message)),
            None => Ok(()),
        }
    }

    fn pending_sleep_presentation(&self) -> Result<Option<u64>, PlatformError> {
        self.lifecycle
            .sleep_presentation
            .pending()
            .map_err(|error| PlatformError::Other(error.to_string()))
    }

    fn dispatch_queued_control(&self) {
        // A producer arriving during a callback keeps a new readable wakeup.
        wakeup::drain(&self.wakeup.read);
        for callback in wakeup::take_pending(&self.queue, &self.quit_flag) {
            if self.quit_flag.load(Ordering::SeqCst) {
                break;
            }
            callback();
        }
    }

    /// An initially sleeping process owns neither native display nor input.
    /// Control and quit remain available without running animations or sensors.
    fn wait_for_native_ownership(&self) -> Result<bool, PlatformError> {
        loop {
            if self.quit_flag.load(Ordering::SeqCst) {
                return Ok(false);
            }
            self.drive_control();
            if self.quit_flag.load(Ordering::SeqCst) {
                return Ok(false);
            }
            if !self.lifecycle.pause_requested.load(Ordering::Acquire) {
                return Ok(true);
            }
            if !self.lifecycle.applied_paused.load(Ordering::Acquire) {
                self.lifecycle.input_released.store(true, Ordering::Release);
                self.lifecycle.applied_paused.store(true, Ordering::Release);
                self.lifecycle.pause_count.fetch_add(1, Ordering::AcqRel);
                let _ = self
                    .refresh_pending
                    .lock()
                    .expect("refresh queue poisoned")
                    .take();
            }
            let mut descriptor = libc::pollfd {
                fd: self.wakeup.read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: The initialized descriptor remains live for the wait.
            let result = unsafe { libc::poll(&mut descriptor, 1, -1) };
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(PlatformError::Other(format!(
                    "initial native ownership wait failed: {error}"
                )));
            }
            if descriptor.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Err(PlatformError::Other(format!(
                    "initial native ownership control descriptor failed (revents={:#x})",
                    descriptor.revents,
                )));
            }
            if descriptor.revents & libc::POLLIN != 0 {
                self.dispatch_queued_control();
            }
        }
    }

    fn apply_external_ownership(
        &self,
        touch: &mut TouchInput,
        framebuffer: &Framebuffer,
    ) -> Result<bool, PlatformError> {
        let requested = self.lifecycle.pause_requested.load(Ordering::Acquire);
        let applied = self.lifecycle.applied_paused.load(Ordering::Acquire);
        if requested && !applied {
            if let Some(observer) = self.input_observer.borrow_mut().as_mut() {
                let _ = observer(&slint::platform::WindowEvent::PointerExited);
            }
            self.window
                .try_dispatch_event(slint::platform::WindowEvent::PointerExited)?;
            touch.set_active(false).map_err(|error| {
                PlatformError::Other(format!(
                    "release native touch for stock screensaver: {error}"
                ))
            })?;
            framebuffer.wait_for_native_handoff().map_err(|error| {
                PlatformError::Other(format!(
                    "failed to settle raw display before native handoff: {error}"
                ))
            })?;
            self.lifecycle.input_released.store(true, Ordering::Release);
            self.lifecycle.applied_paused.store(true, Ordering::Release);
            self.lifecycle.pause_count.fetch_add(1, Ordering::AcqRel);
            let _ = self
                .refresh_pending
                .lock()
                .expect("refresh queue poisoned")
                .take();
        } else if !requested && applied {
            touch.set_active(true).map_err(|error| {
                PlatformError::Other(format!(
                    "retake native touch after stock screensaver: {error}"
                ))
            })?;
            self.lifecycle
                .input_released
                .store(false, Ordering::Release);
            self.lifecycle
                .applied_paused
                .store(false, Ordering::Release);
            self.lifecycle.resume_count.fetch_add(1, Ordering::AcqRel);
            self.lifecycle
                .resume_frame_pending
                .store(true, Ordering::Release);
            self.lifecycle
                .full_repaint_requested
                .store(true, Ordering::Release);
            self.window.request_redraw();
            return Ok(true);
        }
        Ok(false)
    }

    fn output_mode(&self) -> GrayMode {
        GrayMode {
            dark_mode: self.dark_mode.load(Ordering::Acquire),
            black_and_white: self.black_and_white.load(Ordering::Relaxed),
        }
    }

    /// Suspend the device to RAM once it's been idle for `stay_awake` with no
    /// pending work, then arm the wakealarm to bring it back. Returns `true`
    /// if a suspend cycle ran (the caller should restart the event loop).
    fn suspend_if_idle(
        &self,
        frame_buffer: &Framebuffer,
        touch: &mut TouchInput,
        portrait: &mut LivePortrait,
        epoch: &mut InputEpoch,
        wakealarm: Option<&Path>,
        last_interaction: &mut Instant,
        applied_mode: GrayMode,
    ) -> Result<bool, PlatformError> {
        let (Some(schedule), Some(wakealarm_path)) = (
            *self.wake_schedule.lock().expect("wake schedule poisoned"),
            wakealarm,
        ) else {
            return Ok(false);
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
        let nothing_pending = nothing_pending && self.output_mode() == applied_mode;
        if last_interaction.elapsed() < schedule.stay_awake || !nothing_pending {
            return Ok(false);
        }

        frame_buffer.wait_for_native_handoff().map_err(|error| {
            PlatformError::Other(format!(
                "failed to settle raw display before suspend: {error}"
            ))
        })?;
        self.begin_input_boundary(touch, epoch)?;
        portrait.set_paused(true);

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
        portrait.set_paused(false);
        Ok(true)
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
            lifecycle: Arc<LifecycleState>,
        }
        impl Drop for QuitOnExit {
            fn drop(&mut self) {
                let pending = wakeup::close_and_take_pending(&self.queue, &self.quit);
                self.lifecycle.sleep_presentation.terminate();
                drop(pending);
            }
        }
        // Native I/O locals are declared after this guard, so their ownership
        // is released before pending control captures are destroyed on exit.
        let _quit_on_exit = QuitOnExit {
            quit: self.quit_flag.clone(),
            queue: self.queue.clone(),
            lifecycle: self.lifecycle.clone(),
        };
        if !self.wait_for_native_ownership()? {
            return self.event_loop_result();
        }
        // Serialize the pre-I/O calibration claim with its public setter.
        let (binding, revision) = {
            let binding = self
                .lifecycle
                .portrait_calibration
                .lock()
                .expect("portrait calibration poisoned");
            self.lifecycle
                .native_io_started
                .store(true, Ordering::Release);
            (
                *binding,
                self.lifecycle.portrait_revision.load(Ordering::Acquire),
            )
        };
        let mut portrait = LivePortrait::new(binding, revision);
        let mut input_epoch = InputEpoch::default();
        let mut frame_buffer = Framebuffer::open()
            .map_err(|error| PlatformError::Other(format!("failed to open /dev/fb0: {error}")))?;
        let mut touch_input =
            TouchInput::open(frame_buffer.width, frame_buffer.height).map_err(|error| {
                PlatformError::Other(format!("failed to open touch input: {error}"))
            })?;

        if self.quit_flag.load(Ordering::SeqCst) {
            return self.event_loop_result();
        }
        self.drive_control();
        if self.quit_flag.load(Ordering::SeqCst) {
            return self.event_loop_result();
        }
        self.synchronize_portrait_ownership(
            &mut touch_input,
            &frame_buffer,
            &mut portrait,
            &mut input_epoch,
        )?;
        // The first meaningful scene owns the initial full update. Keep input
        // behind its completion barrier even without a portrait calibration.
        if !input_epoch.is_blocked() {
            self.begin_input_boundary(&mut touch_input, &mut input_epoch)?;
        }
        let initial_mode = self.output_mode();
        let mut initial_scene_pending = true;
        let mut input_checkpoint: Option<NativeFrameCheckpoint> = None;

        let width = frame_buffer.width as usize;
        let (panel_width, panel_height) = (frame_buffer.width, frame_buffer.height);
        let mut scene = RetainedScene::new(
            width * panel_height as usize,
            frame_buffer.supports_gray8_target(),
        );
        let mut gray_renderer = GrayRenderer::new(width, initial_mode);
        let mut applied_dithering = self.dithering.revision();
        // The old launch query remains only for callers without an authenticated
        // native calibration. A calibrated startup or resume never probes LIPC
        // from the rendering path while the native WM is paused.
        let compatibility_offset = if binding.is_some() {
            0
        } else {
            launch_render_offset()
        };
        let window_size = |app_rotation: u32| {
            if app_rotation.is_multiple_of(180) {
                slint::PhysicalSize::new(panel_width, panel_height)
            } else {
                slint::PhysicalSize::new(panel_height, panel_width)
            }
        };
        let mut applied_rotation = self.rotation.load(Ordering::Relaxed);
        let mut drawn_rotation = applied_rotation;
        let mut drawn_render_rotation =
            (applied_rotation + portrait.render_offset(compatibility_offset)) % 360;
        self.window.set_size(window_size(applied_rotation));
        let wakeup_read_fd = self.wakeup.read.as_raw_fd();
        let wakealarm = find_wakealarm().ok();
        let mut last_interaction = Instant::now();

        loop {
            if self.quit_flag.load(Ordering::SeqCst) {
                break;
            }
            self.drive_control();
            if self.quit_flag.load(Ordering::SeqCst) {
                break;
            }
            if self.synchronize_portrait_ownership(
                &mut touch_input,
                &frame_buffer,
                &mut portrait,
                &mut input_epoch,
            )? {
                last_interaction = Instant::now();
            }
            let ownership_paused = self.lifecycle.applied_paused.load(Ordering::Acquire);
            let sleep_token = self.pending_sleep_presentation()?;
            if !ownership_paused
                && !portrait.is_blocked()
                && !input_epoch.is_blocked()
                && sleep_token.is_none()
                && self.dithering.revision() == applied_dithering
                && self.suspend_if_idle(
                    &frame_buffer,
                    &mut touch_input,
                    &mut portrait,
                    &mut input_epoch,
                    wakealarm.as_deref(),
                    &mut last_interaction,
                    gray_renderer.mode(),
                )?
            {
                continue;
            }

            // A full-frame barrier runs once without waiting for another input.
            // Idle orientation observation itself adds no periodic timeout.
            let timeout_ms: libc::c_int = if !ownership_paused
                && !portrait.is_blocked()
                && (initial_scene_pending || input_epoch.needs_frame() || sleep_token.is_some())
            {
                0
            } else if self.lifecycle.parked.load(Ordering::Acquire) {
                -1
            } else {
                match (
                    self.window.has_active_animations(),
                    slint::platform::duration_until_next_timer_update(),
                ) {
                    (true, Some(duration)) => duration_to_ms(duration.min(ANIMATION_FRAME)),
                    (true, None) => duration_to_ms(ANIMATION_FRAME),
                    (false, Some(duration)) => duration_to_ms(duration),
                    (false, None) => -1,
                }
            };
            let (portrait_fd, portrait_generation) = portrait.poll_snapshot();
            let touch_generation = input_epoch.snapshot();
            // Orientation is processed before touch from the same poll batch.
            let mut file_descriptors = [
                libc::pollfd {
                    fd: portrait_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: if ownership_paused || input_epoch.needs_frame() || portrait.is_blocked() {
                        -1
                    } else {
                        touch_input.fd()
                    },
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: wakeup_read_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: The complete initialized pollfd array remains alive.
            let poll_result = unsafe {
                libc::poll(
                    file_descriptors.as_mut_ptr(),
                    file_descriptors.len() as libc::nfds_t,
                    timeout_ms,
                )
            };
            if poll_result < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(PlatformError::Other(format!("poll failed: {error}")));
            }
            let error_bits = libc::POLLERR | libc::POLLHUP | libc::POLLNVAL;
            if (file_descriptors[1].revents | file_descriptors[2].revents) & error_bits != 0 {
                return Err(PlatformError::Other(format!(
                    "poll: input fd died (touch revents={:#x}, wakeup revents={:#x})",
                    file_descriptors[1].revents, file_descriptors[2].revents,
                )));
            }
            if file_descriptors[2].revents & libc::POLLIN != 0 {
                self.dispatch_queued_control();
            }
            if self.quit_flag.load(Ordering::SeqCst) {
                break;
            }
            self.drive_control();
            if self.quit_flag.load(Ordering::SeqCst) {
                break;
            }
            if self.synchronize_portrait_ownership(
                &mut touch_input,
                &frame_buffer,
                &mut portrait,
                &mut input_epoch,
            )? {
                last_interaction = Instant::now();
            }
            if self.quit_flag.load(Ordering::SeqCst) {
                break;
            }
            let ownership_paused = self.lifecycle.applied_paused.load(Ordering::Acquire);
            let sleep_token = self.pending_sleep_presentation()?;
            if !ownership_paused {
                // Snapshot application rotation before accepting input. A manual
                // 90/270 preference remains independent of native holding pose.
                self.apply_requested_rotation(
                    &mut touch_input,
                    &mut input_epoch,
                    &mut applied_rotation,
                    (panel_width, panel_height),
                )?;
                let current_portrait_generation = portrait.poll_snapshot().1;
                if portrait_generation == current_portrait_generation {
                    if file_descriptors[0].revents & error_bits != 0 {
                        portrait.fail_source();
                        if !input_epoch.is_blocked() {
                            self.begin_input_boundary(&mut touch_input, &mut input_epoch)?;
                        }
                        log::warn!(
                            "native portrait descriptor failed; observation is gated until resume"
                        );
                    } else if file_descriptors[0].revents & libc::POLLIN != 0 {
                        if let Err(error) = portrait.poll(portrait_generation) {
                            log::warn!(
                                "native portrait observation failed; observation is gated until resume: {error}"
                            );
                            if !input_epoch.is_blocked() {
                                self.begin_input_boundary(&mut touch_input, &mut input_epoch)?;
                            }
                        }
                    }
                }
                // Refresh GETVAR on every ready batch, even a duplicate pose or
                // ordinary wakeup. Native observation failure retires the source
                // until resume, preserving the applied scene and input gate.
                if self.observe_portrait(
                    &frame_buffer,
                    &mut touch_input,
                    &mut portrait,
                    &mut input_epoch,
                )? {
                    last_interaction = Instant::now();
                }
                if sleep_token.is_none()
                    && !portrait.is_blocked()
                    && input_epoch.awaiting_snapshot()
                    && (file_descriptors[1].revents & libc::POLLIN != 0
                        || file_descriptors[2].revents & libc::POLLIN != 0)
                {
                    match touch_input.synchronize_after_frame() {
                        Ok(()) => {
                            input_epoch.complete_frame();
                            if let Some(mut checkpoint) = input_checkpoint.take() {
                                checkpoint.phase = NativeCheckpointPhase::InputReady;
                                checkpoint.input_blocked = false;
                                checkpoint.active = checkpoint.active
                                    && !self.lifecycle.pause_requested.load(Ordering::Acquire)
                                    && !self.lifecycle.applied_paused.load(Ordering::Acquire)
                                    && !self.quit_flag.load(Ordering::Acquire);
                                self.frame_observer.checkpoint(checkpoint);
                            }
                        }
                        Err(error) if synchronization_retryable(&error) => {}
                        Err(error) => {
                            return Err(PlatformError::Other(format!(
                                "resynchronize native touch after GC16: {error}"
                            )));
                        }
                    }
                } else if sleep_token.is_none()
                    && !portrait.is_blocked()
                    && input_epoch.accepts(touch_generation)
                    && file_descriptors[1].revents & libc::POLLIN != 0
                {
                    last_interaction = Instant::now();
                    match touch_input.poll(
                        &self.window,
                        portrait.touch_rotation(applied_rotation),
                        &self.input_observer,
                    ) {
                        Ok(()) => {}
                        Err(error) if synchronization_retryable(&error) => {
                            self.begin_input_boundary(&mut touch_input, &mut input_epoch)?;
                        }
                        Err(error) => {
                            return Err(PlatformError::Other(format!(
                                "native touch poll failed: {error}"
                            )));
                        }
                    }
                }
            }
            if !self.lifecycle.parked.load(Ordering::Acquire) {
                slint::platform::update_timers_and_animations();
            }
            if self.synchronize_portrait_ownership(
                &mut touch_input,
                &frame_buffer,
                &mut portrait,
                &mut input_epoch,
            )? {
                last_interaction = Instant::now();
            }
            if self.lifecycle.applied_paused.load(Ordering::Acquire) || portrait.is_blocked() {
                continue;
            }

            // Timers and control callbacks may change manual landscape after
            // touch processing. The final render snapshot must agree with it.
            self.apply_requested_rotation(
                &mut touch_input,
                &mut input_epoch,
                &mut applied_rotation,
                (panel_width, panel_height),
            )?;
            let sleep_token = self.pending_sleep_presentation()?;
            if input_epoch.awaiting_snapshot() && sleep_token.is_none() {
                // GC16 is already complete. Do not repaint the same barrier or
                // spin at 0ms while waiting for the next complete touch frame.
                continue;
            }

            let output_mode = self.output_mode();
            frame_buffer
                .require_known_dithering_delivery()
                .map_err(|error| {
                    PlatformError::Other(format!(
                        "display delivery is uncertain; native rendering stopped: {error}"
                    ))
                })?;
            let processing_changed =
                frame_buffer.set_dithering(self.dithering.clone(), output_mode.black_and_white);
            let dithering_revision = self.dithering.revision();
            let dithering_changed = dithering_revision != applied_dithering || processing_changed;
            if output_mode != gray_renderer.mode() {
                self.window.request_redraw();
            }
            let render_rotation =
                (applied_rotation + portrait.render_offset(compatibility_offset)) % 360;
            let rotation_changed =
                applied_rotation != drawn_rotation || render_rotation != drawn_render_rotation;
            if rotation_changed {
                self.window.request_redraw();
            }
            let pending = self
                .refresh_pending
                .lock()
                .expect("refresh queue poisoned")
                .take();
            let repaint = RepaintReason {
                presentation: self
                    .lifecycle
                    .full_repaint_requested
                    .swap(false, Ordering::AcqRel)
                    || input_epoch.is_blocked()
                    || initial_scene_pending,
                dithering: dithering_changed,
                sleep: sleep_token.is_some(),
            };
            let force_full = repaint.force_full();
            if !pending.is_empty() || force_full {
                self.window.request_redraw();
            }
            // An earlier accepted update may still scan this same mapping.
            // Sleep presentation must settle it before changing mapped bytes.
            if sleep_token.is_some() || dithering_changed {
                let settled = if dithering_changed {
                    frame_buffer.settle_before_write()
                } else {
                    frame_buffer.wait_for_pending_update_complete_checked()
                };
                settled.map_err(|error| {
                    PlatformError::Other(format!(
                        "failed to settle preceding display update: {error}"
                    ))
                })?;
            }
            // An armed image belongs to this single output attempt. Snapshot
            // rendering and later menu/wake redraws cannot consume it again.
            let mut observed_frame = self.frame_observer.begin();
            let measure_frame = observed_frame.is_some() || self.frame_observer.checkpoint_timing();
            let render_started = measure_frame.then(Instant::now);
            let mut dirty_region = None;
            let mut did_draw = false;
            self.window.draw_if_needed(|renderer| {
                did_draw = true;
                let _scanout_rotation = ScanoutRotation::new(
                    renderer,
                    match render_rotation {
                        90 => RenderingRotation::Rotate90,
                        180 => RenderingRotation::Rotate180,
                        270 => RenderingRotation::Rotate270,
                        _ => RenderingRotation::NoRotation,
                    },
                );
                // Rotation changes require a complete retained scene; Slint
                // does not invalidate its dirty cache for output rotation alone.
                if initial_scene_pending || rotation_changed || sleep_token.is_some() {
                    renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
                }
                let dirty = scene.render(renderer, width);
                if initial_scene_pending || rotation_changed || sleep_token.is_some() {
                    renderer.set_repaint_buffer_type(RepaintBufferType::ReusedBuffer);
                }
                let origin = dirty.bounding_box_origin();
                let size = dirty.bounding_box_size();
                dirty_region = Some(PhysicalRect {
                    x: origin.x,
                    y: origin.y,
                    width: size.width,
                    height: size.height,
                });
            });
            if (initial_scene_pending || sleep_token.is_some()) && !did_draw {
                // A redraw request cannot certify initialized scene pixels.
                // Keep startup or sleep pending until a complete visible draw.
                if let Some(observed) = observed_frame.as_mut() {
                    observed.unavailable(NativeFrameUnavailable::NoWrite);
                }
                continue;
            }
            if force_full {
                dirty_region = Some(PhysicalRect {
                    x: 0,
                    y: 0,
                    width: panel_width,
                    height: panel_height,
                });
            }
            let render_elapsed = render_started.map_or(Duration::ZERO, |start| start.elapsed());
            // SEND queues MDP work asynchronously even on monochrome MTK.
            // Keep its input stable until the previous owned update settles.
            let settle_started = measure_frame.then(Instant::now);
            if dirty_region.is_some() || output_mode != gray_renderer.mode() {
                frame_buffer.settle_before_write().map_err(|error| {
                    PlatformError::Other(format!(
                        "previous framebuffer consumption was not settled: {error}"
                    ))
                })?;
            }
            let settle_elapsed = settle_started.map_or(Duration::ZERO, |start| start.elapsed());
            let write_started = measure_frame.then(Instant::now);
            let written = gray_renderer.write_scene_frame(
                scene.pixels(),
                (panel_width, panel_height),
                dirty_region,
                output_mode,
                |row, columns, pixels| frame_buffer.write_line(row, columns, pixels),
            );
            let write_elapsed = write_started.map_or(Duration::ZERO, |start| start.elapsed());
            let frame_sequence = written.region.map(|_| {
                self.lifecycle
                    .frame_write_count
                    .fetch_add(1, Ordering::AcqRel)
                    .wrapping_add(1)
            });
            if let Some(observed) = observed_frame.as_mut() {
                observed.set_dithering_status(frame_buffer.current_dithering_status());
                if let Some(region) = written.region {
                    let sequence = frame_sequence.expect("written output has a sequence");
                    if repaint.reader_sample_eligible(
                        sequence,
                        self.lifecycle.resume_frame_pending.load(Ordering::Acquire),
                        rotation_changed,
                        written.mode_changed,
                    ) {
                        let full = region.x == 0
                            && region.y == 0
                            && region.width == panel_width
                            && region.height == panel_height;
                        observed.written(sequence, region, full, write_elapsed, render_elapsed);
                    } else {
                        // Initial scene allocation is valid first-image output;
                        // later rewrites and resumes are separate observations.
                        observed.unavailable(NativeFrameUnavailable::Cancelled);
                    }
                } else {
                    observed.unavailable(NativeFrameUnavailable::NoWrite);
                }
            }
            let submission_started = measure_frame.then(Instant::now);
            let fallback_writes_before = frame_buffer.canonical_rewrite_count();
            let mut sleep_submission = None;
            let submission = if sleep_token.is_some() {
                Some(
                    frame_buffer
                        .refresh_full_scene_checked(scene.pixels(), output_mode)
                        .map(|receipt| {
                            sleep_submission = Some(receipt);
                        })
                        .map_err(|error| {
                            PlatformError::Other(format!(
                                "sleep presentation update was rejected: {error}"
                            ))
                        }),
                )
            } else if rotation_changed || written.mode_changed || force_full {
                Some(
                    frame_buffer
                        .refresh_full_scene_checked(scene.pixels(), output_mode)
                        .map(|_| ())
                        .map_err(|error| {
                            PlatformError::Other(format!(
                                "full framebuffer refresh was rejected: {error}"
                            ))
                        }),
                )
            } else if let Some(refresh) = crate::refresh::resolve(
                pending,
                written.region,
                panel_width,
                panel_height,
                render_rotation,
            ) {
                Some(
                    frame_buffer
                        .refresh_planned_scene(refresh, scene.pixels(), output_mode)
                        .map_err(|error| {
                            PlatformError::Other(format!(
                                "framebuffer refresh was rejected: {error}"
                            ))
                        }),
                )
            } else {
                None
            };
            let submission_elapsed =
                submission_started.map_or(Duration::ZERO, |start| start.elapsed());
            let fallback_writes = frame_buffer
                .canonical_rewrite_count()
                .wrapping_sub(fallback_writes_before);
            let rewritten_sequence = (fallback_writes != 0).then(|| {
                self.lifecycle
                    .frame_write_count
                    .fetch_add(fallback_writes, Ordering::AcqRel)
                    .wrapping_add(fallback_writes)
            });
            let accepted = matches!(&submission, Some(Ok(())));
            let initial = initial_scene_pending;
            if accepted {
                initial_scene_pending = false;
                self.lifecycle
                    .frame_send_count
                    .fetch_add(1, Ordering::AcqRel);
            }
            if let Some(observed) = observed_frame.as_mut() {
                match &submission {
                    Some(Ok(())) => {
                        if let Some(marker) = frame_buffer.last_submitted_marker() {
                            if let Some(status) = frame_buffer.accepted_dithering(marker) {
                                observed.set_dithering_status(status);
                            }
                            if frame_buffer.accepted_frame_was_rewritten(marker)
                                && let Some(sequence) = rewritten_sequence
                            {
                                observed.rewritten(
                                    sequence,
                                    PhysicalRect {
                                        x: 0,
                                        y: 0,
                                        width: panel_width,
                                        height: panel_height,
                                    },
                                );
                            }
                        }
                        observed.accepted(frame_buffer.last_submitted_marker(), submission_elapsed)
                    }
                    Some(Err(_)) => {
                        observed.set_dithering_status(frame_buffer.current_dithering_status());
                        observed.unavailable(NativeFrameUnavailable::SendRejected)
                    }
                    None => observed.unavailable(NativeFrameUnavailable::NoWrite),
                }
            }
            if let Some(submission) = submission {
                submission?;
            }
            applied_dithering = dithering_revision;
            drawn_rotation = applied_rotation;
            drawn_render_rotation = render_rotation;
            if accepted {
                let checkpoint = NativeFrameCheckpoint {
                    phase: NativeCheckpointPhase::Submitted,
                    frame_sequence: rewritten_sequence.or(frame_sequence).unwrap_or(0),
                    marker: frame_buffer.last_submitted_marker(),
                    initial,
                    full: force_full || rotation_changed || written.mode_changed,
                    active: sleep_token.is_none()
                        && !self.lifecycle.pause_requested.load(Ordering::Acquire)
                        && !self.lifecycle.applied_paused.load(Ordering::Acquire)
                        && !self.quit_flag.load(Ordering::Acquire),
                    input_blocked: input_epoch.is_blocked(),
                    render_elapsed,
                    settle_elapsed,
                    write_elapsed,
                    submit_elapsed: submission_elapsed,
                };
                input_checkpoint = Some(checkpoint);
                self.frame_observer.checkpoint(checkpoint);
            }
            if let Some(token) = sleep_token {
                let receipt = sleep_submission.as_ref().ok_or_else(|| {
                    PlatformError::Other("sleep presentation has no accepted update".into())
                })?;
                frame_buffer
                    .wait_for_submission_complete_checked(receipt)
                    .map_err(|error| {
                        PlatformError::Other(format!(
                            "sleep presentation update did not complete: {error}"
                        ))
                    })?;
                // A layout/scanout change during the update needs a fresh full
                // sleep frame. Keep its token pending and preserve the input gate.
                if self.observe_portrait(
                    &frame_buffer,
                    &mut touch_input,
                    &mut portrait,
                    &mut input_epoch,
                )? || portrait.is_blocked()
                {
                    continue;
                }
                self.lifecycle_handle()
                    .complete_sleep_presentation(token)
                    .map_err(|error| PlatformError::Other(error.to_string()))?;
                // The caller inhibits ordinary intents and suspends next. Its
                // checkpoint must not wait for a new touch SYN_REPORT here.
                continue;
            }
            if input_epoch.is_blocked() {
                frame_buffer
                    .wait_for_update_complete_checked()
                    .map_err(|error| {
                        PlatformError::Other(format!(
                            "full-frame native input barrier was not acknowledged: {error}"
                        ))
                    })?;
                // If native scanout moved while GC16 ran, rebuild once more
                // before accepting any contact from the new input generation.
                if self.observe_portrait(
                    &frame_buffer,
                    &mut touch_input,
                    &mut portrait,
                    &mut input_epoch,
                )? || portrait.is_blocked()
                {
                    continue;
                }
                input_epoch.mark_frame_applied();
                if self
                    .lifecycle
                    .resume_frame_pending
                    .swap(false, Ordering::AcqRel)
                {
                    self.lifecycle
                        .resume_full_frame_count
                        .fetch_add(1, Ordering::AcqRel);
                }
                match touch_input.synchronize_after_frame() {
                    Ok(()) => input_epoch.complete_frame(),
                    Err(error) if synchronization_retryable(&error) => {}
                    Err(error) => {
                        return Err(PlatformError::Other(format!(
                            "synchronize native touch after full-frame barrier: {error}"
                        )));
                    }
                }
            }
            if !input_epoch.is_blocked()
                && let Some(mut checkpoint) = input_checkpoint.take()
            {
                checkpoint.phase = NativeCheckpointPhase::InputReady;
                checkpoint.input_blocked = false;
                checkpoint.active = checkpoint.active
                    && !self.lifecycle.pause_requested.load(Ordering::Acquire)
                    && !self.lifecycle.applied_paused.load(Ordering::Acquire)
                    && !self.quit_flag.load(Ordering::Acquire);
                self.frame_observer.checkpoint(checkpoint);
            }
        }
        self.event_loop_result()
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
        self.lifecycle.sleep_presentation.terminate();
        drop(pending);
    }
}

/// Devices with an accelerometer let the framework flip its UI 180° when
/// held the other way up. Touch input then already arrives flipped, but the
/// framebuffer does not, so rendering has to compensate by whatever the
/// framework thinks is up when the app starts.
pub(crate) fn launch_render_offset() -> u32 {
    if !has_accelerometer() {
        return 0;
    }
    let mut command = Command::new("lipc-get-prop");
    command.args(["com.lab126.winmgr", "accelerometer"]);
    query_render_offset(&mut command, ORIENTATION_TIMEOUT).unwrap_or(0)
}

fn parse_render_offset(output: &[u8]) -> Option<u32> {
    match output.trim_ascii() {
        b"U" => Some(0),
        b"D" => Some(180),
        _ => None,
    }
}

fn query_render_offset(command: &mut Command, timeout: Duration) -> Option<u32> {
    // This bounds normal userspace protocol polling; it is not a hard deadline
    // on kernel process operations such as spawn, kill, or child reaping.
    let deadline = Instant::now().checked_add(timeout)?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let result = read_render_offset(&mut child, deadline);
    retire_orientation_child(child);
    result
}

fn read_render_offset(child: &mut Child, deadline: Instant) -> Option<u32> {
    let mut stdout = child.stdout.take()?;
    set_orientation_pipe_nonblocking(stdout.as_raw_fd()).ok()?;
    // Reserve one extra byte so an overlong, truncated reply cannot be accepted.
    let mut bytes = [0u8; 17];
    let mut length = 0;
    let mut eof = false;
    loop {
        if Instant::now() >= deadline {
            return None;
        }
        if !eof {
            match stdout.read(&mut bytes[length..]) {
                Ok(0) => eof = true,
                Ok(count) => {
                    length += count;
                    if length == bytes.len() {
                        return None;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return None,
            }
        }
        if let Some(status) = child.try_wait().ok()? {
            if !status.success() {
                return None;
            }
            // A descendant can inherit stdout after the direct child exits.
            // Require EOF under the same deadline rather than a blocking read.
            if eof {
                return parse_render_offset(&bytes[..length]);
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        std::thread::sleep(remaining.min(ORIENTATION_POLL));
    }
}

fn set_orientation_pipe_nonblocking(descriptor: RawFd) -> io::Result<()> {
    // SAFETY: descriptor is the owned child stdout pipe and both fcntl requests
    // use integer flags, with no pointer arguments or change in fd ownership.
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn retire_orientation_child(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod reader_repaint_tests {
    use super::*;
    use crate::framebuffer::FrameDitheringStatus;
    use crate::{NativeFrameObservation, NativeFramePhase};

    fn observed() -> (
        Rc<FrameObserverState>,
        Rc<RefCell<Vec<NativeFrameObservation>>>,
    ) {
        let state = Rc::new(FrameObserverState::default());
        let events = Rc::new(RefCell::new(Vec::new()));
        let recorded = events.clone();
        state.set_observer(move |event| recorded.borrow_mut().push(event));
        (state, events)
    }

    #[test]
    fn an_armed_image_keeps_its_write_and_marker_when_dithering_requires_a_full_frame() {
        for requested in [true, false] {
            let (state, events) = observed();
            let repaint = RepaintReason {
                presentation: false,
                dithering: true,
                sleep: false,
            };
            assert!(repaint.force_full());
            state.arm(12);
            let mut attempt = state.begin().unwrap();
            assert!(repaint.reader_sample_eligible(7, false, false, false));
            attempt.set_dithering_status(FrameDitheringStatus {
                hardware_y4_available: true,
                hardware_y4_requested: requested,
                ..FrameDitheringStatus::default()
            });
            attempt.written(
                7,
                PhysicalRect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 200,
                },
                true,
                Duration::from_micros(20),
                Duration::from_micros(30),
            );
            attempt.set_dithering_status(FrameDitheringStatus {
                hardware_y4_available: true,
                hardware_y4_requested: requested,
                hardware_y4_applied: requested,
                ..FrameDitheringStatus::default()
            });
            attempt.accepted(Some(8), Duration::from_micros(40));
            let events = events.borrow();
            assert_eq!(events.len(), 2);
            assert_eq!(events[0].phase, NativeFramePhase::FramebufferWritten);
            assert!(!events[0].hardware_y4_applied);
            assert_eq!(events[1].phase, NativeFramePhase::SendAccepted);
            assert_eq!(events[1].marker, Some(8));
            assert_eq!(events[1].hardware_y4_requested, requested);
            assert_eq!(events[1].hardware_y4_applied, requested);
            assert!(
                events
                    .iter()
                    .all(|event| event.token == 12 && event.frame_sequence == 7)
            );
            assert!(state.begin().is_none());
        }
    }

    #[test]
    fn a_control_revision_without_an_armed_image_cannot_create_a_reader_observation() {
        let (state, events) = observed();
        let repaint = RepaintReason {
            presentation: false,
            dithering: true,
            sleep: false,
        };
        for sequence in [7, 8] {
            assert!(repaint.force_full());
            assert!(repaint.reader_sample_eligible(sequence, false, false, false));
            assert!(state.begin().is_none());
        }
        assert!(events.borrow().is_empty());
    }

    #[test]
    fn dithering_does_not_override_native_presentation_cancellation() {
        // A sleep presentation remains ineligible even at the first write.
        for (sequence, presentation, sleep, resume, rotation, output_mode) in [
            (7, true, false, false, false, false),
            (7, false, false, true, false, false),
            (7, false, false, false, true, false),
            (7, false, false, false, false, true),
            (7, false, true, false, false, false),
            (1, false, true, false, false, false),
        ] {
            let (state, events) = observed();
            let repaint = RepaintReason {
                presentation,
                dithering: true,
                sleep,
            };
            state.arm(12);
            let mut attempt = state.begin().unwrap();
            assert!(!repaint.reader_sample_eligible(sequence, resume, rotation, output_mode));
            attempt.unavailable(NativeFrameUnavailable::Cancelled);
            attempt.accepted(Some(8), Duration::ZERO);
            let events = events.borrow();
            assert_eq!(events.len(), 1);
            assert_eq!(
                events[0].phase,
                NativeFramePhase::Unavailable(NativeFrameUnavailable::Cancelled)
            );
            assert_eq!(events[0].marker, None);
            assert!(!events[0].hardware_y4_applied);
        }
    }
}

#[cfg(test)]
mod control_queue_tests {
    use super::*;
    use std::sync::mpsc;

    fn platform() -> KindlePlatform {
        KindlePlatform::new(
            Arc::new(Mutex::new(None)),
            Rc::new(RefCell::new(None)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU32::new(0)),
            Rc::new(RefCell::new(None)),
            Rc::new(RefCell::new(None)),
            Rc::new(FrameObserverState::default()),
        )
        .unwrap()
    }

    fn sleeping_platform() -> KindlePlatform {
        let platform = platform();
        platform.lifecycle_handle().begin_pause().unwrap();
        platform
    }

    #[test]
    fn a_sleeping_loop_accepts_control_and_quit_without_opening_native_devices() {
        let platform = sleeping_platform();
        let lifecycle = platform.lifecycle_handle();
        let observed = lifecycle.clone();
        let proxy = platform.new_event_loop_proxy().unwrap();
        let quitter = platform.new_event_loop_proxy().unwrap();
        proxy
            .invoke_from_event_loop(Box::new(move || {
                let status = observed.status();
                assert!(status.render_paused && status.input_released);
                observed.park().unwrap();
                quitter.quit_event_loop().unwrap();
            }))
            .unwrap();
        platform.run_event_loop().unwrap();
        let status = lifecycle.status();
        assert!(status.render_paused && status.input_released && status.parked);
        assert_eq!(status.pause_count, 1);
        assert_eq!(status.resume_count, 0);
        assert_eq!(status.frame_write_count, 0);
        assert_eq!(status.frame_send_count, 0);
        assert!(!lifecycle.state.native_io_started.load(Ordering::Acquire));
    }

    #[test]
    fn initial_resume_defers_native_reacquisition_and_its_counter_until_io_starts() {
        let platform = sleeping_platform();
        let lifecycle = platform.lifecycle_handle();
        let observed = lifecycle.clone();
        platform
            .new_event_loop_proxy()
            .unwrap()
            .invoke_from_event_loop(Box::new(move || {
                observed.park().unwrap();
                observed.resume().unwrap();
            }))
            .unwrap();
        assert!(platform.wait_for_native_ownership().unwrap());
        let status = lifecycle.status();
        assert!(status.render_paused && status.input_released);
        assert!(!status.parked);
        assert_eq!(status.pause_count, 1);
        assert_eq!(status.resume_count, 0);
        assert!(!lifecycle.state.native_io_started.load(Ordering::Acquire));
    }

    #[test]
    fn sleeping_control_quit_cancels_the_remainder_of_its_batch() {
        let platform = sleeping_platform();
        let proxy = platform.new_event_loop_proxy().unwrap();
        let quitter = platform.new_event_loop_proxy().unwrap();
        proxy
            .invoke_from_event_loop(Box::new(move || quitter.quit_event_loop().unwrap()))
            .unwrap();
        let (sender, receiver) = mpsc::channel::<()>();
        proxy
            .invoke_from_event_loop(Box::new(move || {
                drop(sender);
                panic!("a control callback ran after quit");
            }))
            .unwrap();
        platform.run_event_loop().unwrap();
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected),
        );
        assert!(!platform.lifecycle.native_io_started.load(Ordering::Acquire));
    }

    #[test]
    fn panicking_sleeping_control_still_closes_admission_and_releases_captures() {
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
            Err(mpsc::RecvTimeoutError::Disconnected),
        );
        assert!(wakeup::queue_is_empty(&platform.queue, &platform.quit_flag));
        assert!(proxy.invoke_from_event_loop(Box::new(|| {})).is_err());
        assert!(!platform.lifecycle.native_io_started.load(Ordering::Acquire));
    }

    #[test]
    fn quit_before_native_io_releases_pending_work_and_closes_admission() {
        let platform = platform();
        let lifecycle = platform.lifecycle_handle();
        let proxy = platform.new_event_loop_proxy().unwrap();
        let (sender, receiver) = mpsc::channel::<()>();
        proxy
            .invoke_from_event_loop(Box::new(move || drop(sender)))
            .unwrap();
        proxy.quit_event_loop().unwrap();
        platform.run_event_loop().unwrap();
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected),
        );
        assert!(!lifecycle.state.native_io_started.load(Ordering::Acquire));
        assert!(lifecycle.resume().is_err());
        assert!(proxy.invoke_from_event_loop(Box::new(|| {})).is_err());
    }

    #[test]
    fn queue_capacity_failure_returns_without_acquiring_native_devices() {
        let platform = platform();
        let proxy = platform.new_event_loop_proxy().unwrap();
        for _ in 0..wakeup::MAX_QUEUED_CLOSURES {
            proxy.invoke_from_event_loop(Box::new(|| {})).unwrap();
        }
        assert!(proxy.invoke_from_event_loop(Box::new(|| {})).is_err());
        let error = platform.run_event_loop().unwrap_err();
        assert!(format!("{error}").contains("capacity"));
        assert!(!platform.lifecycle.native_io_started.load(Ordering::Acquire));
        assert!(wakeup::queue_is_empty(&platform.queue, &platform.quit_flag));
    }

    #[test]
    fn platform_drop_releases_captures_while_proxies_keep_the_pipe_reader_alive() {
        let platform = platform();
        let lifecycle = platform.lifecycle_handle();
        let token = lifecycle.request_sleep_presentation().unwrap();
        let proxy = platform.new_event_loop_proxy().unwrap();
        let (sender, receiver) = mpsc::channel::<()>();
        proxy
            .invoke_from_event_loop(Box::new(move || drop(sender)))
            .unwrap();
        drop(platform);
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected),
        );
        assert!(proxy.invoke_from_event_loop(Box::new(|| {})).is_err());
        proxy.quit_event_loop().unwrap();
        assert!(lifecycle.sleep_presentation_completed(token).is_err());
        assert!(lifecycle.pending_sleep_presentation().is_err());
    }
}

#[cfg(test)]
mod scanout_rotation_tests {
    use super::{RenderingRotation, ScanoutRotation, SoftwareRenderer};

    #[test]
    fn quarter_turn_scopes_restore_the_window_snapshot_rotation() {
        let renderer = SoftwareRenderer::new();
        for rotation in [
            RenderingRotation::Rotate90,
            RenderingRotation::Rotate270,
            RenderingRotation::Rotate90,
        ] {
            {
                let _scanout_rotation = ScanoutRotation::new(&renderer, rotation);
                assert_eq!(renderer.rendering_rotation(), rotation);
            }
            assert_eq!(renderer.rendering_rotation(), RenderingRotation::NoRotation);
        }
    }

    #[test]
    fn scanout_unwinding_restores_the_window_snapshot_rotation() {
        let renderer = SoftwareRenderer::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _scanout_rotation = ScanoutRotation::new(&renderer, RenderingRotation::Rotate270);
            panic!("scanout scope fixture");
        }));
        assert!(result.is_err());
        assert_eq!(renderer.rendering_rotation(), RenderingRotation::NoRotation);
    }
}

#[cfg(test)]
mod orientation_tests {
    use super::{
        parse_render_offset, query_render_offset, read_render_offset, retire_orientation_child,
        set_orientation_pipe_nonblocking,
    };
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn fixture(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn only_complete_up_or_down_orientation_tokens_are_accepted() {
        assert_eq!(parse_render_offset(b"U\n"), Some(0));
        assert_eq!(parse_render_offset(b" \tD\r\n"), Some(180));
        for output in [
            &b""[..],
            &b"u"[..],
            &b"DD"[..],
            &b"U D"[..],
            &b"orientation D"[..],
            &b"\xffD"[..],
        ] {
            assert_eq!(parse_render_offset(output), None);
        }
    }

    #[test]
    fn child_fixtures_require_success_and_reject_overlong_replies() {
        for (script, expected) in [
            ("printf 'U\\n'", Some(0)),
            ("printf 'D\\n'", Some(180)),
            ("exit 0", None),
            ("printf D; exit 7", None),
            ("printf 'D               '", Some(180)),
            ("printf 'D                '", None),
            ("printf 'U D'", None),
        ] {
            assert_eq!(
                query_render_offset(&mut fixture(script), Duration::from_secs(1)),
                expected,
                "{script}",
            );
        }
    }

    #[test]
    fn a_hung_child_is_killed_and_reaped_after_the_poll_deadline() {
        let started = Instant::now();
        assert_eq!(
            query_render_offset(
                &mut fixture("printf D; exec sleep 5"),
                Duration::from_millis(60),
            ),
            None,
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn inherited_stdout_cannot_hold_the_orientation_query_open() {
        let mut child = fixture("sleep 5 & printf D")
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let process_group = child.id() as libc::pid_t;
        let started = Instant::now();
        let result = read_render_offset(&mut child, started + Duration::from_millis(60));
        // SAFETY: this fixture creates its own process group; its sleeping child
        // keeps that group alive after the shell exits and is cleaned up here.
        unsafe { libc::kill(-process_group, libc::SIGKILL) };
        retire_orientation_child(child);
        assert_eq!(result, None);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn an_invalid_pipe_descriptor_is_rejected_without_a_read() {
        assert!(set_orientation_pipe_nonblocking(-1).is_err());
    }
}

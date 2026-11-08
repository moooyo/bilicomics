//! UI-thread display ownership for an external system screensaver lifecycle.

use std::{
    cell::{Cell, RefCell},
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use slint::platform::{PlatformError, WindowAdapter, software_renderer::MinimalSoftwareWindow};

use crate::{
    NativeFramebufferObservation, NativePortraitOrientation,
    framebuffer::{Framebuffer, FramebufferLayout},
    portrait_events::NativePortraitObserver,
    refresh::PendingRefresh,
    touch::TouchInput,
};

fn failure(message: &str) -> PlatformError {
    PlatformError::Other(message.to_owned())
}

/// A UI-thread-only handle. It deliberately does not implement `Send` or `Sync`.
///
/// Suspend returns only after touch has been dropped and future input, timers,
/// framebuffer writes, and update requests are gated. The application must
/// inhibit its intents before calling it, and must not hold an application
/// `RefCell` borrow across pointer cancellation callbacks.
#[derive(Clone)]
pub struct DisplayLifecycleHandle {
    pub(crate) inner: Rc<DisplayLifecycle>,
}

pub(crate) struct DisplayLifecycle {
    window: Rc<MinimalSoftwareWindow>,
    state: LifecycleState<TouchInput>,
    framebuffer: RefCell<Option<Framebuffer>>,
    output_inverted: Cell<bool>,
    native_portrait_orientation: Cell<Option<NativePortraitOrientation>>,
    native_scanout_orientation: Cell<Option<NativePortraitOrientation>>,
    portrait_calibration: Cell<Option<PortraitCalibration>>,
    portrait_observer: RefCell<Option<NativePortraitObserver>>,
    portrait_observer_attempted: Cell<bool>,
    portrait_generation: Cell<u64>,
    live_portrait_repaint_pending: Cell<bool>,
    live_input_drain_waiting: Cell<bool>,
    resume_trace_path: RefCell<Option<PathBuf>>,
    resume_frame_pending: Cell<bool>,
    geometry: Cell<Option<(u32, u32)>>,
    sleep_settled: Cell<bool>,
    pending: Arc<Mutex<PendingRefresh>>,
    wake: Arc<OwnedFd>,
    quit: Arc<AtomicBool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PortraitCalibration {
    scanout: NativePortraitOrientation,
    raw_rotation: u32,
}

impl PortraitCalibration {
    fn resolve_scanout(self, raw_rotation: u32) -> Result<NativePortraitOrientation, PlatformError> {
        if raw_rotation > 3 {
            return Err(failure("the observed native rotation is unknown"));
        }
        match (raw_rotation + 4 - self.raw_rotation) % 4 {
            0 => Ok(self.scanout),
            2 => Ok(match self.scanout {
                NativePortraitOrientation::Up => NativePortraitOrientation::Down,
                NativePortraitOrientation::Down => NativePortraitOrientation::Up,
            }),
            _ => Err(failure("a live native landscape transition is not approved")),
        }
    }
}

/// The transition logic is shared with pure fake-input tests below.
struct LifecycleState<T> {
    input: RefCell<Option<T>>,
    suspended: Arc<AtomicBool>,
    full_redraw: Cell<bool>,
    input_generation: Cell<u64>,
    transition_in_progress: Cell<bool>,
}

struct TransitionGuard<'a>(&'a Cell<bool>);
impl Drop for TransitionGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl<T> LifecycleState<T> {
    fn new() -> Self {
        Self {
            input: RefCell::new(None),
            suspended: Arc::new(AtomicBool::new(false)),
            full_redraw: Cell::new(true),
            input_generation: Cell::new(0),
            transition_in_progress: Cell::new(false),
        }
    }

    fn enter_transition(&self) -> Result<TransitionGuard<'_>, PlatformError> {
        if self.transition_in_progress.replace(true) {
            return Err(failure(
                "a Kindle display lifecycle transition is already in progress",
            ));
        }
        Ok(TransitionGuard(&self.transition_in_progress))
    }

    fn suspend_with(
        &self,
        pending: &Mutex<PendingRefresh>,
        cancel: impl FnOnce(&mut T) -> Result<(), PlatformError>,
    ) -> Result<TransitionGuard<'_>, PlatformError> {
        let transition = self.enter_transition()?;
        // Publish the gate first: producers cannot race a later queue clear.
        self.suspended.store(true, Ordering::SeqCst);
        self.full_redraw.set(true);
        let input = self
            .input
            .try_borrow_mut()
            .map_err(|_| failure("touch input is already borrowed"))?
            .take();
        self.input_generation
            .set(self.input_generation.get().wrapping_add(1));
        let cancelled = if let Some(mut input) = input {
            let result = cancel(&mut input);
            drop(input);
            result
        } else {
            Ok(())
        };
        pending
            .lock()
            .map_err(|_| failure("refresh queue is poisoned"))?
            .take();
        cancelled?;
        Ok(transition)
    }

    fn resume_with(
        &self,
        pending: &Mutex<PendingRefresh>,
        open: impl FnOnce() -> Result<Option<T>, PlatformError>,
    ) -> Result<TransitionGuard<'_>, PlatformError> {
        let transition = self.enter_transition()?;
        if !self.suspended.load(Ordering::SeqCst) {
            return Ok(transition);
        }
        pending
            .lock()
            .map_err(|_| failure("refresh queue is poisoned"))?
            .take();
        let mut input = self
            .input
            .try_borrow_mut()
            .map_err(|_| failure("touch input is already borrowed"))?;
        if input.is_some() {
            return Err(failure("suspended touch input has not been released"));
        }
        // A failed grab leaves the display suspended and input unowned.
        *input = open()?;
        self.input_generation
            .set(self.input_generation.get().wrapping_add(1));
        self.full_redraw.set(true);
        self.suspended.store(false, Ordering::SeqCst);
        Ok(transition)
    }
}

impl DisplayLifecycleHandle {
    pub(crate) fn new(
        window: Rc<MinimalSoftwareWindow>,
        pending: Arc<Mutex<PendingRefresh>>,
        wake: Arc<OwnedFd>,
        quit: Arc<AtomicBool>,
    ) -> Self {
        Self {
            inner: Rc::new(DisplayLifecycle {
                window,
                state: LifecycleState::new(),
                framebuffer: RefCell::new(None),
                output_inverted: Cell::new(false),
                native_portrait_orientation: Cell::new(None),
                native_scanout_orientation: Cell::new(None),
                portrait_calibration: Cell::new(None),
                portrait_observer: RefCell::new(None),
                portrait_observer_attempted: Cell::new(false),
                portrait_generation: Cell::new(0),
                live_portrait_repaint_pending: Cell::new(false),
                live_input_drain_waiting: Cell::new(false),
                resume_trace_path: RefCell::new(None),
                resume_frame_pending: Cell::new(false),
                geometry: Cell::new(None),
                sleep_settled: Cell::new(false),
                pending,
                wake,
                quit,
            }),
        }
    }

    pub fn is_suspended(&self) -> bool {
        self.inner.state.suspended.load(Ordering::SeqCst)
    }

    /// Read current framebuffer polarity without mapping or writing the display.
    ///
    /// Each call opens `/dev/fb0` read-only and issues only the two screen-info
    /// GET ioctls. No input is acquired and no display gate is removed. Known
    /// 8-bit grayscale modes return their actual polarity; another valid format
    /// returns `None` so the consumer can use its reviewed legacy mode source.
    /// I/O failures or layout inconsistent with the retained mapping fail.
    pub fn probe_output_inversion(&self) -> Result<Option<bool>, PlatformError> {
        self.probe_output_inversion_with(crate::framebuffer::probe_output_inversion)
    }

    fn probe_output_inversion_with(
        &self,
        query: impl FnOnce(
            Option<(u32, u32)>,
            Option<FramebufferLayout>,
        ) -> std::io::Result<Option<bool>>,
    ) -> Result<Option<bool>, PlatformError> {
        if self.inner.quit.load(Ordering::SeqCst) {
            return Err(failure("the Kindle event loop has terminated"));
        }
        let retained_layout = self
            .inner
            .framebuffer
            .try_borrow()
            .map_err(|_| failure("framebuffer is already borrowed"))?
            .as_ref()
            .map(Framebuffer::retained_layout);
        // Copy only metadata, then release the borrow before querying the device.
        // A changed layout must fail rather than reusing or remapping its pixels.
        query(self.inner.geometry.get(), retained_layout).map_err(|error| {
            PlatformError::Other(format!(
                "failed to observe framebuffer output polarity: {error}"
            ))
        })
    }

    /// Observe fresh raw rotation and polarity without resuming display or input.
    ///
    /// The retained geometry, stride, and pixel depth must still match. This
    /// returns a raw kernel rotation, not a device-specific portrait mapping.
    pub fn probe_native_framebuffer(&self) -> Result<NativeFramebufferObservation, PlatformError> {
        self.probe_native_framebuffer_with(crate::framebuffer::probe_native_framebuffer)
    }

    /// Read a recognized accelerometer's current pose without acquiring input.
    ///
    /// This queries only evdev capabilities, device name, and ABS_PRESSURE.
    /// The owner must combine this independent pose with its calibrated scanout
    /// observation before resuming display and touch ownership.
    pub fn probe_native_portrait_pose(
        &self,
    ) -> Result<Option<NativePortraitOrientation>, PlatformError> {
        if self.inner.quit.load(Ordering::SeqCst) {
            return Err(failure("the Kindle event loop has terminated"));
        }
        crate::portrait_pose::read_native_portrait_pose().map_err(|error| {
            PlatformError::Other(format!("failed to observe native portrait pose: {error}"))
        })
    }

    fn probe_native_framebuffer_with(
        &self,
        query: impl FnOnce(
            Option<(u32, u32)>,
            Option<FramebufferLayout>,
        ) -> std::io::Result<NativeFramebufferObservation>,
    ) -> Result<NativeFramebufferObservation, PlatformError> {
        if self.inner.quit.load(Ordering::SeqCst) {
            return Err(failure("the Kindle event loop has terminated"));
        }
        let retained_layout = self
            .inner
            .framebuffer
            .try_borrow()
            .map_err(|_| failure("framebuffer is already borrowed"))?
            .as_ref()
            .map(Framebuffer::retained_layout);
        // Release the mapping borrow before observing metadata through GET ioctls.
        query(self.inner.geometry.get(), retained_layout).map_err(|error| {
            PlatformError::Other(format!(
                "failed to observe native framebuffer presentation: {error}"
            ))
        })
    }

    /// Stage the owner's trusted native portrait orientation before acquiring I/O.
    ///
    /// This compatibility mapping compensates rendered pixels and preserves the
    /// native framework's existing touch transform.
    /// A changed value is accepted before the first framebuffer is initialized or
    /// while suspended. It performs no device I/O or pixel writes, and the next
    /// active frame uses the existing complete rewrite and input-drain barrier.
    /// Changing orientation while initialized and active requires a separate
    /// suspend handoff so an existing Slint pointer grab cannot cross directions.
    pub fn set_native_portrait_orientation(
        &self,
        orientation: NativePortraitOrientation,
    ) -> Result<(), PlatformError> {
        self.set_native_portrait_mapping(orientation, None)
    }

    /// Stage a calibrated orientation when native scanout already rotates pixels.
    ///
    /// This opt-in mapping leaves native pixel rotation to the framebuffer driver
    /// and compensates raw touch coordinates instead. The owner must establish
    /// that this mapping applies to its approved device and firmware. The same
    /// pre-I/O or suspended-only transition and complete-redraw barrier apply.
    pub fn set_native_framebuffer_portrait_orientation(
        &self,
        orientation: NativePortraitOrientation,
    ) -> Result<(), PlatformError> {
        self.set_native_framebuffer_portrait_presentation(orientation, orientation)
    }

    /// Stage a desired holding direction independently of observed native scanout.
    ///
    /// The owner must authenticate both portrait directions for its reviewed
    /// device/firmware mapping. Native scanout may still describe the old holding
    /// direction when the application takes display ownership after sleep.
    /// Pixels compensate only the residual between holding and scanout, while
    /// raw touch coordinates use the holding direction. Equal directions retain
    /// the zero-pixel-offset behavior of the compatibility setter above.
    /// This performs no native I/O and accepts changes only before initial I/O
    /// or while suspended, with the existing complete-redraw and input barrier.
    pub fn set_native_framebuffer_portrait_presentation(
        &self,
        holding_orientation: NativePortraitOrientation,
        scanout_orientation: NativePortraitOrientation,
    ) -> Result<(), PlatformError> {
        self.set_native_portrait_mapping(holding_orientation, Some(scanout_orientation))
    }

    /// Opt into live portrait observation with a trusted scanout/GETVAR anchor.
    ///
    /// The owner must authenticate all three values within its current startup
    /// or resume transaction. A changed calibration is accepted only before
    /// initial native I/O or while suspended. This stages no sensor I/O; the
    /// event loop opens the observer once active geometry is available.
    /// Compatibility setters alone never enable a live observer.
    pub fn set_native_framebuffer_portrait_calibration(
        &self,
        holding: NativePortraitOrientation,
        scanout: NativePortraitOrientation,
        raw_rotation: u32,
    ) -> Result<(), PlatformError> {
        if self.inner.quit.load(Ordering::SeqCst) {
            return Err(failure("the Kindle event loop has terminated"));
        }
        if raw_rotation > 3 {
            return Err(failure("the native portrait calibration rotation is unknown"));
        }
        let calibration = PortraitCalibration { scanout, raw_rotation };
        if self.inner.native_portrait_orientation.get() == Some(holding)
            && self.inner.native_scanout_orientation.get() == Some(scanout)
            && self.inner.portrait_calibration.get() == Some(calibration)
        {
            return Ok(());
        }
        let _transition = self.inner.state.enter_transition()?;
        if self.inner.geometry.get().is_some() && !self.is_suspended() {
            return Err(failure(
                "a native portrait calibration change requires suspended display ownership",
            ));
        }
        self.clear_portrait_observer(true)?;
        self.inner.native_portrait_orientation.set(Some(holding));
        self.inner.native_scanout_orientation.set(Some(scanout));
        self.inner.portrait_calibration.set(Some(calibration));
        self.inner.live_input_drain_waiting.set(false);
        self.inner.state.full_redraw.set(true);
        self.inner.window.request_redraw();
        crate::wakeup::poke(&self.inner.wake);
        Ok(())
    }

    fn set_native_portrait_mapping(
        &self,
        orientation: NativePortraitOrientation,
        scanout_orientation: Option<NativePortraitOrientation>,
    ) -> Result<(), PlatformError> {
        if self.inner.quit.load(Ordering::SeqCst) {
            return Err(failure("the Kindle event loop has terminated"));
        }
        if self.inner.native_portrait_orientation.get() == Some(orientation)
            && self.inner.native_scanout_orientation.get() == scanout_orientation
        {
            return Ok(());
        }
        let _transition = self.inner.state.enter_transition()?;
        if self.inner.geometry.get().is_some() && !self.is_suspended() {
            return Err(failure(
                "a native portrait orientation change requires suspended display ownership",
            ));
        }
        self.clear_portrait_observer(true)?;
        self.inner.portrait_calibration.set(None);
        self.inner.live_input_drain_waiting.set(false);
        self.inner
            .native_portrait_orientation
            .set(Some(orientation));
        self.inner
            .native_scanout_orientation
            .set(scanout_orientation);
        self.inner.state.full_redraw.set(true);
        self.inner.window.request_redraw();
        crate::wakeup::poke(&self.inner.wake);
        Ok(())
    }

    fn portrait_observation_is_active(&self) -> bool {
        !self.inner.quit.load(Ordering::SeqCst)
            && !self.is_suspended()
            && self.inner.geometry.get().is_some()
            && self.inner.portrait_calibration.get().is_some()
    }

    fn clear_portrait_observer(&self, allow_retry: bool) -> Result<(), PlatformError> {
        let mut observer = self.inner.portrait_observer.try_borrow_mut()
            .map_err(|_| failure("portrait observer is already borrowed"))?;
        observer.take();
        self.inner.portrait_generation.set(self.inner.portrait_generation.get().wrapping_add(1));
        self.inner.portrait_observer_attempted.set(!allow_retry);
        Ok(())
    }

    pub(crate) fn ensure_portrait_observer(&self) -> Result<(), PlatformError> {
        if self.inner.quit.load(Ordering::SeqCst) {
            return self.clear_portrait_observer(false);
        }
        self.ensure_portrait_observer_with(
            NativePortraitObserver::open,
            |holding| self.apply_live_portrait(holding),
        )
    }

    fn ensure_portrait_observer_with(
        &self,
        open: impl FnOnce() -> std::io::Result<Option<NativePortraitObserver>>,
        apply_initial: impl FnOnce(NativePortraitOrientation) -> Result<bool, PlatformError>,
    ) -> Result<(), PlatformError> {
        if !self.portrait_observation_is_active() || self.inner.portrait_observer_attempted.get() {
            return Ok(());
        }
        let mut slot = self.inner.portrait_observer.try_borrow_mut()
            .map_err(|_| failure("portrait observer is already borrowed"))?;
        self.inner.portrait_observer_attempted.set(true);
        self.inner.portrait_generation.set(self.inner.portrait_generation.get().wrapping_add(1));
        let observer = match open() {
            Ok(observer) => observer,
            Err(error) => {
                log::warn!("live portrait observer is disabled until resume: {error}");
                return Ok(());
            }
        };
        let initial = observer.as_ref().and_then(NativePortraitObserver::current_pose);
        *slot = observer;
        drop(slot);
        if let Some(holding) = initial
            && let Err(error) = apply_initial(holding)
        {
            log::warn!("initial live portrait observation was rejected: {error}");
            self.retire_portrait_observer(self.inner.portrait_generation.get());
        }
        Ok(())
    }

    pub(crate) fn portrait_poll_snapshot(&self) -> (libc::c_int, u64) {
        let generation = self.inner.portrait_generation.get();
        if !self.portrait_observation_is_active() {
            return (-1, generation);
        }
        let descriptor = self.inner.portrait_observer.try_borrow().ok()
            .and_then(|observer| observer.as_ref().map(NativePortraitObserver::fd))
            .unwrap_or(-1);
        (descriptor, generation)
    }

    pub(crate) fn poll_portrait(&self, expected_generation: u64) -> Result<bool, PlatformError> {
        self.poll_portrait_with(expected_generation, |holding| self.apply_live_portrait(holding))
    }

    fn poll_portrait_with(
        &self,
        expected_generation: u64,
        apply: impl FnOnce(NativePortraitOrientation) -> Result<bool, PlatformError>,
    ) -> Result<bool, PlatformError> {
        if !self.portrait_observation_is_active()
            || self.inner.portrait_generation.get() != expected_generation
        {
            return Ok(false);
        }
        let observation = {
            let mut slot = self.inner.portrait_observer.try_borrow_mut()
                .map_err(|_| failure("portrait observer is already borrowed"))?;
            let Some(observer) = slot.as_mut() else { return Ok(false); };
            observer.poll().map_err(|error| failure(&format!("live portrait observation failed: {error}")))
        };
        let result = match observation {
            Ok(Some(holding)) => apply(holding),
            Ok(None) => Ok(false),
            Err(error) => Err(error),
        };
        if result.is_err() {
            self.retire_portrait_observer(expected_generation);
        }
        result
    }

    pub(crate) fn retire_portrait_observer(&self, expected_generation: u64) {
        if self.inner.portrait_generation.get() != expected_generation {
            return;
        }
        if let Err(error) = self.clear_portrait_observer(false) {
            log::warn!("portrait observer retirement was rejected: {error}");
        } else {
            crate::wakeup::poke(&self.inner.wake);
        }
    }

    fn apply_live_portrait(&self, holding: NativePortraitOrientation) -> Result<bool, PlatformError> {
        self.apply_live_portrait_with(holding, || self.probe_native_framebuffer())
    }

    fn apply_live_portrait_with(
        &self,
        holding: NativePortraitOrientation,
        observe: impl FnOnce() -> Result<NativeFramebufferObservation, PlatformError>,
    ) -> Result<bool, PlatformError> {
        if !self.portrait_observation_is_active() {
            return Ok(false);
        }
        let anchor = self.inner.portrait_calibration.get()
            .expect("active portrait observation has a calibration");
        let observed = observe()?;
        if !self.portrait_observation_is_active() {
            return Ok(false);
        }
        if self.inner.portrait_calibration.get() != Some(anchor) {
            return Err(failure("portrait calibration changed during live observation"));
        }
        let scanout = anchor.resolve_scanout(observed.rotation)?;
        let old_holding = self.inner.native_portrait_orientation.get()
            .expect("a portrait calibration stages its holding direction");
        if old_holding == holding && self.inner.native_scanout_orientation.get() == Some(scanout) {
            return Ok(false);
        }
        // Validate the input borrow before invalidating a live pointer. The
        // transition/full-redraw gate prevents callbacks from polling input.
        let mut input = self.inner.state.input.try_borrow_mut()
            .map_err(|_| failure("touch input is already borrowed"))?;
        let _transition = self.inner.state.enter_transition()?;
        self.inner.state.full_redraw.set(true);
        self.inner.live_portrait_repaint_pending.set(true);
        self.inner.live_input_drain_waiting.set(false);
        let input_result = if let Some(input) = input.as_mut() {
            input.cancel_for_suspend(&self.inner.window).and_then(|_| {
                match input.drain_for_reorientation() {
                    Ok(()) => Ok(()),
                    // The pointer is already cancelled and ordinary input is
                    // gated. Submit the new frame, then wait for the missing
                    // report/snapshot in the final drain instead of retiring
                    // a valid new holding observation.
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
                    Err(error) => Err(failure(&format!("failed to drain live portrait input: {error}"))),
                }
            })
        } else {
            Ok(())
        };
        self.inner.state.input_generation.set(self.inner.state.input_generation.get().wrapping_add(1));
        input_result?;
        self.inner.native_portrait_orientation.set(Some(holding));
        self.inner.native_scanout_orientation.set(Some(scanout));
        self.inner.portrait_calibration.set(Some(PortraitCalibration {
            scanout,
            raw_rotation: observed.rotation,
        }));
        drop(input);
        self.inner.window.request_redraw();
        crate::wakeup::poke(&self.inner.wake);
        self.trace_live_portrait(old_holding, holding, scanout, observed.rotation);
        Ok(true)
    }

    fn trace_live_portrait(
        &self,
        old_holding: NativePortraitOrientation,
        holding: NativePortraitOrientation,
        scanout: NativePortraitOrientation,
        raw_rotation: u32,
    ) {
        use std::io::{Read, Write};
        use std::os::unix::fs::OpenOptionsExt;

        let path = self.inner.resume_trace_path.try_borrow().ok().and_then(|path| path.clone());
        let Some(path) = path else { return; };
        let name = |direction| match direction {
            NativePortraitOrientation::Up => "U",
            NativePortraitOrientation::Down => "D",
        };
        let mut uptime_text = String::new();
        let uptime = std::fs::File::open("/proc/uptime").ok()
            .and_then(|file| file.take(128).read_to_string(&mut uptime_text).ok())
            .and_then(|_| uptime_text.split_whitespace().next()?.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map_or_else(|| "null".to_owned(), |value| value.to_string());
        let record = format!(
            "{{\"event\":\"native-live-portrait\",\"process_id\":{},\"uptime_seconds\":{},\"pose_source\":\"evdev-native-portrait\",\"old_holding_direction\":\"{}\",\"holding_direction\":\"{}\",\"scanout_direction\":\"{}\",\"raw_rotation\":{},\"render_offset\":{},\"native_touch_offset\":{},\"input_generation\":{}}}\n",
            std::process::id(), uptime, name(old_holding), name(holding), name(scanout), raw_rotation,
            self.native_render_offset().unwrap_or(0), self.native_touch_offset(), self.input_generation(),
        );
        if let Err(error) = std::fs::OpenOptions::new().create(true).append(true).mode(0o600)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path)
            .and_then(|mut file| {
                if !file.metadata()?.is_file() {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput,
                        "live portrait trace must be a regular file"));
                }
                file.write_all(record.as_bytes())
            })
        {
            log::warn!("live portrait trace could not be appended: {error}");
        }
    }

    pub(crate) fn native_render_offset(&self) -> Option<u32> {
        self.inner
            .native_portrait_orientation
            .get()
            .map(|orientation| {
                let holding_offset = orientation.render_offset();
                self.inner.native_scanout_orientation.get().map_or(
                    holding_offset,
                    |scanout| (holding_offset + 360 - scanout.render_offset()) % 360,
                )
            })
    }

    pub(crate) fn native_touch_offset(&self) -> u32 {
        if self.inner.native_scanout_orientation.get().is_some() {
            self.inner
                .native_portrait_orientation
                .get()
                .map_or(0, NativePortraitOrientation::render_offset)
        } else {
            0
        }
    }

    /// Select an optional best-effort JSONL destination for resumed-frame timing.
    ///
    /// This stores only the path and performs no file I/O. It does not retain an
    /// application callback or UI reference. Changing or disabling the path
    /// cancels a pending receipt; a later successful resume can arm a new one.
    pub fn set_resume_trace_path(&self, path: Option<PathBuf>) -> Result<(), PlatformError> {
        if self.inner.quit.load(Ordering::SeqCst) {
            return Err(failure("the Kindle event loop has terminated"));
        }
        let mut current = self
            .inner
            .resume_trace_path
            .try_borrow_mut()
            .map_err(|_| failure("resume trace configuration is already borrowed"))?;
        if *current != path {
            self.inner.resume_frame_pending.set(false);
            *current = path;
        }
        Ok(())
    }

    /// Compensate native output polarity at the framebuffer write boundary.
    ///
    /// This does not change the system EPDC mode or the application's logical
    /// colors. It can be called before the event loop starts or while suspended:
    /// no framebuffer or input device is opened, no pixels are written, and no
    /// input is acquired. A change requests the existing complete scene rewrite
    /// and GC16 path on the next active iteration. Repeating the current value
    /// schedules no additional work. Calls after event-loop termination fail.
    pub fn set_output_inverted(&self, enabled: bool) -> Result<(), PlatformError> {
        if self.inner.quit.load(Ordering::SeqCst) {
            return Err(failure("the Kindle event loop has terminated"));
        }
        if self.inner.output_inverted.get() == enabled {
            return Ok(());
        }
        let _transition = self.inner.state.enter_transition()?;
        // Acquire the fallible borrow before changing either copy of the flag.
        // A sleeping framebuffer can be configured without passing its write gate.
        let mut framebuffer = self
            .inner
            .framebuffer
            .try_borrow_mut()
            .map_err(|_| failure("framebuffer is already borrowed"))?;
        if let Some(framebuffer) = framebuffer.as_mut() {
            framebuffer.set_output_inverted(enabled);
        }
        self.inner.output_inverted.set(enabled);
        self.inner.live_input_drain_waiting.set(false);
        self.inner.state.full_redraw.set(true);
        self.inner.window.request_redraw();
        crate::wakeup::poke(&self.inner.wake);
        Ok(())
    }

    /// Yield input and display ownership without terminating the event loop.
    ///
    /// The existing panel-completion ioctl remains best effort. Success is an
    /// input-release/display-gate acknowledgement, not physical panel evidence.
    pub fn suspend(&self) -> Result<(), PlatformError> {
        if self.inner.quit.load(Ordering::SeqCst) {
            return Err(failure("the Kindle event loop has terminated"));
        }
        self.inner.resume_frame_pending.set(false);
        let result = (|| {
            let mut observer = self.inner.portrait_observer.try_borrow_mut()
                .map_err(|_| failure("portrait observer is already borrowed"))?;
            let transition = self
                .inner
                .state
                .suspend_with(&self.inner.pending, |input| {
                    input.cancel_for_suspend(&self.inner.window)
                });
            if self.is_suspended() {
                observer.take();
                self.inner.portrait_generation.set(self.inner.portrait_generation.get().wrapping_add(1));
                self.inner.portrait_observer_attempted.set(true);
                self.inner.live_portrait_repaint_pending.set(false);
                self.inner.live_input_drain_waiting.set(false);
            }
            drop(observer);
            let _transition = transition?;
            if !self.inner.sleep_settled.get() {
                let framebuffer = self
                    .inner
                    .framebuffer
                    .try_borrow()
                    .map_err(|_| failure("framebuffer is already borrowed"))?;
                if let Some(framebuffer) = framebuffer.as_ref() {
                    framebuffer.wait_for_update_complete();
                }
                self.inner.sleep_settled.set(true);
            }
            Ok(())
        })();
        crate::wakeup::poke(&self.inner.wake);
        result
    }

    /// Reacquire fresh input and schedule a complete scene rewrite plus GC16.
    ///
    /// The rewrite runs on the event-loop thread immediately after control
    /// closures, before ordinary timers or input. This method does not invent a
    /// panel-completion receipt. Before the loop starts, device opening remains
    /// deferred until its first active iteration.
    pub fn resume(&self) -> Result<(), PlatformError> {
        self.resume_with_input(|| {
            self.inner
                .geometry
                .get()
                .map(|(width, height)| {
                    TouchInput::open(width, height).map_err(|error| {
                        PlatformError::Other(format!("failed to reacquire touch input: {error}"))
                    })
                })
                .transpose()
        })
    }

    fn resume_with_input(
        &self,
        open: impl FnOnce() -> Result<Option<TouchInput>, PlatformError>,
    ) -> Result<(), PlatformError> {
        if self.inner.quit.load(Ordering::SeqCst) {
            return Err(failure("the Kindle event loop has terminated"));
        }
        let was_suspended = self.is_suspended();
        let mut observer = self.inner.portrait_observer.try_borrow_mut()
            .map_err(|_| failure("portrait observer is already borrowed"))?;
        let _transition = self.inner.state.resume_with(&self.inner.pending, open)?;
        if was_suspended {
            observer.take();
            self.inner.portrait_generation.set(self.inner.portrait_generation.get().wrapping_add(1));
            self.inner.portrait_observer_attempted.set(false);
            self.inner.live_input_drain_waiting.set(false);
            self.inner
                .resume_frame_pending
                .set(self.inner.resume_trace_path.borrow().is_some());
        }
        drop(observer);
        self.inner.sleep_settled.set(false);
        self.inner.window.request_redraw();
        crate::wakeup::poke(&self.inner.wake);
        Ok(())
    }

    pub(crate) fn suspended_flag(&self) -> Arc<AtomicBool> {
        self.inner.state.suspended.clone()
    }

    /// No framebuffer is opened and no touch is grabbed while initially asleep.
    pub(crate) fn ensure_active_io(&self) -> Result<Option<(u32, u32)>, PlatformError> {
        let Some(geometry) = self.ensure_framebuffer_with(Framebuffer::open)? else {
            return Ok(None);
        };
        let mut input = self
            .inner
            .state
            .input
            .try_borrow_mut()
            .map_err(|_| failure("touch input is already borrowed"))?;
        if input.is_none() {
            *input = Some(TouchInput::open(geometry.0, geometry.1).map_err(|error| {
                PlatformError::Other(format!("failed to open touch input: {error}"))
            })?);
            self.inner
                .state
                .input_generation
                .set(self.inner.state.input_generation.get().wrapping_add(1));
        }
        Ok(Some(geometry))
    }

    fn ensure_framebuffer_with(
        &self,
        open: impl FnOnce() -> std::io::Result<Framebuffer>,
    ) -> Result<Option<(u32, u32)>, PlatformError> {
        if self.is_suspended() {
            return Ok(None);
        }
        if self.inner.geometry.get().is_none() {
            let mut framebuffer = open().map_err(|error| {
                PlatformError::Other(format!("failed to open /dev/fb0: {error}"))
            })?;
            // Install the stored polarity before the first clear or scene write.
            framebuffer.set_output_inverted(self.inner.output_inverted.get());
            let geometry = (framebuffer.width, framebuffer.height);
            *self
                .inner
                .framebuffer
                .try_borrow_mut()
                .map_err(|_| failure("framebuffer is already borrowed"))? = Some(framebuffer);
            self.inner.geometry.set(Some(geometry));
            self.inner.live_input_drain_waiting.set(false);
            self.inner.state.full_redraw.set(true);
        }
        Ok(Some(
            self.inner
                .geometry
                .get()
                .expect("opened framebuffer has geometry"),
        ))
    }

    pub(crate) fn touch_poll_snapshot(&self) -> (libc::c_int, u64) {
        let generation = self.inner.state.input_generation.get();
        if self.is_suspended() {
            return (-1, generation);
        }
        (
            self.inner
                .state
                .input
                .borrow()
                .as_ref()
                .map_or(-1, TouchInput::fd),
            generation,
        )
    }

    pub(crate) fn input_generation(&self) -> u64 {
        self.inner.state.input_generation.get()
    }

    pub(crate) fn poll_touch(&self, rotation: u32) {
        if self.is_suspended() || self.needs_full_redraw() {
            return;
        }
        if let Some(input) = self.inner.state.input.borrow_mut().as_mut() {
            input.poll(&self.inner.window, rotation);
        }
    }

    pub(crate) fn needs_full_redraw(&self) -> bool {
        self.inner.state.full_redraw.get()
    }

    /// Pixels are already submitted; wait on input/control instead of redrawing.
    pub(crate) fn live_input_drain_waiting(&self) -> bool {
        self.inner.live_input_drain_waiting.get()
    }

    /// Called only after pixels and their GC16 update have been submitted.
    pub(crate) fn complete_full_redraw(&self) -> Result<(), PlatformError> {
        self.complete_full_redraw_with_trace(crate::resume_trace::append_resume_frame)
    }

    fn complete_full_redraw_with_trace(
        &self,
        emit: impl FnOnce(&Path, bool) -> std::io::Result<()>,
    ) -> Result<(), PlatformError> {
        // Consume before logging: a failed diagnostic must never be retried on
        // an unrelated later frame, or alter the display/input acknowledgment.
        if self.inner.resume_frame_pending.replace(false)
            && !self.inner.quit.load(Ordering::SeqCst)
            && !self.is_suspended()
        {
            let path = self
                .inner
                .resume_trace_path
                .try_borrow()
                .ok()
                .and_then(|path| path.clone());
            if let Some(path) = path {
                if let Err(error) = emit(&path, self.inner.output_inverted.get()) {
                    log::warn!("resumed frame diagnostic could not be appended: {error}");
                }
            }
        }
        self.finish_full_redraw_input_with(|input, live| {
            if live { input.drain_for_reorientation() } else { input.drain_pending() }
        })
    }

    fn finish_full_redraw_input_with(
        &self,
        drain: impl FnOnce(&mut TouchInput, bool) -> std::io::Result<()>,
    ) -> Result<(), PlatformError> {
        let live = self.inner.live_portrait_repaint_pending.get();
        if let Some(input) = self
            .inner
            .state
            .input
            .try_borrow_mut()
            .map_err(|_| failure("touch input is already borrowed"))?
            .as_mut()
        {
            match drain(input, live) {
                Ok(()) => {},
                Err(error) if live && error.kind() == std::io::ErrorKind::WouldBlock => {
                    self.inner.live_input_drain_waiting.set(true);
                    return Ok(());
                },
                Err(error) => return Err(PlatformError::Other(format!("failed to discard wake input: {error}"))),
            }
        }
        self.inner.live_portrait_repaint_pending.set(false);
        self.inner.live_input_drain_waiting.set(false);
        self.inner.state.full_redraw.set(false);
        Ok(())
    }

    pub(crate) fn with_framebuffer<R>(
        &self,
        action: impl FnOnce(&mut Framebuffer) -> R,
    ) -> Result<R, PlatformError> {
        if self.is_suspended() {
            return Err(failure("the Kindle display is suspended"));
        }
        let mut framebuffer = self
            .inner
            .framebuffer
            .try_borrow_mut()
            .map_err(|_| failure("framebuffer is already borrowed"))?;
        let framebuffer = framebuffer
            .as_mut()
            .ok_or_else(|| failure("the Kindle framebuffer is not initialized"))?;
        Ok(action(framebuffer))
    }

    pub(crate) fn release_event_loop_io(&self) {
        self.inner.resume_frame_pending.set(false);
        self.inner.resume_trace_path.borrow_mut().take();
        self.inner.state.suspended.store(true, Ordering::SeqCst);
        self.inner.portrait_observer.borrow_mut().take();
        self.inner.portrait_generation.set(self.inner.portrait_generation.get().wrapping_add(1));
        self.inner.portrait_observer_attempted.set(true);
        self.inner.live_portrait_repaint_pending.set(false);
        self.inner.live_input_drain_waiting.set(false);
        self.inner.state.input.borrow_mut().take();
        self.inner.framebuffer.borrow_mut().take();
    }
}

#[cfg(test)]
mod resume_trace_tests;

#[cfg(test)]
mod orientation_tests;

#[cfg(test)]
mod live_portrait_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::software_renderer::RepaintBufferType;
    use std::os::fd::AsRawFd;
    use std::sync::atomic::AtomicUsize;

    fn output_handle() -> (DisplayLifecycleHandle, crate::wakeup::Wakeup) {
        let wake = crate::wakeup::make_wakeup().unwrap();
        let handle = DisplayLifecycleHandle::new(
            MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer),
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
            // SAFETY: The ordinary nonblocking pipe and buffer remain valid.
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
        assert!(handle.inner.state.input.borrow().is_none());
    }

    #[test]
    fn output_polarity_can_be_staged_before_native_io_and_precedes_the_initial_fill() {
        let (handle, wake) = output_handle();
        handle.set_output_inverted(true).unwrap();
        assert!(handle.inner.output_inverted.get());
        assert!(handle.inner.framebuffer.borrow().is_none());
        assert_eq!(handle.inner.geometry.get(), None);
        assert!(handle.inner.state.input.borrow().is_none());
        assert_eq!(handle.input_generation(), 0);
        assert_eq!(take_wake_bytes(&wake), 1);

        install_memory_framebuffer(&handle);
        handle
            .ensure_framebuffer_with(|| panic!("the installed mapping must be reused"))
            .unwrap();
        handle
            .with_framebuffer(|framebuffer| {
                assert!(
                    framebuffer
                        .mapped_bytes_for_test()
                        .iter()
                        .all(|&value| value == 0x5a)
                );
                framebuffer.fill(0xff);
                assert_eq!(
                    framebuffer.mapped_bytes_for_test(),
                    &[0, 0, 0, 0x5a, 0x5a, 0, 0, 0, 0x5a, 0x5a]
                );
            })
            .unwrap();
    }

    #[test]
    fn a_rejected_retained_layout_probe_changes_no_mapping_pixels_or_sleep_state() {
        let (startup, startup_wake) = output_handle();
        assert_eq!(
            startup
                .probe_output_inversion_with(|geometry, retained| {
                    assert_eq!(geometry, None);
                    assert_eq!(retained, None);
                    Ok(Some(false))
                })
                .unwrap(),
            Some(false)
        );
        assert!(startup.inner.framebuffer.borrow().is_none());
        assert!(startup.inner.state.input.borrow().is_none());
        assert_eq!(take_wake_bytes(&startup_wake), 0);

        let (handle, wake) = output_handle();
        install_memory_framebuffer(&handle);
        handle.set_output_inverted(true).unwrap();
        handle
            .with_framebuffer(|framebuffer| framebuffer.fill(0x20))
            .unwrap();
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
        let before = handle
            .inner
            .framebuffer
            .borrow()
            .as_ref()
            .unwrap()
            .mapped_bytes_for_test()
            .to_vec();
        let mapping = handle
            .inner
            .framebuffer
            .borrow()
            .as_ref()
            .unwrap()
            .mapped_bytes_for_test()
            .as_ptr();
        let geometry = handle.inner.geometry.get();
        let generation = handle.input_generation();
        let redraw = handle.needs_full_redraw();
        let sleep_settled = handle.inner.sleep_settled.get();
        for reason in ["stride changed", "pixel depth changed"] {
            let error = handle
                .probe_output_inversion_with(|observed_geometry, retained| {
                    assert_eq!(observed_geometry, geometry);
                    assert_eq!(
                        retained,
                        Some(FramebufferLayout {
                            width: 3,
                            height: 2,
                            stride: 5,
                            bytes_per_pixel: 1,
                            bits_per_pixel: 8,
                        })
                    );
                    assert!(handle.inner.framebuffer.try_borrow_mut().is_ok());
                    Err(std::io::Error::new(std::io::ErrorKind::InvalidData, reason))
                })
                .unwrap_err();
            assert!(error.to_string().contains(reason));
            let framebuffer = handle.inner.framebuffer.borrow();
            let framebuffer = framebuffer.as_ref().unwrap();
            assert_eq!(framebuffer.mapped_bytes_for_test().as_ptr(), mapping);
            assert_eq!(framebuffer.mapped_bytes_for_test(), before);
            assert_eq!(handle.inner.geometry.get(), geometry);
            assert!(handle.inner.output_inverted.get());
            assert!(handle.is_suspended());
            assert_eq!(handle.input_generation(), generation);
            assert!(handle.inner.state.input.borrow().is_none());
            assert_eq!(handle.needs_full_redraw(), redraw);
            assert_eq!(handle.inner.sleep_settled.get(), sleep_settled);
            assert_eq!(take_wake_bytes(&wake), 0);
        }
    }

    #[test]
    fn initially_sleeping_output_changes_keep_native_io_deferred() {
        let (handle, wake) = output_handle();
        handle.suspend().unwrap();
        let input_generation = handle.input_generation();
        take_wake_bytes(&wake);
        handle.set_output_inverted(true).unwrap();
        assert!(handle.is_suspended());
        assert_eq!(handle.ensure_active_io().unwrap(), None);
        assert_eq!(
            handle
                .ensure_framebuffer_with(|| panic!("sleep must not open a mapping"))
                .unwrap(),
            None
        );
        assert!(handle.inner.framebuffer.borrow().is_none());
        assert_eq!(handle.inner.geometry.get(), None);
        assert!(handle.inner.state.input.borrow().is_none());
        assert_eq!(handle.touch_poll_snapshot(), (-1, input_generation));
        assert!(handle.inner.sleep_settled.get());
        assert_eq!(take_wake_bytes(&wake), 1);

        handle.resume().unwrap();
        install_memory_framebuffer(&handle);
        handle
            .with_framebuffer(|framebuffer| {
                framebuffer.write_line(0, 0..3, &[0, 127, 255]);
                assert_eq!(&framebuffer.mapped_bytes_for_test()[..3], &[255, 128, 0]);
            })
            .unwrap();
    }

    #[test]
    fn output_changes_in_both_directions_require_a_complete_scene_rewrite() {
        let (handle, wake) = output_handle();
        install_memory_framebuffer(&handle);
        handle
            .with_framebuffer(|framebuffer| framebuffer.fill(0x20))
            .unwrap();
        let input_generation = handle.input_generation();
        for enabled in [true, false] {
            handle.complete_full_redraw().unwrap();
            take_wake_bytes(&wake);
            let before = handle
                .inner
                .framebuffer
                .borrow()
                .as_ref()
                .unwrap()
                .mapped_bytes_for_test()
                .to_vec();
            handle.set_output_inverted(enabled).unwrap();
            assert!(handle.needs_full_redraw());
            assert_eq!(handle.inner.output_inverted.get(), enabled);
            assert!(!handle.is_suspended());
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
            handle
                .with_framebuffer(|framebuffer| {
                    framebuffer.fill(0x20);
                    let expected = if enabled { 0xdf } else { 0x20 };
                    assert_eq!(&framebuffer.mapped_bytes_for_test()[..3], &[expected; 3]);
                })
                .unwrap();
        }
    }

    #[test]
    fn repeated_output_polarity_is_a_noop_even_with_a_borrowed_framebuffer() {
        let (handle, wake) = output_handle();
        install_memory_framebuffer(&handle);
        handle.set_output_inverted(true).unwrap();
        handle.complete_full_redraw().unwrap();
        take_wake_bytes(&wake);
        let _borrow = handle.inner.framebuffer.borrow_mut();
        handle.set_output_inverted(true).unwrap();
        assert!(!handle.needs_full_redraw());
        assert_eq!(take_wake_bytes(&wake), 0);
    }

    #[test]
    fn sleeping_output_changes_configure_the_mapping_without_touching_its_pixels() {
        let (handle, wake) = output_handle();
        install_memory_framebuffer(&handle);
        handle
            .with_framebuffer(|framebuffer| framebuffer.fill(0x20))
            .unwrap();
        let before = handle
            .inner
            .framebuffer
            .borrow()
            .as_ref()
            .unwrap()
            .mapped_bytes_for_test()
            .to_vec();
        // The pure transition publishes the same gate without issuing an ioctl.
        drop(
            handle
                .inner
                .state
                .suspend_with(&handle.inner.pending, |_| {
                    panic!("the memory fixture has no native input")
                })
                .unwrap(),
        );
        let input_generation = handle.input_generation();
        take_wake_bytes(&wake);
        handle.set_output_inverted(true).unwrap();
        assert!(handle.is_suspended());
        assert!(handle.needs_full_redraw());
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
        // Model the completed external handoff without acquiring real input.
        drop(
            handle
                .inner
                .state
                .resume_with(&handle.inner.pending, || Ok(None))
                .unwrap(),
        );
        handle
            .with_framebuffer(|framebuffer| {
                framebuffer.fill(0x20);
                assert_eq!(&framebuffer.mapped_bytes_for_test()[..3], &[0xdf; 3]);
            })
            .unwrap();
    }

    #[test]
    fn an_output_change_borrow_failure_preserves_both_flags_and_redraw_state() {
        let (handle, wake) = output_handle();
        install_memory_framebuffer(&handle);
        handle.complete_full_redraw().unwrap();
        take_wake_bytes(&wake);
        let borrow = handle.inner.framebuffer.borrow();
        assert!(handle.set_output_inverted(true).is_err());
        assert!(!handle.inner.output_inverted.get());
        assert!(!handle.needs_full_redraw());
        assert!(!handle.inner.state.transition_in_progress.get());
        assert_eq!(take_wake_bytes(&wake), 0);
        drop(borrow);
        handle
            .with_framebuffer(|framebuffer| {
                framebuffer.fill(0x20);
                assert_eq!(&framebuffer.mapped_bytes_for_test()[..3], &[0x20; 3]);
            })
            .unwrap();
        handle.set_output_inverted(true).unwrap();
        assert!(handle.needs_full_redraw());
    }

    #[test]
    fn output_changes_reject_reentrant_transitions_and_post_quit_calls() {
        let (handle, wake) = output_handle();
        handle.complete_full_redraw().unwrap();
        let transition = handle.inner.state.enter_transition().unwrap();
        assert!(handle.set_output_inverted(true).is_err());
        assert!(!handle.inner.output_inverted.get());
        assert!(!handle.needs_full_redraw());
        drop(transition);
        handle.inner.quit.store(true, Ordering::SeqCst);
        for enabled in [false, true] {
            assert!(handle.set_output_inverted(enabled).is_err());
        }
        assert!(!handle.inner.output_inverted.get());
        assert!(!handle.needs_full_redraw());
        assert!(handle.inner.framebuffer.borrow().is_none());
        assert!(handle.inner.state.input.borrow().is_none());
        assert_eq!(take_wake_bytes(&wake), 0);
    }

    struct FakeInput(Arc<AtomicUsize>);
    impl Drop for FakeInput {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn suspend_releases_input_before_returning_and_blocks_reentry_work() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let state = LifecycleState::new();
        *state.input.borrow_mut() = Some(FakeInput(dropped.clone()));
        let pending = Mutex::new(PendingRefresh::default());
        state
            .suspend_with(&pending, |_| {
                assert!(state.suspended.load(Ordering::SeqCst));
                assert_eq!(dropped.load(Ordering::SeqCst), 0);
                Ok(())
            })
            .unwrap();
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert!(state.input.borrow().is_none());
        assert!(state.full_redraw.get());
        state
            .suspend_with(&pending, |_| panic!("idempotent suspend has no input"))
            .unwrap();
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failed_reacquisition_stays_suspended_and_success_requires_a_full_rewrite() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let state = LifecycleState::<FakeInput>::new();
        let pending = Mutex::new(PendingRefresh::default());
        state.suspend_with(&pending, |_| Ok(())).unwrap();
        assert!(
            state
                .resume_with(&pending, || Err(failure("fixture input unavailable")))
                .is_err()
        );
        assert!(state.suspended.load(Ordering::SeqCst));
        assert!(state.input.borrow().is_none());
        state
            .resume_with(&pending, || Ok(Some(FakeInput(dropped.clone()))))
            .unwrap();
        assert!(!state.suspended.load(Ordering::SeqCst));
        assert!(state.full_redraw.get());
        assert!(state.input.borrow().is_some());
        state
            .resume_with(&pending, || {
                panic!("idempotent resume cannot grab a second input")
            })
            .unwrap();
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn an_initially_suspended_loop_can_wait_without_opening_devices() {
        let state = LifecycleState::<FakeInput>::new();
        let pending = Mutex::new(PendingRefresh::default());
        state
            .suspend_with(&pending, |_| panic!("there is no initial input"))
            .unwrap();
        assert!(state.suspended.load(Ordering::SeqCst));
        state.resume_with(&pending, || Ok(None)).unwrap();
        assert!(state.input.borrow().is_none());
        assert!(state.full_redraw.get());
    }

    #[test]
    fn pointer_cancel_failure_still_drops_the_exclusive_input() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let state = LifecycleState::new();
        *state.input.borrow_mut() = Some(FakeInput(dropped.clone()));
        assert!(
            state
                .suspend_with(&Mutex::new(PendingRefresh::default()), |_| Err(failure(
                    "fixture cancel failed"
                )))
                .is_err()
        );
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert!(state.suspended.load(Ordering::SeqCst));
        assert!(!state.transition_in_progress.get());
    }

    #[test]
    fn suspend_discards_old_refreshes_and_rejects_new_ones_until_resume() {
        let state = LifecycleState::<FakeInput>::new();
        let pending = Arc::new(Mutex::new(PendingRefresh::default()));
        let wake = crate::wakeup::make_wakeup().unwrap();
        let refresh = crate::refresh::RefreshHandle {
            pending: pending.clone(),
            wake: wake.write,
            quit: Arc::new(AtomicBool::new(false)),
            suspended: state.suspended.clone(),
        };
        refresh
            .request(crate::refresh::RefreshRequest::full())
            .unwrap();
        assert!(!pending.lock().unwrap().is_empty());
        state.suspend_with(&pending, |_| Ok(())).unwrap();
        assert!(pending.lock().unwrap().is_empty());
        assert_eq!(
            refresh.request(crate::refresh::RefreshRequest::full()),
            Err(crate::refresh::RefreshError::DisplaySuspended)
        );
        state.resume_with(&pending, || Ok(None)).unwrap();
        assert!(state.full_redraw.get());
        refresh
            .request(crate::refresh::RefreshRequest::full())
            .unwrap();
    }

    #[test]
    fn reentrant_cancel_cannot_acknowledge_suspend_before_the_old_input_drops() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let state = LifecycleState::new();
        *state.input.borrow_mut() = Some(FakeInput(dropped.clone()));
        let pending = Mutex::new(PendingRefresh::default());
        let transition = state
            .suspend_with(&pending, |_| {
                assert_eq!(dropped.load(Ordering::SeqCst), 0);
                assert!(state.suspend_with(&pending, |_| Ok(())).is_err());
                assert!(
                    state
                        .resume_with(&pending, || panic!(
                            "reentrant resume cannot reacquire input"
                        ))
                        .is_err()
                );
                assert!(state.suspended.load(Ordering::SeqCst));
                assert_eq!(dropped.load(Ordering::SeqCst), 0);
                Ok(())
            })
            .unwrap();
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert!(state.transition_in_progress.get());
        drop(transition);
        assert!(!state.transition_in_progress.get());
    }
}

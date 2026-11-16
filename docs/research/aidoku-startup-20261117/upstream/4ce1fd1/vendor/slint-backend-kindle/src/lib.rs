//! Slint platform backend for Kindles.
//!
//! # Usage
//!
//! ```no_run
//! slint::include_modules!();
//!
//! static DEFAULT_FONT: &[u8] = include_bytes!("../ui/MyFont.ttf");
//! static SERIF_FONT: &[u8] = include_bytes!("../ui/MySerif.ttf");
//!
//! fn main() {
//!     let backend = slint_backend_kindle::install(DEFAULT_FONT)
//!         .expect("failed to install Kindle backend");
//!     let app = AppWindow::new().expect("failed to create window");
//!     backend.register_font_from_memory(SERIF_FONT).expect("failed to register font");
//!     app.run().expect("event loop error");
//! }
//! ```

mod clipboard;
mod dithering;
mod frame_observer;
mod framebuffer;
mod gray_target;
mod input_discovery;
mod lifecycle;
mod platform;
mod portrait;
mod portrait_events;
mod portrait_pose;
mod portrait_runtime;
mod power;
mod refresh;
mod render_control;
mod sleep_presentation;
mod splash;
mod touch;
mod wakeup;

pub use dithering::DitheringHandle;
pub use frame_observer::{NativeFrameObservation, NativeFramePhase, NativeFrameUnavailable};
pub use framebuffer::{NativeFramebufferObservation, probe_native_framebuffer};
pub use lifecycle::{LifecycleError, LifecycleHandle, LifecycleStatus};
pub use portrait::NativePortraitOrientation;
pub use portrait_pose::{
    NativePortraitSensorObservation, observe_native_portrait_sensor, read_native_portrait_pose,
};
pub use refresh::{
    PhysicalRect, RefreshArea, RefreshError, RefreshHandle, RefreshRequest, RefreshWaveform,
    SwipeDirection,
};
pub use render_control::{RenderError, RenderHandle};
pub use splash::{StartupImage, StartupImages, StartupSplash};

use platform::KindlePlatform;
use slint::platform::WindowAdapter;
use slint::platform::software_renderer::MinimalSoftwareWindow;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) type OnWakeCallback = Rc<RefCell<Option<Box<dyn FnMut()>>>>;
pub(crate) type InputObserver =
    Rc<RefCell<Option<Box<dyn FnMut(&slint::platform::WindowEvent) -> bool>>>>;
pub(crate) type ControlCallback = Rc<RefCell<Option<Box<dyn FnMut()>>>>;

/// How often to wake from suspend-to-RAM and how long to stay awake afterwards.
///
/// Pass to [`KindleBackend::set_wake_schedule`] to opt in. Without it, the
/// backend never suspends the SoC — the event loop just blocks in `poll(2)`,
/// which is fine for plugged-in use but burns battery.
///
/// Touch activity during the awake window resets `stay_awake`, exactly like
/// the device's normal idle timer.
#[derive(Debug, Clone, Copy)]
pub struct WakeSchedule {
    /// Time between scheduled wakes from suspend.
    pub wake_interval: Duration,
    /// How long to stay awake after a wake or the last touch.
    pub stay_awake: Duration,
}

/// Typestate markers
pub struct NoSchedule;
pub struct Scheduled;

/// Returned by [`install`]. Use it to add more fonts and configure power.
///
/// A new backend is [`NoSchedule`]. Calling
/// [`set_wake_schedule`](KindleBackend::set_wake_schedule) turns it into a
/// [`Scheduled`] one, and only that form has
/// [`on_wake`](KindleBackend::on_wake) — so you can't set a wake callback
/// without first setting up a wake schedule.
pub struct KindleBackend<State = NoSchedule> {
    window: Rc<MinimalSoftwareWindow>,
    wake_schedule: Arc<Mutex<Option<WakeSchedule>>>,
    on_wake: OnWakeCallback,
    input_observer: InputObserver,
    control_callback: ControlCallback,
    frame_observer: Rc<frame_observer::FrameObserverState>,
    black_and_white: Arc<AtomicBool>,
    rotation: Arc<AtomicU32>,
    refresh_handle: RefreshHandle,
    render_handle: RenderHandle,
    dithering_handle: DitheringHandle,
    lifecycle_handle: LifecycleHandle,
    _state: PhantomData<State>,
}

impl<State> KindleBackend<State> {
    /// Observe one armed Reader output without changing rendering or refreshes.
    /// The callback runs on the UI thread after a write or accepted SEND. It
    /// must not mutate UI models or render recursively. No callback borrow or
    /// mutex remains held during invocation; observation performs no file I/O.
    pub fn set_frame_observer(&self, observer: impl Fn(NativeFrameObservation) + 'static) {
        self.frame_observer.set_observer(observer);
    }

    /// Arm the next output for an accepted Reader image. Returns an unconsumed
    /// replaced token. The application must cancel on navigation or overlays.
    pub fn arm_reader_frame(&self, token: u64) -> Option<u64> {
        self.frame_observer.arm(token)
    }

    /// Retire an unconsumed token and invalidate any active observation attempt.
    pub fn cancel_reader_frame(&self) -> Option<u64> {
        self.frame_observer.cancel()
    }

    /// Receive signal-pipe/control notifications even while the normal UI is parked.
    pub fn set_control_callback(&self, callback: impl FnMut() + 'static) {
        *self.control_callback.borrow_mut() = Some(Box::new(callback));
    }

    pub fn lifecycle_handle(&self) -> LifecycleHandle {
        self.lifecycle_handle.clone()
    }

    /// Observe native logical pointer events before Slint dispatches to children.
    /// Return true to consume an event. The callback runs on the UI thread;
    /// direct Window::try_dispatch_event calls do not pass through this hook.
    pub fn set_input_observer(
        &self,
        observer: impl FnMut(&slint::platform::WindowEvent) -> bool + 'static,
    ) {
        *self.input_observer.borrow_mut() = Some(Box::new(observer));
    }

    /// Queue display requests for execution after the current frame is drawn.
    pub fn refresh_handle(&self) -> RefreshHandle {
        self.refresh_handle.clone()
    }

    /// Control application rendering without changing system display preferences.
    pub fn render_handle(&self) -> RenderHandle {
        self.render_handle.clone()
    }

    /// Bind hardware processing to the Gray8 image already in the scene.
    pub fn dithering_handle(&self) -> DitheringHandle {
        self.dithering_handle.clone()
    }

    /// Add an extra font (TTF/OTF) from bytes.
    ///
    /// Call this **after** you've created your window (e.g. `AppWindow::new()`).
    /// Fonts can't be added before then because Slint hasn't set up its font
    /// system yet.
    pub fn register_font_from_memory(
        &self,
        data: &'static [u8],
    ) -> Result<(), slint::PlatformError> {
        self.window
            .renderer()
            .register_font_from_memory(data)
            .map_err(|e| slint::PlatformError::Other(format!("{e}")))
    }

    /// Render in **pure black and white** (bilevel) mode: force every pixel to
    /// pure black or white, with no grey levels at all. Useful on devices where
    /// greyscale rendering causes a flicker through black to be displayed.
    ///
    /// Off by default. A change takes effect on the next render, so set it
    /// before your first window draw, toggling it later only affects pixels
    /// redrawn after that.
    pub fn set_black_and_white(&self, enabled: bool) {
        self.black_and_white.store(enabled, Ordering::Relaxed);
    }

    /// Rotate the UI by 0, 90, 180 or 270 degrees relative to the starting orientation.
    pub fn set_rotation(&self, degrees: u32) {
        assert!(
            degrees.is_multiple_of(90),
            "rotation must be a multiple of 90 degrees, got {degrees}"
        );
        self.rotation.store(degrees % 360, Ordering::Relaxed);
        crate::wakeup::poke(&self.lifecycle_handle.wake);
    }

    /// Switch state, keeping the same internals.
    fn into_state<Next>(self) -> KindleBackend<Next> {
        KindleBackend {
            window: self.window,
            wake_schedule: self.wake_schedule,
            on_wake: self.on_wake,
            input_observer: self.input_observer,
            control_callback: self.control_callback,
            frame_observer: self.frame_observer,
            black_and_white: self.black_and_white,
            rotation: self.rotation,
            refresh_handle: self.refresh_handle,
            render_handle: self.render_handle,
            dithering_handle: self.dithering_handle,
            lifecycle_handle: self.lifecycle_handle,
            _state: PhantomData,
        }
    }
}

impl KindleBackend<NoSchedule> {
    /// Turn on the wake-from-suspend cycle.
    ///
    /// The device sleeps once your app has been idle for `stay_awake`, then
    /// wakes every `wake_interval` (or earlier, e.g. on a button press) so it
    /// can refresh.
    ///
    /// Returns a [`Scheduled`] backend that lets you set
    /// [`on_wake`](KindleBackend::on_wake).
    pub fn set_wake_schedule(self, schedule: WakeSchedule) -> KindleBackend<Scheduled> {
        *self.wake_schedule.lock().expect("wake schedule poisoned") = Some(schedule);
        self.into_state()
    }
}

impl KindleBackend<Scheduled> {
    /// Change the wake schedule.
    ///
    /// The new schedule takes effect the next time the device is awake (it
    /// can't be reached while asleep).
    pub fn set_wake_schedule(&self, schedule: WakeSchedule) {
        *self.wake_schedule.lock().expect("wake schedule poisoned") = Some(schedule);
    }

    /// Turn the wake cycle back off.
    ///
    /// Forgets the [`on_wake`](KindleBackend::on_wake) callback, since it can't
    /// fire anymore. Takes effect the next time the device is awake.
    pub fn clear_wake_schedule(self) -> KindleBackend<NoSchedule> {
        *self.wake_schedule.lock().expect("wake schedule poisoned") = None;
        *self.on_wake.borrow_mut() = None;
        self.into_state()
    }

    /// Run `callback` once each time the device wakes from a scheduled suspend.
    ///
    /// Fires on the event-loop (UI) thread, after resume but before the next
    /// render. The right place to refresh state that should be current when
    /// the screen redraws after waking up, like polling an HTTP API, reading a sensor, etc.
    /// Don't rely on a `slint::Timer` to align with `wake_interval`; Slint timers
    /// run on their own schedule and may fire before or after the wake.
    ///
    /// Replaces any previously-set callback. Not invoked on the initial start of the app.
    pub fn on_wake<F: FnMut() + 'static>(&self, callback: F) {
        *self.on_wake.borrow_mut() = Some(Box::new(callback));
    }
}

/// Set up the Kindle backend and use `font_data` as the default font.
///
/// You **must** pass a font. The Kindle doesn't ship any usable system fonts,
/// so without one Slint will crash the first time it tries to draw text.
/// We write the font to a temp file and point Slint at it through an
/// environment variable so it gets used everywhere a font is needed.
///
/// Call this once at startup, before creating any windows. Use the returned
/// [`KindleBackend`] to add more fonts later.
///
/// # Errors
///
/// Fails if the temp file can't be written, or if Slint already has a
/// platform set up.
pub fn install(font_data: &[u8]) -> Result<KindleBackend, slint::PlatformError> {
    let path = std::env::temp_dir().join("slint-kindle-default.ttf");
    std::fs::write(&path, font_data)
        .map_err(|e| slint::PlatformError::Other(format!("failed to stage default font: {e}")))?;

    // SAFETY: install() runs once at startup while no other threads are alive,
    // so nothing else can read this environment variable at the same time.
    unsafe {
        std::env::set_var("SLINT_DEFAULT_FONT", &path);
    }

    let wake_schedule = Arc::new(Mutex::new(None));
    let on_wake: OnWakeCallback = Rc::new(RefCell::new(None));
    let input_observer: InputObserver = Rc::new(RefCell::new(None));
    let control_callback: ControlCallback = Rc::new(RefCell::new(None));
    let frame_observer = Rc::new(frame_observer::FrameObserverState::default());
    let black_and_white = Arc::new(AtomicBool::new(false));
    let rotation = Arc::new(AtomicU32::new(0));
    let platform = KindlePlatform::new(
        wake_schedule.clone(),
        on_wake.clone(),
        black_and_white.clone(),
        rotation.clone(),
        input_observer.clone(),
        control_callback.clone(),
        frame_observer.clone(),
    )
    .map_err(|e| slint::PlatformError::Other(format!("failed to init Kindle platform: {e}")))?;
    let window = platform.window.clone();
    let refresh_handle = platform.refresh_handle();
    let render_handle = platform.render_handle();
    let dithering_handle = platform.dithering_handle();
    let lifecycle_handle = platform.lifecycle_handle();
    slint::platform::set_platform(Box::new(platform))
        .map_err(|e| slint::PlatformError::Other(format!("{e}")))?;
    Ok(KindleBackend {
        window,
        wake_schedule,
        on_wake,
        input_observer,
        control_callback,
        frame_observer,
        black_and_white,
        rotation,
        refresh_handle,
        render_handle,
        dithering_handle,
        lifecycle_handle,
        _state: PhantomData,
    })
}

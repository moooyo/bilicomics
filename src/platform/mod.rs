mod profile;
mod refresh;
#[cfg(test)]
mod dithering_tests;

#[cfg(all(feature = "kindle", target_os = "linux"))]
mod native;
#[cfg(any(test, all(feature = "kindle", target_os = "linux")))]
mod native_capture;

pub use profile::DeviceProfile;
pub use refresh::{RefreshKind, RefreshPlan, RefreshReason, RefreshRect, RefreshScheduler};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("hardware capability is unavailable: {0}")]
    Unsupported(&'static str),
    #[error("Kindle launch was not approved by the device/firmware allowlist")]
    UnapprovedDevice,
    #[error(
        "the framebuffer geometry does not match the approved display: expected {expected:?}, actual {actual:?}"
    )]
    GeometryMismatch {
        expected: (u32, u32),
        actual: (u32, u32),
    },
    #[error("the framebuffer offset is unsupported: expected (0, 0), actual {actual:?}")]
    FramebufferOffset { actual: (u32, u32) },
    #[error("invalid hardware value: {0}")]
    InvalidValue(&'static str),
    #[error("device operation failed: {0}")]
    Operation(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(target_os = "linux")]
fn validate_native_launch_approval(profile: DeviceProfile) -> Result<(), PlatformError> {
    let firmware = std::env::var("BILICOMICS_APPROVED_FIRMWARE").unwrap_or_default();
    if std::env::var("BILICOMICS_APPROVED_PROFILE").ok().as_deref() != Some(profile.id())
        || firmware.trim().is_empty()
    {
        return Err(PlatformError::UnapprovedDevice);
    }
    Ok(())
}

fn calibrated_framebuffer_portrait(
    profile: DeviceProfile,
    approved_profile: Option<&str>,
    approved_firmware: Option<&str>,
) -> bool {
    profile == DeviceProfile::Scribe
        && approved_profile == Some("scribe")
        && approved_firmware == Some("5.19.5")
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PlatformCapabilities {
    pub brightness: bool,
    pub warmth: bool,
    pub hardware_inversion: bool,
    pub keep_awake: bool,
    pub pen: bool,
    pub explicit_refresh: bool,
}

/// Hardware details stay behind a replaceable service boundary.
pub trait DeviceControl: Send + Sync {
    /// Return the approved visible framebuffer dimensions in physical pixels.
    fn native_viewport(&self) -> Option<(u32, u32)> {
        None
    }
    fn capabilities(&self) -> PlatformCapabilities;
    fn brightness(&self) -> Result<u8, PlatformError>;
    fn set_brightness(&self, value: u8) -> Result<(), PlatformError>;
    fn warmth(&self) -> Result<u8, PlatformError>;
    fn set_warmth(&self, value: u8) -> Result<(), PlatformError>;
    fn battery_percentage(&self) -> Result<u8, PlatformError>;
    fn set_inverted(&self, enabled: bool) -> Result<(), PlatformError>;
    fn reset_idle_timer(&self) -> Result<(), PlatformError>;
}

/// A producer queue acknowledges delivery to the renderer, not panel success.
pub trait RefreshSink: Send + Sync {
    fn submit(&self, plan: RefreshPlan) -> Result<(), PlatformError>;
}

/// Configure the processing required by the image that is actually displayed.
/// Capability reflects the reviewed native path, not physical panel acceptance.
pub trait DitheringSink: Send + Sync {
    fn hardware_y4_available(&self) -> bool;
    /// Retain the requested input format even if hardware needs software fallback.
    fn set_hardware_y4(&self, enabled: bool) -> bool;
}

struct DesktopControl;

impl DeviceControl for DesktopControl {
    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities::default()
    }
    fn brightness(&self) -> Result<u8, PlatformError> {
        Err(PlatformError::Unsupported("brightness"))
    }
    fn set_brightness(&self, _: u8) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("brightness"))
    }
    fn warmth(&self) -> Result<u8, PlatformError> {
        Err(PlatformError::Unsupported("warmth"))
    }
    fn set_warmth(&self, _: u8) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("warmth"))
    }
    fn battery_percentage(&self) -> Result<u8, PlatformError> {
        Err(PlatformError::Unsupported("battery"))
    }
    fn set_inverted(&self, _: bool) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("hardware_inversion"))
    }
    fn reset_idle_timer(&self) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("keep_awake"))
    }
}

pub struct Platform {
    pub profile: DeviceProfile,
    pub is_kindle: bool,
    control: Arc<dyn DeviceControl>,
    refresh: Mutex<RefreshScheduler>,
    refresh_sink: Mutex<Option<Arc<dyn RefreshSink>>>,
    dithering_sink: Mutex<Option<Arc<dyn DitheringSink>>>,
    reader_hardware_y4: AtomicBool,
    #[cfg(all(feature = "kindle", target_os = "linux"))]
    performance_trace: Mutex<Option<slint_backend_kindle::PerformanceTraceHandle>>,
    started: Instant,
}

impl Platform {
    pub fn new(profile: DeviceProfile, is_kindle: bool) -> Result<Self, PlatformError> {
        let control: Arc<dyn DeviceControl> = if is_kindle {
            #[cfg(all(feature = "kindle", target_os = "linux"))]
            {
                Arc::new(native::NativeControl::new(profile)?)
            }
            #[cfg(not(all(feature = "kindle", target_os = "linux")))]
            {
                return Err(PlatformError::Unsupported("kindle_backend"));
            }
        } else {
            Arc::new(DesktopControl)
        };
        Ok(Self::with_control(profile, is_kindle, control))
    }

    pub fn with_control(
        profile: DeviceProfile,
        is_kindle: bool,
        control: Arc<dyn DeviceControl>,
    ) -> Self {
        Self {
            profile,
            is_kindle,
            control,
            refresh: Mutex::new(RefreshScheduler::default()),
            refresh_sink: Mutex::new(None),
            dithering_sink: Mutex::new(None),
            reader_hardware_y4: AtomicBool::new(false),
            #[cfg(all(feature = "kindle", target_os = "linux"))]
            performance_trace: Mutex::new(None),
            started: Instant::now(),
        }
    }

    pub fn capabilities(&self) -> PlatformCapabilities {
        let mut capabilities = self.control.capabilities();
        capabilities.explicit_refresh = self.refresh_sink.lock().is_ok_and(|sink| sink.is_some());
        capabilities
    }

    pub fn native_viewport_size(&self) -> Option<(u32, u32)> {
        if self.is_kindle {
            self.control.native_viewport()
        } else {
            None
        }
    }

    pub fn logical_viewport_size(&self) -> (u32, u32) {
        self.native_viewport_size()
            .map(|(width, height)| (width / 2, height / 2))
            .unwrap_or_else(|| self.profile.logical_size())
    }

    pub fn scale_factor(&self) -> f32 {
        if self.is_kindle { 2.0 } else { 1.0 }
    }

    /// Select only the calibrated native pair with observed driver scanout.
    /// The caller still authenticates its portrait observation before staging.
    pub fn uses_calibrated_framebuffer_portrait(&self) -> bool {
        self.is_kindle
            && calibrated_framebuffer_portrait(
                self.profile,
                std::env::var("BILICOMICS_APPROVED_PROFILE").ok().as_deref(),
                std::env::var("BILICOMICS_APPROVED_FIRMWARE")
                    .ok()
                    .as_deref(),
            )
    }

    pub fn brightness(&self) -> Result<u8, PlatformError> {
        self.control.brightness()
    }
    pub fn set_brightness(&self, value: u8) -> Result<(), PlatformError> {
        self.control.set_brightness(value)
    }
    pub fn warmth(&self) -> Result<u8, PlatformError> {
        self.control.warmth()
    }
    pub fn set_warmth(&self, value: u8) -> Result<(), PlatformError> {
        self.control.set_warmth(value)
    }
    pub fn battery_percentage(&self) -> Result<u8, PlatformError> {
        let value = self.control.battery_percentage()?;
        if value > 100 {
            return Err(PlatformError::InvalidValue(
                "battery percentage must be between 0 and 100",
            ));
        }
        Ok(value)
    }
    pub fn set_inverted(&self, enabled: bool) -> Result<(), PlatformError> {
        self.control.set_inverted(enabled)
    }

    pub fn plan_refresh(&self, reason: RefreshReason) -> Option<RefreshPlan> {
        self.refresh
            .lock()
            .ok()?
            .plan(reason, self.started.elapsed())
    }

    pub fn set_refresh_sink(&self, sink: Arc<dyn RefreshSink>) -> Result<(), PlatformError> {
        *self
            .refresh_sink
            .lock()
            .map_err(|_| PlatformError::Operation("refresh sink lock was poisoned".into()))? =
            Some(sink);
        Ok(())
    }

    pub fn set_dithering_sink(&self, sink: Arc<dyn DitheringSink>) -> Result<(), PlatformError> {
        sink.set_hardware_y4(self.reader_hardware_y4_requested());
        *self.dithering_sink.lock()
            .map_err(|_| PlatformError::Operation("dithering sink lock was poisoned".into()))? = Some(sink);
        Ok(())
    }

    pub fn hardware_y4_available(&self) -> bool {
        self.dithering_sink.lock().ok().and_then(|sink| sink.clone())
            .is_some_and(|sink| sink.hardware_y4_available())
    }

    /// This describes the retained frame's input format, not a pending preference.
    pub fn reader_hardware_y4_requested(&self) -> bool {
        self.reader_hardware_y4.load(Ordering::Acquire)
    }

    /// Switch together with a newly imported frame, never while it is preparing.
    pub fn set_reader_hardware_y4(&self, enabled: bool) -> bool {
        self.reader_hardware_y4.store(enabled, Ordering::Release);
        self.dithering_sink.lock().ok().and_then(|sink| sink.clone())
            .is_some_and(|sink| sink.set_hardware_y4(enabled))
    }

    /// Connect opt-in diagnostics without changing refresh or lifecycle policy.
    #[cfg(all(feature = "kindle", target_os = "linux"))]
    pub fn set_kindle_performance_trace(&self, trace: slint_backend_kindle::PerformanceTraceHandle)
        -> Result<(), PlatformError>
    {
        if !crate::performance::enabled() { return Ok(()); }
        let previous = self.performance_trace.lock()
            .map_err(|_| PlatformError::Operation("performance trace lock was poisoned".into()))?
            .replace(trace.clone());
        if let Some(previous) = previous { previous.clear_callback(); }
        native::configure_performance_trace(&trace);
        Ok(())
    }

    /// Mark the actual UI image; this does not queue a native refresh.
    /// Desktop renderers have no native framebuffer submission to correlate.
    pub fn set_reader_performance_operation(&self, operation_id: Option<u64>) {
        if !crate::performance::enabled() { return; }
        #[cfg(all(feature = "kindle", target_os = "linux"))]
        if let Ok(trace) = self.performance_trace.lock() {
            if let Some(trace) = trace.as_ref() {
                trace.set_reader_operation_id(operation_id);
            }
        }
        #[cfg(not(all(feature = "kindle", target_os = "linux")))]
        let _ = operation_id;
    }

    pub fn submit_refresh(&self, plan: RefreshPlan) -> Result<(), PlatformError> {
        let sink = self
            .refresh_sink
            .lock()
            .map_err(|_| PlatformError::Operation("refresh sink lock was poisoned".into()))?
            .clone();
        match sink {
            Some(sink) => sink.submit(plan),
            None if !self.is_kindle => Ok(()),
            None => Err(PlatformError::Unsupported("explicit_refresh")),
        }
    }

    pub fn request_refresh(
        &self,
        reason: RefreshReason,
    ) -> Result<Option<RefreshPlan>, PlatformError> {
        let plan = self.plan_refresh(reason);
        if let Some(plan) = plan {
            self.submit_refresh(plan)?;
        }
        Ok(plan)
    }

    pub fn set_reader_refresh_interval(&self, pages: u32) -> Result<(), PlatformError> {
        self.refresh
            .lock()
            .map_err(|_| PlatformError::Operation("refresh scheduler lock was poisoned".into()))?
            .set_reader_interval(pages)
            .map_err(PlatformError::InvalidValue)
    }

    pub fn set_flash_on_navigation(&self, enabled: bool) {
        if let Ok(mut scheduler) = self.refresh.lock() {
            scheduler.set_flash_on_navigation(enabled);
        }
    }

    /// Keep a task active without changing persistent power settings.
    ///
    /// The initial reset must succeed before a guard is returned. Subsequent
    /// failures are retained and can be surfaced by the application.
    pub fn hold_awake(&self) -> Result<WakeGuard, PlatformError> {
        self.control.reset_idle_timer()?;
        spawn_wake_guard(self.control.clone(), false, true)
    }

    /// Start task wake maintenance without waiting for any device command.
    ///
    /// The initial reset runs on the worker. Inspect `WakeGuard::failure()` for
    /// an initial or later device error. Dropping this guard requests stop and
    /// detaches; an in-flight bounded command may finish after the drop.
    pub fn hold_awake_deferred(&self) -> Result<WakeGuard, PlatformError> {
        spawn_wake_guard(self.control.clone(), true, false)
    }
}

fn spawn_wake_guard(
    control: Arc<dyn DeviceControl>,
    reset_on_start: bool,
    join_on_drop: bool,
) -> Result<WakeGuard, PlatformError> {
    let (stop, receiver) = std::sync::mpsc::channel();
    let error = Arc::new(Mutex::new(None));
    let worker_error = error.clone();
    let worker = std::thread::Builder::new()
        .name("bilicomics-wake-guard".into())
        .spawn(move || {
            run_wake_worker(control, receiver, worker_error, reset_on_start);
        })?;
    Ok(WakeGuard {
        stop: Some(stop),
        worker: Some(worker),
        error,
        join_on_drop,
    })
}

fn run_wake_worker(
    control: Arc<dyn DeviceControl>,
    receiver: std::sync::mpsc::Receiver<()>,
    error: Arc<Mutex<Option<String>>>,
    reset_on_start: bool,
) {
    let reset = || {
        if let Err(failure) = control.reset_idle_timer() {
            if let Ok(mut slot) = error.lock() {
                *slot = Some(failure.to_string());
            }
            false
        } else {
            true
        }
    };
    if reset_on_start {
        match receiver.try_recv() {
            Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        if !reset() {
            return;
        }
    }
    loop {
        match receiver.recv_timeout(Duration::from_secs(60)) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if !reset() {
                    return;
                }
            }
        }
    }
}

pub struct WakeGuard {
    stop: Option<std::sync::mpsc::Sender<()>>,
    worker: Option<std::thread::JoinHandle<()>>,
    error: Arc<Mutex<Option<String>>>,
    join_on_drop: bool,
}

impl WakeGuard {
    pub fn failure(&self) -> Option<String> {
        self.error.lock().ok()?.clone()
    }
}

impl Drop for WakeGuard {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            if self.join_on_drop {
                let _ = worker.join();
            }
        }
    }
}

/// Call after creating the window, before starting its event loop.
pub fn configure_window(
    window: &slint::Window,
    platform: &Platform,
) -> Result<(), slint::PlatformError> {
    window.try_dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
        scale_factor: platform.scale_factor(),
    })?;
    let (width, height) = platform.logical_viewport_size();
    let size = slint::LogicalSize::new(width as f32, height as f32);
    if platform.is_kindle {
        let (width, height) = platform.native_viewport_size().ok_or_else(|| {
            slint::PlatformError::Other("The native display geometry was not approved".into())
        })?;
        window.set_size(slint::PhysicalSize::new(width, height));
    } else {
        window.set_size(size);
    }
    window.try_dispatch_event(slint::platform::WindowEvent::Resized { size })?;
    Ok(())
}

/// Install before creating Tokio runtimes or other application threads.
#[cfg(all(feature = "kindle", target_os = "linux"))]
pub fn install_kindle_backend(
    font_bytes: &[u8],
) -> Result<slint_backend_kindle::KindleBackend, slint::PlatformError> {
    slint_backend_kindle::install(font_bytes)
}

/// Capture one consistent native display record without installing the UI backend.
/// The existing private native-read context must be proven before native I/O.
/// Returns `ready:<Y8|Y8INV>:<U|D|FIXED>:<0..3>` or `not-active`, without an LF.
pub fn capture_native_display_record(profile: DeviceProfile) -> Result<String, PlatformError> {
    #[cfg(all(feature = "kindle", target_os = "linux"))]
    {
        native_capture::capture(profile)
    }
    #[cfg(not(all(feature = "kindle", target_os = "linux")))]
    {
        let _ = profile;
        Err(PlatformError::Unsupported("kindle_backend"))
    }
}

/// Produce one bounded-launcher record without starting UI, workers, or input.
///
/// The owning launcher runs this internal helper in its existing private read
/// group before pausing the native WM. Successful inventory reads distinguish
/// fixed portrait devices from accelerometer devices; absolute rotation codes
/// are metadata only and never select a direction here.
pub fn native_portrait_framebuffer_record(profile: DeviceProfile) -> Result<String, PlatformError> {
    #[cfg(all(feature = "kindle", target_os = "linux"))]
    {
        let native = native::NativeControl::new(profile)?;
        let observed = slint_backend_kindle::probe_native_framebuffer(native.native_viewport())?;
        let automatic = slint_backend_kindle::native_accelerometer_available()?;
        Ok(format!(
            "{}:{}",
            if automatic { "AUTO" } else { "FIXED" },
            observed.rotation
        ))
    }
    #[cfg(not(all(feature = "kindle", target_os = "linux")))]
    {
        let _ = profile;
        Err(PlatformError::Unsupported("kindle_backend"))
    }
}

/// Publish only an authenticated portrait sidecar, without opening native I/O.
pub fn publish_native_portrait_orientation(
    profile: DeviceProfile,
    value: &str,
) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        validate_native_launch_approval(profile)?;
        crate::native_presentation::publish_native_portrait_orientation(value)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (profile, value);
        Err(PlatformError::Unsupported("kindle_backend").into())
    }
}

#[cfg(all(feature = "kindle", target_os = "linux"))]
pub struct KindleRefreshSink(slint_backend_kindle::RefreshHandle);

#[cfg(all(feature = "kindle", target_os = "linux"))]
pub struct KindleDitheringSink(slint_backend_kindle::DitheringHandle);

#[cfg(all(feature = "kindle", target_os = "linux"))]
impl KindleDitheringSink {
    pub fn new(handle: slint_backend_kindle::DitheringHandle) -> Self {
        Self(handle)
    }
}

#[cfg(all(feature = "kindle", target_os = "linux"))]
impl DitheringSink for KindleDitheringSink {
    fn hardware_y4_available(&self) -> bool {
        self.0.hardware_y4_available()
    }

    fn set_hardware_y4(&self, enabled: bool) -> bool {
        self.0.set_hardware_y4(enabled)
    }
}

#[cfg(all(feature = "kindle", target_os = "linux"))]
impl KindleRefreshSink {
    pub fn new(handle: slint_backend_kindle::RefreshHandle) -> Self {
        Self(handle)
    }
}

#[cfg(all(feature = "kindle", target_os = "linux"))]
impl RefreshSink for KindleRefreshSink {
    fn submit(&self, plan: RefreshPlan) -> Result<(), PlatformError> {
        self.0
            .request(kindle_refresh_request(plan))
            .map_err(|error| PlatformError::Operation(error.to_string()))
    }
}

#[cfg(all(feature = "kindle", target_os = "linux"))]
fn kindle_refresh_request(plan: RefreshPlan) -> slint_backend_kindle::RefreshRequest {
    use slint_backend_kindle::{PhysicalRect, RefreshArea, RefreshRequest, RefreshWaveform};
    let (waveform, flashing) = match plan.kind {
        RefreshKind::Full | RefreshKind::FlashUi | RefreshKind::FlashPartial => {
            (RefreshWaveform::Gc16, true)
        }
        RefreshKind::Fast => (RefreshWaveform::Du, false),
        RefreshKind::A2 => (RefreshWaveform::A2, false),
        RefreshKind::Ui | RefreshKind::Partial => (RefreshWaveform::Auto, false),
    };
    let area = if let Some(rect) = plan.rect {
        RefreshArea::Rect(PhysicalRect {
            x: i32::try_from(rect.x.saturating_mul(2)).unwrap_or(i32::MAX),
            y: i32::try_from(rect.y.saturating_mul(2)).unwrap_or(i32::MAX),
            width: rect.width.saturating_mul(2),
            height: rect.height.saturating_mul(2),
        })
    } else if matches!(plan.kind, RefreshKind::Full | RefreshKind::FlashUi) {
        RefreshArea::Full
    } else {
        RefreshArea::Dirty
    };
    RefreshRequest {
        waveform,
        flashing,
        area,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Condvar;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(all(feature = "kindle", target_os = "linux"))]
    #[test]
    fn reader_geometry_maps_to_a_complete_gc16_native_presentation() {
        use slint_backend_kindle::{RefreshArea, RefreshWaveform};
        let mut scheduler = RefreshScheduler::default();
        scheduler.set_reader_interval(0).unwrap();
        let request = kindle_refresh_request(
            scheduler.plan(RefreshReason::ReaderGeometry, Duration::ZERO).unwrap());
        assert_eq!(request.area, RefreshArea::Full);
        assert_eq!(request.waveform, RefreshWaveform::Gc16);
        assert!(request.flashing);
    }

    #[cfg(all(feature = "kindle", target_os = "linux"))]
    #[test]
    fn keyboard_feedback_maps_to_native_a2_and_dirty_ui_updates() {
        use slint_backend_kindle::{RefreshArea, RefreshWaveform};
        let mut scheduler = RefreshScheduler::default();
        let pressed = kindle_refresh_request(
            scheduler
                .plan(RefreshReason::Keyboard, Duration::ZERO)
                .unwrap(),
        );
        assert_eq!(pressed.waveform, RefreshWaveform::A2);
        assert_eq!(pressed.area, RefreshArea::Dirty);
        assert!(!pressed.flashing);

        let released = kindle_refresh_request(
            scheduler
                .plan(RefreshReason::FeedbackReleased, Duration::ZERO)
                .unwrap(),
        );
        assert_eq!(released.waveform, RefreshWaveform::Auto);
        assert_eq!(released.area, RefreshArea::Dirty);
        assert!(!released.flashing);
    }

    #[derive(Default)]
    struct ResetGate {
        state: Mutex<(bool, bool)>,
        changed: Condvar,
    }

    impl ResetGate {
        fn block_reset(&self) {
            let mut state = self.state.lock().unwrap();
            state.0 = true;
            self.changed.notify_all();
            while !state.1 {
                state = self.changed.wait(state).unwrap();
            }
        }

        fn wait_until_entered(&self) -> bool {
            let (state, _) = self
                .changed
                .wait_timeout_while(
                    self.state.lock().unwrap(),
                    Duration::from_secs(2),
                    |state| !state.0,
                )
                .unwrap();
            state.0
        }

        fn release(&self) {
            self.state.lock().unwrap().1 = true;
            self.changed.notify_all();
        }
    }

    #[derive(Default)]
    struct FakeControl {
        resets: AtomicUsize,
        gate: Option<Arc<ResetGate>>,
        fail_reset: bool,
        battery: u8,
        native_viewport: Option<(u32, u32)>,
    }

    impl DeviceControl for FakeControl {
        fn native_viewport(&self) -> Option<(u32, u32)> {
            self.native_viewport
        }

        fn capabilities(&self) -> PlatformCapabilities {
            PlatformCapabilities {
                keep_awake: true,
                ..Default::default()
            }
        }
        fn brightness(&self) -> Result<u8, PlatformError> {
            Err(PlatformError::Unsupported("brightness"))
        }
        fn set_brightness(&self, _: u8) -> Result<(), PlatformError> {
            Err(PlatformError::Unsupported("brightness"))
        }
        fn warmth(&self) -> Result<u8, PlatformError> {
            Err(PlatformError::Unsupported("warmth"))
        }
        fn set_warmth(&self, _: u8) -> Result<(), PlatformError> {
            Err(PlatformError::Unsupported("warmth"))
        }
        fn battery_percentage(&self) -> Result<u8, PlatformError> {
            Ok(self.battery)
        }
        fn set_inverted(&self, _: bool) -> Result<(), PlatformError> {
            Err(PlatformError::Unsupported("hardware_inversion"))
        }
        fn reset_idle_timer(&self) -> Result<(), PlatformError> {
            self.resets.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = &self.gate {
                gate.block_reset();
            }
            if self.fail_reset {
                Err(PlatformError::Operation("fixture reset failed".into()))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn native_viewport_is_separate_from_the_desktop_design_geometry() {
        let profile = DeviceProfile::Paperwhite6;
        let control = Arc::new(FakeControl {
            native_viewport: Some((1272, 1696)),
            ..Default::default()
        });
        let native = Platform::with_control(profile, true, control.clone());
        assert_eq!(native.native_viewport_size(), Some((1272, 1696)));
        assert_eq!(native.logical_viewport_size(), (636, 848));
        assert_eq!(native.scale_factor(), 2.0);

        let desktop = Platform::with_control(profile, false, control);
        assert_eq!(desktop.native_viewport_size(), None);
        assert_eq!(desktop.logical_viewport_size(), (632, 840));
        assert_eq!(desktop.scale_factor(), 1.0);
        assert_eq!(profile.logical_size(), (632, 840));
        assert_eq!(profile.physical_size(), (1264, 1680));
    }

    #[test]
    fn framebuffer_portrait_mapping_requires_the_observed_native_pair() {
        assert!(calibrated_framebuffer_portrait(
            DeviceProfile::Scribe,
            Some("scribe"),
            Some("5.19.5")
        ));
        for (profile, approved, firmware) in [
            (DeviceProfile::Paperwhite5, Some("pw5"), Some("5.19.5")),
            (
                DeviceProfile::Scribe2025,
                Some("scribe2025"),
                Some("5.19.5"),
            ),
            (DeviceProfile::Scribe, None, Some("5.19.5")),
            (DeviceProfile::Scribe, Some("scribe"), None),
            (DeviceProfile::Scribe, Some("scribe"), Some("5.19.6")),
        ] {
            assert!(!calibrated_framebuffer_portrait(
                profile, approved, firmware
            ));
        }
        let desktop = Platform::with_control(
            DeviceProfile::Scribe,
            false,
            Arc::new(FakeControl::default()),
        );
        assert!(!desktop.uses_calibrated_framebuffer_portrait());
    }

    #[test]
    fn deferred_creation_and_drop_do_not_wait_for_a_blocked_device_command() {
        let gate = Arc::new(ResetGate::default());
        let control = Arc::new(FakeControl {
            gate: Some(gate.clone()),
            ..Default::default()
        });
        let platform = Platform::with_control(DeviceProfile::Paperwhite6, true, control.clone());
        let (created, received) = std::sync::mpsc::channel();
        let creator = std::thread::spawn(move || {
            created.send(platform.hold_awake_deferred()).unwrap();
        });
        let guard = match received.recv_timeout(Duration::from_secs(2)) {
            Ok(result) => result.unwrap(),
            Err(error) => {
                gate.release();
                creator.join().unwrap();
                panic!("deferred creation waited for the device: {error}");
            }
        };
        if !gate.wait_until_entered() {
            gate.release();
            drop(guard);
            creator.join().unwrap();
            panic!("the deferred worker did not attempt the initial reset");
        }
        let (dropped, drop_received) = std::sync::mpsc::channel();
        let dropper = std::thread::spawn(move || {
            drop(guard);
            dropped.send(()).unwrap();
        });
        let returned_without_release = drop_received.recv_timeout(Duration::from_secs(1)).is_ok();
        gate.release();
        creator.join().unwrap();
        dropper.join().unwrap();
        assert!(
            returned_without_release,
            "deferred drop joined a blocked device command"
        );
        assert_eq!(control.resets.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cancellation_before_worker_start_skips_the_initial_device_command() {
        for explicit_stop in [true, false] {
            let control = Arc::new(FakeControl::default());
            let (stop, receiver) = std::sync::mpsc::channel();
            if explicit_stop {
                stop.send(()).unwrap();
            }
            drop(stop);
            let error = Arc::new(Mutex::new(None));
            run_wake_worker(control.clone(), receiver, error.clone(), true);
            assert_eq!(control.resets.load(Ordering::SeqCst), 0);
            assert!(error.lock().unwrap().is_none());
        }
    }

    #[test]
    fn an_initial_deferred_failure_is_retained_for_the_application() {
        let control = Arc::new(FakeControl {
            fail_reset: true,
            ..Default::default()
        });
        let platform = Platform::with_control(DeviceProfile::Paperwhite6, true, control.clone());
        let mut guard = platform.hold_awake_deferred().unwrap();
        guard.worker.take().unwrap().join().unwrap();
        assert!(guard.failure().unwrap().contains("fixture reset failed"));
        assert_eq!(control.resets.load(Ordering::SeqCst), 1);
        assert!(!guard.join_on_drop);
    }

    #[test]
    fn the_synchronous_api_still_returns_initial_device_failures_directly() {
        let control = Arc::new(FakeControl {
            fail_reset: true,
            ..Default::default()
        });
        let platform = Platform::with_control(DeviceProfile::Paperwhite6, true, control.clone());
        assert!(matches!(
            platform.hold_awake(),
            Err(PlatformError::Operation(_))
        ));
        assert_eq!(control.resets.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn battery_readings_keep_valid_endpoints_and_reject_invalid_values() {
        for battery in [0, 43, 100] {
            let platform = Platform::with_control(
                DeviceProfile::Paperwhite6,
                true,
                Arc::new(FakeControl {
                    battery,
                    ..Default::default()
                }),
            );
            assert_eq!(platform.battery_percentage().unwrap(), battery);
        }
        let platform = Platform::with_control(
            DeviceProfile::Paperwhite6,
            true,
            Arc::new(FakeControl {
                battery: 101,
                ..Default::default()
            }),
        );
        assert!(matches!(
            platform.battery_percentage(),
            Err(PlatformError::InvalidValue(_))
        ));
        let desktop = Platform::new(DeviceProfile::Paperwhite6, false).unwrap();
        assert!(matches!(
            desktop.battery_percentage(),
            Err(PlatformError::Unsupported("battery"))
        ));
    }
}

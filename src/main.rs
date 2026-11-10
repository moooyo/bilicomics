use std::{
    cell::{Cell, RefCell},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use bilicomics::{
    app::{App, AppOptions, PreparedApp},
    fonts::{Fonts, PreparedFont},
    native_presentation::{NativeOutputMode, NativePortraitDirection, NativeResumePresentation},
    platform::{self, DeviceProfile, Platform, RefreshReason},
    power_events::{PowerAck, PowerEvent, SignalBridge},
    ui::{
        AccountModel, AppWindow, AuthModel, CatalogModel, DownloadModel, ReaderModel, ShellModel,
        Theme,
    },
};
use clap::Parser;
use serde::Deserialize;
use slint::{ComponentHandle, Model};

mod runtime_wakeup;
use runtime_wakeup::{RuntimeWake, StartupContinuation, TraceMoment, trace_moment};

#[derive(Debug, Parser)]
#[command(
    name = "bilicomics",
    version,
    about = "Independent Bilibili Comics client for Kindle"
)]
struct Arguments {
    /// Select a design profile, not a declaration of hardware compatibility.
    #[arg(long, default_value = "pw5")]
    device: DeviceProfile,
    /// Use the approved native Kindle framebuffer and input adapter.
    #[arg(long)]
    kindle: bool,
    /// Store account state, downloads, and preferences in this directory.
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Use isolated design fixtures with no authenticated network operations.
    #[arg(long)]
    demo: bool,
    /// Read local metadata and cached content without network requests.
    #[arg(long)]
    offline: bool,
    /// Start at a named route when local data is available.
    #[arg(long, default_value = "")]
    screen: String,
    /// Use a landscape desktop preview of the selected profile.
    #[arg(long)]
    landscape: bool,
    /// Save the final application window as a PNG.
    #[arg(long)]
    capture: Option<PathBuf>,
    /// Save two real startup frames before attachment completes.
    #[arg(long)]
    capture_startup: Option<PathBuf>,
    /// Replay timed intents; their clock begins when the application is ready.
    #[arg(long)]
    actions: Option<PathBuf>,
    /// Quit after a bounded duration following application readiness.
    #[arg(long)]
    exit_after_ms: Option<u64>,
    /// Delay only the isolated demonstration worker for startup verification.
    #[arg(long, hide = true)]
    startup_delay_ms: Option<u64>,
    /// Record power transitions without credentials or network payloads.
    #[arg(long)]
    lifecycle_trace: Option<PathBuf>,
    /// Record bounded diagnostic stage timing in a new private JSONL file.
    #[arg(long, hide = true, conflicts_with_all = ["relay_power_events", "capture_native_display", "probe_native_portrait_framebuffer", "publish_native_portrait_orientation"])]
    performance_trace: Option<PathBuf>,
    /// Relay bounded native power records without initializing application services.
    #[arg(long, hide = true)]
    relay_power_events: bool,
    /// Capture one owned native-state transaction without starting application services.
    #[arg(long, hide = true, conflicts_with_all = ["relay_power_events", "probe_native_portrait_framebuffer", "publish_native_portrait_orientation"])]
    capture_native_display: bool,
    /// Observe read-only portrait metadata inside the owning launcher's bounded group.
    #[arg(long, hide = true, conflicts_with = "relay_power_events")]
    probe_native_portrait_framebuffer: bool,
    /// Publish one typed sidecar from the owning launcher's bounded read group.
    #[arg(long, hide = true, conflicts_with_all = ["relay_power_events", "probe_native_portrait_framebuffer"])]
    publish_native_portrait_orientation: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptAction {
    at_ms: u64,
    section: String,
    action: String,
    #[serde(default)]
    value: String,
}

type Application = Rc<RefCell<App>>;

#[derive(Default)]
struct RuntimeControl {
    application: RefCell<Option<Application>>,
    suspended: Cell<bool>,
    quitting: Cell<bool>,
    native_epdc_mode: Cell<Option<NativeOutputMode>>,
}

#[derive(Clone)]
enum DisplayPower {
    Desktop,
    #[cfg(all(feature = "kindle", target_os = "linux"))]
    Kindle(slint_backend_kindle::DisplayLifecycleHandle),
}

struct NativeResumeFramebuffer {
    rotation: u32,
    output_inverted: Option<bool>,
    holding_direction: Option<NativePortraitDirection>,
}

impl DisplayPower {
    fn probe_native_resume_framebuffer(
        &self,
        framebuffer_scanout: bool,
    ) -> Result<(u32, NativeResumeFramebuffer)> {
        match self {
            Self::Desktop => {
                let _ = framebuffer_scanout;
                bail!("Native framebuffer observation requires the Kindle display backend")
            }
            #[cfg(all(feature = "kindle", target_os = "linux"))]
            Self::Kindle(handle) => {
                let observation = handle.probe_native_framebuffer()?;
                let holding_direction = if framebuffer_scanout {
                    handle.probe_native_portrait_pose()?.map(|orientation| match orientation {
                        slint_backend_kindle::NativePortraitOrientation::Up => NativePortraitDirection::Up,
                        slint_backend_kindle::NativePortraitOrientation::Down => NativePortraitDirection::Down,
                    })
                } else {
                    None
                };
                Ok((observation.rotation, NativeResumeFramebuffer {
                    rotation: observation.rotation,
                    output_inverted: observation.output_inverted,
                    holding_direction,
                }))
            }
        }
    }
    fn set_native_portrait_direction(
        &self,
        direction: NativePortraitDirection,
        raw_rotation: u32,
        framebuffer_scanout: bool,
    ) -> Result<()> {
        self.set_native_portrait_presentation(direction, direction, raw_rotation, framebuffer_scanout)
    }

    fn set_native_portrait_presentation(
        &self,
        holding_direction: NativePortraitDirection,
        scanout_direction: NativePortraitDirection,
        raw_rotation: u32,
        framebuffer_scanout: bool,
    ) -> Result<()> {
        match self {
            Self::Desktop => {
                let _ = (holding_direction, scanout_direction, raw_rotation, framebuffer_scanout);
                Ok(())
            }
            #[cfg(all(feature = "kindle", target_os = "linux"))]
            Self::Kindle(handle) => {
                let orientation = |direction| match direction {
                    NativePortraitDirection::Up => {
                        slint_backend_kindle::NativePortraitOrientation::Up
                    }
                    NativePortraitDirection::Down => {
                        slint_backend_kindle::NativePortraitOrientation::Down
                    }
                };
                let staged = if framebuffer_scanout {
                    handle.set_native_framebuffer_portrait_calibration(
                        orientation(holding_direction),
                        orientation(scanout_direction),
                        raw_rotation,
                    )
                } else {
                    handle.set_native_portrait_orientation(orientation(scanout_direction))
                };
                staged.map_err(Into::into)
            }
        }
    }
    fn set_output_inverted(&self, inverted: bool) -> Result<()> {
        match self {
            Self::Desktop => {
                let _ = inverted;
                Ok(())
            }
            #[cfg(all(feature = "kindle", target_os = "linux"))]
            Self::Kindle(handle) => handle.set_output_inverted(inverted).map_err(Into::into),
        }
    }
    fn suspend(&self) -> Result<()> {
        match self {
            Self::Desktop => Ok(()),
            #[cfg(all(feature = "kindle", target_os = "linux"))]
            Self::Kindle(handle) => handle.suspend().map_err(Into::into),
        }
    }
    fn resume(&self) -> Result<()> {
        match self {
            Self::Desktop => Ok(()),
            #[cfg(all(feature = "kindle", target_os = "linux"))]
            Self::Kindle(handle) => handle.resume().map_err(Into::into),
        }
    }
}

enum StartupEvent {
    Font(PreparedFont, TraceMoment),
    State(PreparedApp, TraceMoment),
    Stage(&'static str, TraceMoment),
    Failed(String),
}

fn main() -> std::process::ExitCode {
    runtime_wakeup::initialize_trace_clock();
    let result = run(Arguments::parse());
    bilicomics::performance::finish();
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bilicomics: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(arguments: Arguments) -> Result<()> {
    if arguments.relay_power_events {
        return bilicomics::power_events::relay_native_power_events();
    }
    if arguments.capture_native_display {
        if !arguments.kindle || arguments.demo {
            bail!("Native display capture requires an approved native launcher");
        }
        let record = platform::capture_native_display_record(arguments.device)
            .context("Capture the owning launcher's native display state")?;
        println!("{record}");
        return Ok(());
    }
    if let Some(value) = arguments.publish_native_portrait_orientation.as_deref() {
        if !arguments.kindle || arguments.demo {
            bail!("Native portrait publication requires an approved native launcher");
        }
        platform::publish_native_portrait_orientation(arguments.device, value)
            .context("Publish the owning launcher's private portrait sidecar")?;
        println!("PUBLISHED");
        return Ok(());
    }
    if arguments.probe_native_portrait_framebuffer {
        if !arguments.kindle || arguments.demo {
            bail!("Native portrait observation requires an approved native launcher");
        }
        let record = platform::native_portrait_framebuffer_record(arguments.device)
            .context("Observe read-only native portrait metadata")?;
        println!("{record}");
        return Ok(());
    }
    if let Some(path) = arguments.performance_trace.as_deref() {
        if arguments.lifecycle_trace.as_deref() == Some(path) {
            eprintln!("bilicomics: performance diagnostics disabled because output files overlap");
        } else if let Err(error) = bilicomics::performance::configure(path) {
            eprintln!("bilicomics: performance diagnostics unavailable: {:?}", error.kind());
        }
    }
    if arguments.kindle && arguments.landscape {
        bail!(
            "Native landscape needs an approved rotation adapter; use the desktop landscape preview"
        );
    }
    if arguments.startup_delay_ms.is_some() && !arguments.demo {
        bail!("A controlled startup delay requires isolated demonstration data");
    }
    if arguments
        .startup_delay_ms
        .is_some_and(|delay| delay > 10_000)
    {
        bail!("The controlled startup delay must not exceed ten seconds");
    }
    record_startup_stage(arguments.lifecycle_trace.as_deref(), "process_enter",
        runtime_wakeup::process_entry_moment(), None);
    let data_dir = arguments
        .data_dir
        .clone()
        .map(Ok)
        .unwrap_or_else(default_data_dir)?;
    let mut signals = SignalBridge::install().context("Install lifecycle signal delivery")?;
    let native_presentation = if arguments.kindle {
        Some(
            signals
                .native_presentation_context()
                .context("Native output requires an owned launcher mode snapshot")?,
        )
    } else {
        None
    };
    let startup_native_mode = native_presentation
        .as_ref()
        .map(|context| context.startup_mode())
        .transpose()
        .context("Read the native startup output mode before acquiring display or input")?;
    if let Some(context) = native_presentation.as_ref() {
        context
            .startup_portrait_calibration()
            .context("Require a protected startup portrait calibration before native I/O")?;
    }
    let platform = Arc::new(Platform::new(arguments.device, arguments.kindle)?);
    let framebuffer_portrait = platform.uses_calibrated_framebuffer_portrait();

    // Install once with a tiny real font, before starting any worker or signal reader.
    #[cfg(all(feature = "kindle", target_os = "linux"))]
    let native_backend = if arguments.kindle {
        Some(platform::install_kindle_backend(Fonts::bootstrap_bytes())?)
    } else {
        None
    };
    #[cfg(all(feature = "kindle", target_os = "linux"))]
    if bilicomics::performance::enabled() {
        if let Some(backend) = native_backend.as_ref() {
            platform.set_kindle_performance_trace(backend.performance_trace_handle())?;
        }
    }
    #[cfg(all(feature = "kindle", target_os = "linux"))]
    let startup_native_mode = if let Some(backend) = native_backend.as_ref() {
        let lifecycle = backend.lifecycle_handle();
        let context = native_presentation
            .as_ref()
            .context("Native presentation context is unavailable")?;
        let observed = context
            .observe_startup_portrait(|| {
                let framebuffer = lifecycle.probe_native_framebuffer()?;
                Ok((framebuffer.rotation, framebuffer))
            })
            .context("Match the pre-takeover portrait calibration to the current framebuffer")?;
        let framebuffer_mode = observed.framebuffer.output_inverted;
        context
            .publish_framebuffer_capability(framebuffer_mode.is_some())
            .context("Publish the native output observation capability")?;
        let supervised_mode = match observed.presentation {
            NativeResumePresentation::Observed(mode) => Some(mode),
            NativeResumePresentation::FramebufferProbe => None,
        };
        let mode = framebuffer_mode
            .map(native_mode_from_inversion)
            .or(supervised_mode)
            .context("Native startup output mode is unavailable")?;
        DisplayPower::Kindle(lifecycle.clone())
            .set_native_portrait_direction(observed.direction, observed.framebuffer.rotation, framebuffer_portrait)
            .context("Stage the trusted native startup portrait direction")?;
        lifecycle
            .set_resume_trace_path(arguments.lifecycle_trace.clone())
            .context("Configure optional native resume timing")?;
        lifecycle
            .set_output_inverted(mode.inverted())
            .context("Configure native startup output compensation")?;
        platform.set_refresh_sink(Arc::new(platform::KindleRefreshSink::new(
            backend.refresh_handle(),
        )))?;
        Some(mode)
    } else {
        startup_native_mode
    };
    if !arguments.kindle {
        #[cfg(feature = "desktop")]
        slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name("software".into())
            .select()
            .context("Select the desktop software renderer")?;
        #[cfg(not(feature = "desktop"))]
        bail!("The desktop backend is disabled in this executable");
    }
    let ui = AppWindow::new().context("Create the startup window")?;
    platform::configure_window(ui.window(), &platform)?;
    let (mut width, mut height) = arguments.device.logical_size();
    if arguments.landscape {
        std::mem::swap(&mut width, &mut height);
    }
    ui.global::<Theme>()
        .set_scribe(arguments.device.is_scribe());
    ui.global::<ShellModel>().set_landscape(arguments.landscape);
    ui.global::<ShellModel>().set_demo(arguments.demo);
    ui.global::<ShellModel>().set_booting(true);
    Fonts::register_bootstrap(&ui)?;
    let wake = RuntimeWake::new(ui.as_weak());
    if !arguments.kindle {
        ui.window()
            .set_size(slint::LogicalSize::new(width as f32, height as f32));
    }
    let display_power = DisplayPower::Desktop;
    #[cfg(all(feature = "kindle", target_os = "linux"))]
    let display_power = native_backend
        .as_ref()
        .map(|backend| DisplayPower::Kindle(backend.lifecycle_handle()))
        .unwrap_or(display_power);

    let control = Rc::new(RuntimeControl::default());
    control.native_epdc_mode.set(startup_native_mode);
    let bootstrap_exit = control.clone();
    ui.global::<ShellModel>().on_action(move |action, _| {
        if action == "exit" && !bootstrap_exit.suspended.get() {
            bootstrap_exit.quitting.set(true);
            let _ = slint::quit_event_loop();
        }
    });
    let power_control = control.clone();
    let power_ui = ui.as_weak();
    let power_wake = wake.clone();
    let trace_path = arguments.lifecycle_trace.clone();
    signals.start(move |event| {
        // Release the application borrow before native pointer cancellation can
        // emit widget callbacks. Their application intent gate is already set.
        let application = power_control.application.borrow().clone();
        let Some(ui) = power_ui.upgrade() else { return Ok(PowerAck::saved()); };
        let mut resume_stages = Vec::new();
        let acknowledgement = match event {
            PowerEvent::Suspend => {
                power_control.suspended.set(true);
                ui.global::<ShellModel>().set_suspended(true);
                if let Some(app) = &application { app.borrow_mut().begin_suspend_intent_gate(); }
                display_power.suspend().context("Release native input and display for screensaver")?;
                let checkpoint = application.as_ref().map(|app| app.borrow_mut().suspend()).transpose();
                if let Err(error) = &checkpoint { eprintln!("bilicomics: sleep checkpoint failed: {error:#}"); }
                if checkpoint.is_ok() { PowerAck::saved() } else { PowerAck::checkpoint_error() }
            }
            PowerEvent::Resume => {
                capture_resume_stage(&mut resume_stages, trace_path.is_some(), "callback_received");
                if let Some(context) = native_presentation.as_ref() {
                    power_control.native_epdc_mode.set(None);
                    let observed = context.observe_resume_portrait(|| display_power.probe_native_resume_framebuffer(framebuffer_portrait))
                        .context("Resolve the current portrait resume from a fresh framebuffer observation")?;
                    let mode = match observed.presentation {
                        NativeResumePresentation::Observed(mode) => mode,
                        NativeResumePresentation::FramebufferProbe => {
                            let inverted = observed.framebuffer.output_inverted
                                .context("The previously advertised kernel output observation is no longer supported")?;
                            native_mode_from_inversion(inverted)
                        }
                    };
                    let holding = observed.framebuffer.holding_direction.unwrap_or(observed.direction);
                    display_power.set_native_portrait_presentation(holding, observed.direction, observed.framebuffer.rotation, framebuffer_portrait)
                        .context("Stage native portrait direction before reacquiring input")?;
                    capture_native_portrait_stage(&mut resume_stages, trace_path.is_some(),
                        holding, observed.direction, observed.framebuffer.holding_direction.is_some());
                    display_power.set_output_inverted(mode.inverted())
                        .context("Configure native resume output compensation before reacquiring input")?;
                    power_control.native_epdc_mode.set(Some(mode));
                }
                capture_resume_stage(&mut resume_stages, trace_path.is_some(), "output_mode_ready");
                display_power.resume().context("Reacquire native display and input")?;
                capture_resume_stage(&mut resume_stages, trace_path.is_some(), "display_resumed");
                power_control.suspended.set(false);
                let restored = application.as_ref().map(|app| app.borrow_mut().resume()).transpose();
                capture_resume_stage(&mut resume_stages, trace_path.is_some(), "application_work_done");
                ui.global::<ShellModel>().set_suspended(false);
                power_wake.request();
                if let Err(error) = &restored { eprintln!("bilicomics: resume preparation failed: {error:#}"); }
                if restored.is_ok() { PowerAck::saved() } else { PowerAck::checkpoint_error() }
            }
            PowerEvent::Quit => {
                power_control.quitting.set(true);
                if let Some(app) = &application { app.borrow_mut().request_quit(); }
                let _ = slint::quit_event_loop();
                PowerAck::saved()
            }
        };
        if let Some(path) = trace_path.as_deref() {
            let snapshot = application.as_ref().and_then(|app| app.borrow().reader_lifecycle_snapshot());
            if let Err(error) = append_power_trace(path, event, acknowledgement, &ui, snapshot,
                power_control.native_epdc_mode.get(), &resume_stages) {
                eprintln!("bilicomics: power trace could not be saved: {error:#}");
            }
        }
        Ok(acknowledgement)
    })?;
    if signals.quit_requested() || control.quitting.get() {
        return Ok(());
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("bilicomics-network")
        .enable_all()
        .build()
        .context("Create background workers")?;
    let handle = runtime.handle().clone();
    let options = AppOptions {
        data_dir,
        profile: arguments.device,
        demo: arguments.demo,
        offline: arguments.offline,
        initial_screen: arguments.screen.clone(),
        landscape: arguments.landscape,
    };
    let mut actions = match arguments.actions.as_deref() {
        Some(path) => serde_json::from_slice::<Vec<ScriptAction>>(&fs::read(path)?)?,
        None => Vec::new(),
    };
    actions.sort_by_key(|action| action.at_ms);
    let deadline = arguments.exit_after_ms.or_else(|| {
        if arguments.actions.is_some() {
            Some(
                actions
                    .last()
                    .map_or(500, |action| action.at_ms.saturating_add(1000)),
            )
        } else if arguments.capture.is_some() {
            Some(1500)
        } else {
            None
        }
    });
    let failures = Rc::new(RefCell::new(Vec::<String>::new()));
    let cancelled = Arc::new(AtomicBool::new(false));
    let close_control = control.clone();
    ui.window().on_close_requested(move || {
        close_control.quitting.set(true);
        if let Some(app) = close_control.application.borrow().as_ref() {
            app.borrow_mut().request_quit();
        }
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::KeepWindowShown
    });

    let (startup_sender, startup_receiver) = mpsc::channel::<StartupEvent>();
    let tick_ui = ui.as_weak();
    let tick_control = control.clone();
    let tick_failures = failures.clone();
    let tick_cancelled = cancelled.clone();
    let tick_platform = platform.clone();
    let startup_capture = arguments.capture_startup.clone();
    let startup_delay = arguments.startup_delay_ms.unwrap_or_default();
    let startup_trace = arguments.lifecycle_trace.clone();
    let trace_demo_ui = arguments.demo;
    let automatic_error_exit = arguments.capture.is_some()
        || arguments.actions.is_some()
        || arguments.exit_after_ms.is_some();
    let first_rendered = Arc::new(AtomicBool::new(false));
    let render_gate = first_rendered.clone();
    let render_wake = wake.clone();
    let render_trace = arguments.lifecycle_trace.clone();
    let rendering_notifier = ui.window().set_rendering_notifier(move |state, _| {
        if matches!(state, slint::RenderingState::AfterRendering)
            && !render_gate.swap(true, Ordering::AcqRel)
        {
            record_startup_stage(render_trace.as_deref(), "first_scene_rendered", trace_moment(), None);
            render_wake.request();
        }
    }).is_ok();
    // Software renderers may not expose a notifier. In that case the first
    // maintenance turn retains the existing startup yield before consuming work.
    let timer = slint::Timer::default();
    let tick_wake = wake.clone();
    let startup_continue = StartupContinuation::new(wake.clone());
    let mut worker_started = false;
    let mut fonts = Fonts::default();
    let mut prepared = None::<PreparedApp>;
    let mut fonts_registered = false;
    let mut ready_at = None::<Instant>;
    let mut failed_at = None::<Instant>;
    let mut next_action = 0;
    let mut frame = 0;
    let mut last_animation = Instant::now();
    let mut captured_startup = 0;
    let mut last_reader_trace = String::new();
    let mut last_reader_generation = None::<u64>;
    let mut last_ui_trace = String::new();
    let mut last_checkpoint_trace = None::<serde_json::Value>;
    let mut performance_first_driver = true;
    let mut performance_gate = None::<&'static str>;
    let driver = Rc::new(RefCell::new(move || {
            if performance_first_driver {
                performance_first_driver = false;
                bilicomics::performance::startup("driver_first_entry", &());
            }
            let Some(ui) = tick_ui.upgrade() else {
                return;
            };
            let application_quit = tick_control
                .application
                .borrow()
                .as_ref()
                .is_some_and(|app| app.borrow().is_quitting());
            if tick_control.quitting.get() || application_quit {
                let _ = slint::quit_event_loop();
                return;
            }
            if tick_control.suspended.get() {
                return;
            }
            let shell = ui.global::<ShellModel>();
            if shell.get_booting() {
                if failed_at.is_some() {
                    if automatic_error_exit
                        && failed_at.is_some_and(|at| at.elapsed() >= Duration::from_millis(700))
                    {
                        tick_control.quitting.set(true);
                        let _ = slint::quit_event_loop();
                    }
                    return;
                }
                if last_animation.elapsed() >= Duration::from_millis(400) {
                    frame = (frame + 1) % 4;
                    shell.set_startup_frame(frame);
                    last_animation = Instant::now();
                    // Only the changed black-and-white indicator is dirty.
                    let _ = tick_platform.request_refresh(RefreshReason::Press);
                }
                if let Some(path) = startup_capture.as_deref() {
                    if captured_startup == 0 || captured_startup == 1 && frame != 0 {
                        let output = startup_frame_path(path, captured_startup);
                        if let Err(error) = capture_startup_frame(
                            &ui,
                            &output,
                            worker_started,
                            tick_control.native_epdc_mode.get(),
                        ) {
                            tick_failures
                                .borrow_mut()
                                .push(format!("Startup capture failed: {error:#}"));
                        }
                        captured_startup += 1;
                    }
                }
                if !worker_started {
                    if rendering_notifier && !first_rendered.load(Ordering::Acquire) {
                        record_performance_gate(&mut performance_gate, "first_scene");
                        return;
                    }
                    record_performance_gate(&mut performance_gate, "prepare");
                    worker_started = true;
                    record_startup_stage(startup_trace.as_deref(), "workers_started", trace_moment(), None);
                    start_startup_workers(&handle, options.clone(), startup_delay,
                        tick_cancelled.clone(), startup_sender.clone(), tick_wake.clone(),
                        startup_trace.is_some());
                    // Yield this bootstrap turn before consuming even a very fast result.
                    return;
                }
                while let Ok(event) = startup_receiver.try_recv() {
                    match event {
                        StartupEvent::Font(value, moment) => {
                            if let Err(error) = fonts.accept(value) {
                                tick_failures.borrow_mut().push(format!("Prepare fonts failed: {error:#}"));
                                shell.set_startup_error(true);
                                failed_at = Some(Instant::now());
                                return;
                            }
                            record_startup_stage(startup_trace.as_deref(), "font_decoded", moment, None);
                        }
                        StartupEvent::State(value, moment) => {
                            prepared = Some(value);
                            record_startup_stage(startup_trace.as_deref(), "state_prepared", moment, None);
                        }
                        StartupEvent::Stage(stage, moment) => {
                            record_startup_stage(startup_trace.as_deref(), stage, moment, None);
                        }
                        StartupEvent::Failed(error) => {
                            tick_cancelled.store(true, Ordering::Release);
                            prepared = None;
                            fonts = Fonts::default();
                            for pending in startup_receiver.try_iter() { drop(pending); }
                            tick_failures
                                .borrow_mut()
                                .push(format!("Application preparation failed: {error}"));
                            shell.set_startup_error(true);
                            failed_at = Some(Instant::now());
                            return;
                        }
                    }
                }
                if !fonts_registered {
                    record_performance_gate(&mut performance_gate, "fonts");
                    if fonts.can_register_next() {
                        let measured = bilicomics::performance::enabled().then(bilicomics::performance::Span::start);
                        let registration = fonts.register_next();
                        if let Some(measured) = measured {
                            bilicomics::performance::startup("font_registration", &serde_json::json!({
                                "elapsed": measured.finish(), "success": registration.is_ok(),
                                "registered_faces": fonts.registered_count(),
                            }));
                        }
                        match registration {
                            Ok(done) => {
                                fonts_registered = done;
                                record_startup_stage(startup_trace.as_deref(), "font_registered",
                                    trace_moment(), Some(fonts.registered_count()));
                                if done {
                                    let measured = bilicomics::performance::enabled().then(bilicomics::performance::Span::start);
                                    let finished = fonts.finish(&ui);
                                    if let Some(measured) = measured {
                                        bilicomics::performance::startup("font_finish", &serde_json::json!({
                                            "elapsed": measured.finish(), "success": finished.is_ok(),
                                        }));
                                    }
                                    if let Err(error) = finished {
                                        tick_failures
                                            .borrow_mut()
                                            .push(format!("Register fonts failed: {error:#}"));
                                        shell.set_startup_error(true);
                                        failed_at = Some(Instant::now());
                                    }
                                    shell.set_startup_phase("state".into());
                                }
                            }
                            Err(error) => {
                                tick_failures
                                    .borrow_mut()
                                    .push(format!("Register fonts failed: {error:#}"));
                                shell.set_startup_error(true);
                                failed_at = Some(Instant::now());
                            }
                        }
                        if failed_at.is_none() {
                            startup_continue.schedule();
                        }
                        return;
                    }
                    // The next decompressed face will notify this driver.
                    return;
                }
                if tick_control.application.borrow().is_none() {
                    record_performance_gate(&mut performance_gate, "state");
                    if fonts_registered {
                        if let Some(prepared) = prepared.take() {
                            shell.set_startup_phase("attach".into());
                            match App::attach_prepared(
                                &ui,
                                prepared,
                                tick_platform.clone(),
                                handle.clone(),
                            ) {
                                Ok(application) => {
                                    let job_wake = tick_wake.clone();
                                    application.borrow().set_runtime_wakeup(Arc::new(move || job_wake.request()));
                                    *tick_control.application.borrow_mut() = Some(application);
                                    record_startup_stage(startup_trace.as_deref(), "application_attached", trace_moment(), None);
                                    startup_continue.schedule();
                                }
                                Err(error) => {
                                    tick_failures
                                        .borrow_mut()
                                        .push(format!("Attach application failed: {error:#}"));
                                    shell.set_startup_error(true);
                                    failed_at = Some(Instant::now());
                                }
                            }
                        }
                    }
                    return;
                }
                let application = tick_control
                    .application
                    .borrow()
                    .clone()
                    .expect("Attached startup application");
                record_performance_gate(&mut performance_gate, "attach");
                let startup_result = { application.borrow_mut().advance_startup() };
                match startup_result {
                    Ok(true) => {
                        shell.set_booting(false);
                        record_performance_gate(&mut performance_gate, "ready");
                        bilicomics::performance::startup("ui_ready", &());
                        ready_at = Some(Instant::now());
                        let _ = tick_platform.request_refresh(RefreshReason::EnterScreen);
                        if let Some(path) = startup_trace.as_deref() {
                            if let Err(error) = append_ready_trace(
                                path,
                                "ready",
                                &ui,
                                application.borrow().reader_lifecycle_snapshot(),
                                application.borrow().resource_snapshot(),
                                tick_control.native_epdc_mode.get(),
                            ) {
                                eprintln!(
                                    "bilicomics: startup trace could not be saved: {error:#}"
                                );
                            }
                        }
                        // Results may have queued while startup owned the UI.
                        startup_continue.schedule();
                    }
                    Ok(false) => {
                        record_startup_stage(startup_trace.as_deref(), "attach_stage_advanced", trace_moment(), None);
                        startup_continue.schedule();
                    }
                    Err(error) => {
                        tick_failures
                            .borrow_mut()
                            .push(format!("Initialize application views failed: {error:#}"));
                        shell.set_startup_error(true);
                        failed_at = Some(Instant::now());
                    }
                }
                return;
            }
            let Some(application) = tick_control.application.borrow().clone() else {
                return;
            };
            let elapsed = ready_at
                .get_or_insert_with(Instant::now)
                .elapsed()
                .as_millis() as u64;
            let mut application = application.borrow_mut();
            while let Some(action) = actions
                .get(next_action)
                .filter(|action| action.at_ms <= elapsed)
            {
                if let Err(error) =
                    application.dispatch(&action.section, &action.action, &action.value)
                {
                    if action.section == "lifecycle" {
                        eprintln!("bilicomics: lifecycle checkpoint warning: {error:#}");
                    } else {
                        tick_failures
                            .borrow_mut()
                            .push(format!("Scripted intent failed: {error:#}"));
                        application.request_quit();
                    }
                }
                next_action += 1;
            }
            if let Err(error) = application.tick() {
                tick_failures
                    .borrow_mut()
                    .push(format!("Application update failed: {error:#}"));
                application.request_quit();
            }
            if let Some(path) = startup_trace.as_deref() {
                let resources = application.resource_snapshot();
                if let Some(checkpoint) = anonymous_checkpoint_state(&resources) {
                    if last_checkpoint_trace.as_ref() != Some(&checkpoint) {
                        if let Err(error) = append_trace_record(path, serde_json::json!({
                            "process_id": std::process::id(), "event": "reader-checkpoint",
                            "checkpoint": &checkpoint,
                        })) {
                            eprintln!("bilicomics: reader checkpoint trace could not be saved: {error:#}");
                        }
                        last_checkpoint_trace = Some(checkpoint);
                    }
                }
                let generation = resources
                    .get("reader_generation")
                    .and_then(serde_json::Value::as_u64);
                if let Some(previous) = last_reader_generation.filter(|_| generation.is_none()) {
                    if let Err(error) = append_reader_closed_trace(
                        path,
                        &ui,
                        previous,
                        &resources,
                        tick_control.native_epdc_mode.get(),
                    ) {
                        eprintln!(
                            "bilicomics: reader resource trace could not be saved: {error:#}"
                        );
                    }
                }
                last_reader_generation = generation;
                if trace_demo_ui {
                    let state = ui_state(&ui);
                    let key = serde_json::to_string(&state).unwrap_or_default();
                    if key != last_ui_trace {
                        if let Err(error) =
                            append_ui_trace(path, state, tick_control.native_epdc_mode.get())
                        {
                            eprintln!(
                                "bilicomics: demonstration UI trace could not be saved: {error:#}"
                            );
                        }
                        last_ui_trace = key;
                    }
                }
                if shell.get_route() == "reader"
                    && !ui.global::<ReaderModel>().get_loading()
                    && ui.global::<ReaderModel>().get_page().size().width > 0
                {
                    let state = application.reader_lifecycle_snapshot();
                    let key = serde_json::to_string(&state).unwrap_or_default();
                    if key != last_reader_trace {
                        if let Err(error) = append_ready_trace(
                            path,
                            "reader-ready",
                            &ui,
                            state,
                            resources,
                            tick_control.native_epdc_mode.get(),
                        ) {
                            eprintln!("bilicomics: reader trace could not be saved: {error:#}");
                        }
                        last_reader_trace = key;
                    }
                }
            }
            if deadline.is_some_and(|deadline| elapsed >= deadline) || application.is_quitting() {
                tick_control.quitting.set(true);
                let _ = slint::quit_event_loop();
            }
        }));
    let notified_driver = driver.clone();
    let retry_wake = wake.clone();
    ui.on_runtime_wakeup(move || {
        if let Ok(mut drive) = notified_driver.try_borrow_mut() {
            (*drive)();
        } else {
            retry_wake.request();
        }
    });
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(80), move || {
        if let Ok(mut drive) = driver.try_borrow_mut() {
            (*drive)();
        }
    });

    ui.show()?;
    if !arguments.kindle {
        ui.window()
            .set_size(slint::LogicalSize::new(width as f32, height as f32));
    }
    let _ = platform.request_refresh(RefreshReason::EnterScreen);
    record_startup_stage(arguments.lifecycle_trace.as_deref(), "window_shown", trace_moment(), None);
    let loop_result = slint::run_event_loop();
    wake.stop();
    timer.stop();
    ui.on_runtime_wakeup(|| {});
    control.quitting.set(true);
    signals.retire();
    cancelled.store(true, Ordering::Release);
    let application = control.application.borrow().clone();
    let shutdown = application
        .as_ref()
        .map(|app| app.borrow_mut().shutdown())
        .transpose();
    if let Some(path) = arguments.capture.as_deref() {
        let offline = application
            .as_ref()
            .map_or(arguments.offline, |app| app.borrow().is_offline());
        capture_window(
            &ui,
            path,
            &arguments,
            width,
            height,
            offline,
            control.native_epdc_mode.get(),
        )?;
    }
    ui.hide()?;
    *control.application.borrow_mut() = None;
    runtime.shutdown_timeout(Duration::from_secs(3));
    // Futures that had not observed a response retain their pre-submission
    // journal for reconciliation. Flush any observations made while shutting
    // down once more, after asynchronous execution has been retired.
    let final_receipts = application
        .as_ref()
        .map(|app| app.borrow_mut().flush_observed_receipts())
        .transpose();
    drop(application);
    drop(signals);
    loop_result?;
    shutdown?;
    final_receipts?;
    if !failures.borrow().is_empty() {
        bail!("{}", failures.borrow().join("\n"));
    }
    Ok(())
}

fn start_startup_workers(
    handle: &tokio::runtime::Handle,
    options: AppOptions,
    delay_ms: u64,
    cancelled: Arc<AtomicBool>,
    sender: mpsc::Sender<StartupEvent>,
    wake: RuntimeWake,
    trace_enabled: bool,
) {
    let font_sender = sender.clone();
    let font_wake = wake.clone();
    let font_cancelled = cancelled.clone();
    bilicomics::performance::startup("font_worker_queued", &());
    let font_worker = handle.spawn_blocking(move || -> Result<()> {
        bilicomics::performance::startup("font_worker_begin", &());
        let measured = bilicomics::performance::enabled().then(bilicomics::performance::Span::start);
        let result = (|| -> Result<()> {
        if trace_enabled {
            publish_startup(&font_sender, &font_wake,
                StartupEvent::Stage("font_prepare_begin", trace_moment()))?;
        }
        Fonts::prepare_faces(&font_cancelled, |face| {
            publish_startup(&font_sender, &font_wake, StartupEvent::Font(face, trace_moment()))
        })?;
        if trace_enabled && !font_cancelled.load(Ordering::Acquire) {
            publish_startup(&font_sender, &font_wake,
                StartupEvent::Stage("fonts_prepared", trace_moment()))?;
        }
        Ok(())
        })();
        if let Some(measured) = measured {
            bilicomics::performance::startup("font_worker_end", &serde_json::json!({
                "elapsed": measured.finish(), "success": result.is_ok(),
                "cancelled": font_cancelled.load(Ordering::Acquire),
            }));
        }
        result
    });
    observe_startup_worker(handle, font_worker, sender.clone(), wake.clone());

    let state_sender = sender.clone();
    let state_wake = wake.clone();
    bilicomics::performance::startup("state_worker_queued", &());
    let state_worker = handle.spawn_blocking(move || -> Result<()> {
        bilicomics::performance::startup("state_worker_begin", &());
        let measured = bilicomics::performance::enabled().then(bilicomics::performance::Span::start);
        let result = (|| -> Result<()> {
        if delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(delay_ms));
        }
        if cancelled.load(Ordering::Acquire) { return Ok(()); }
        if trace_enabled {
            publish_startup(&state_sender, &state_wake,
                StartupEvent::Stage("state_prepare_begin", trace_moment()))?;
        }
        let prepared = App::prepare(options)?;
        if cancelled.load(Ordering::Acquire) { return Ok(()); }
        publish_startup(&state_sender, &state_wake, StartupEvent::State(prepared, trace_moment()))
        })();
        if let Some(measured) = measured {
            bilicomics::performance::startup("state_worker_end", &serde_json::json!({
                "elapsed": measured.finish(), "success": result.is_ok(),
                "cancelled": cancelled.load(Ordering::Acquire),
            }));
        }
        result
    });
    observe_startup_worker(handle, state_worker, sender, wake);
}

fn publish_startup(sender: &mpsc::Sender<StartupEvent>, wake: &RuntimeWake, event: StartupEvent) -> Result<()> {
    sender.send(event).map_err(|_| anyhow::anyhow!("Startup window closed"))?;
    wake.request();
    Ok(())
}

fn observe_startup_worker(
    handle: &tokio::runtime::Handle,
    worker: tokio::task::JoinHandle<Result<()>>,
    sender: mpsc::Sender<StartupEvent>,
    wake: RuntimeWake,
) {
    handle.spawn(async move {
        let failure = match worker.await {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(format!("{error:#}")),
            Err(error) => Some(format!("Startup worker stopped: {error}")),
        };
        if let Some(error) = failure {
            let _ = publish_startup(&sender, &wake, StartupEvent::Failed(error));
        }
    });
}

fn record_performance_gate(current: &mut Option<&'static str>, gate: &'static str) {
    if bilicomics::performance::enabled() && *current != Some(gate) {
        *current = Some(gate);
        bilicomics::performance::startup("startup_gate", &serde_json::json!({"gate": gate}));
    }
}

fn record_startup_stage(path: Option<&Path>, stage: &'static str, moment: TraceMoment, font_count: Option<usize>) {
    if bilicomics::performance::enabled() {
        bilicomics::performance::startup(stage, &serde_json::json!({
            "observed": moment, "registered_fonts": font_count,
        }));
    }
    let Some(path) = path else { return; };
    let record = serde_json::json!({
        "process_id": std::process::id(), "event": "startup-stage", "stage": stage,
        "monotonic_ns": moment.monotonic_ns, "elapsed_ms": moment.elapsed_ms,
        "registered_fonts": font_count,
        "timing_scope": "Application preparation or UI rendering; not physical panel completion"
    });
    if let Err(error) = append_trace_record(path, record) {
        eprintln!("bilicomics: startup timing could not be saved: {error:#}");
    }
}

fn anonymous_checkpoint_state(resources: &serde_json::Value) -> Option<serde_json::Value> {
    let checkpoint = resources.get("reader_checkpoint")?.as_object()?;
    let mut output = serde_json::Map::new();
    for name in ["pending", "in_flight"] {
        if let Some(value) = checkpoint.get(name).and_then(serde_json::Value::as_bool) {
            output.insert(name.into(), value.into());
        }
    }
    for name in ["failed_attempts"] {
        if let Some(value) = checkpoint.get(name).and_then(serde_json::Value::as_u64) {
            output.insert(name.into(), value.into());
        }
    }
    (!output.is_empty()).then_some(serde_json::Value::Object(output))
}

fn append_trace_record(path: &Path, mut record: serde_json::Value) -> Result<()> {
    use std::io::Write;
    static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let moment = trace_moment();
    if let Some(object) = record.as_object_mut() {
        object.entry("monotonic_ns").or_insert_with(|| serde_json::json!(moment.monotonic_ns));
        object.entry("elapsed_ms").or_insert_with(|| serde_json::json!(moment.elapsed_ms));
    }
    let mut encoded = serde_json::to_vec(&record)?;
    encoded.push(b'\n');
    let _writer = WRITER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::OpenOptions::new().append(true).create(true).open(path)?.write_all(&encoded)?;
    Ok(())
}

fn startup_frame_path(path: &Path, index: usize) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{stem}-{index}.png"))
}

fn native_mode_name(mode: Option<NativeOutputMode>) -> Option<&'static str> {
    mode.map(|mode| if mode.inverted() { "Y8INV" } else { "Y8" })
}

fn capture_startup_frame(
    ui: &AppWindow,
    path: &Path,
    worker_started: bool,
    native_mode: Option<NativeOutputMode>,
) -> Result<()> {
    save_png(ui, path)?;
    let shell = ui.global::<ShellModel>();
    fs::write(
        path.with_extension("json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "process_id": std::process::id(), "booting": shell.get_booting(),
            "native_epdc_mode": native_mode_name(native_mode), "pixel_semantics": "Logical Slint snapshot before framebuffer compensation",
            "phase": shell.get_startup_phase().as_str(), "animation_frame": shell.get_startup_frame(),
            "worker_started": worker_started, "error": shell.get_startup_error(), "suspended": shell.get_suspended()
        }))?,
    )?;
    Ok(())
}

fn native_mode_from_inversion(inverted: bool) -> NativeOutputMode {
    if inverted {
        NativeOutputMode::Y8Inv
    } else {
        NativeOutputMode::Y8
    }
}

fn capture_resume_stage(stages: &mut Vec<serde_json::Value>, enabled: bool, stage: &str) {
    if !enabled {
        return;
    }
    let uptime = fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|value| value.split_whitespace().next().map(str::to_owned));
    stages.push(serde_json::json!({"stage": stage, "uptime_seconds": uptime}));
}

fn capture_native_portrait_stage(
    stages: &mut Vec<serde_json::Value>,
    enabled: bool,
    holding: NativePortraitDirection,
    scanout: NativePortraitDirection,
    independent_pose: bool,
) {
    if !enabled {
        return;
    }
    capture_resume_stage(stages, true, "portrait_direction_ready");
    let direction_name = |direction| match direction {
        NativePortraitDirection::Up => "U",
        NativePortraitDirection::Down => "D",
    };
    if let Some(record) = stages.last_mut().and_then(serde_json::Value::as_object_mut) {
        record.insert("holding_direction".into(), direction_name(holding).into());
        record.insert("scanout_direction".into(), direction_name(scanout).into());
        record.insert("pose_source".into(), if independent_pose { "evdev-native-portrait" } else { "framebuffer-calibration" }.into());
    }
}

fn append_power_trace(
    path: &Path,
    event: PowerEvent,
    ack: PowerAck,
    ui: &AppWindow,
    reader_state: Option<serde_json::Value>,
    native_mode: Option<NativeOutputMode>,
    resume_stages: &[serde_json::Value],
) -> Result<()> {
    let reader = ui.global::<ReaderModel>();
    let record = serde_json::json!({ "process_id": std::process::id(), "event": match event {
        PowerEvent::Suspend => "suspend", PowerEvent::Resume => "resume", PowerEvent::Quit => "quit" },
        "checkpoint_saved": ack.checkpoint_saved, "route": ui.global::<ShellModel>().get_route().as_str(),
        "native_epdc_mode": native_mode_name(native_mode),
        "resume_stages": resume_stages,
        "page": reader.get_page_index(), "comic_title": reader.get_comic_title().as_str(),
        "episode_title": reader.get_episode_title().as_str(), "mode": reader.get_mode().as_str(),
        "booting": ui.global::<ShellModel>().get_booting(), "suspended": ui.global::<ShellModel>().get_suspended(), "reader_state": reader_state });
    append_trace_record(path, record)
}

fn append_ready_trace(
    path: &Path,
    event: &str,
    ui: &AppWindow,
    reader_state: Option<serde_json::Value>,
    reader_resources: serde_json::Value,
    native_mode: Option<NativeOutputMode>,
) -> Result<()> {
    let reader = ui.global::<ReaderModel>();
    append_trace_record(path, serde_json::json!({
            "process_id": std::process::id(), "event": event,
            "native_epdc_mode": native_mode_name(native_mode),
            "route": ui.global::<ShellModel>().get_route().as_str(),
            "reader_loading": reader.get_loading(),
            "frame_size": [reader.get_page().size().width, reader.get_page().size().height],
            "reader_state": reader_state, "reader_resources": reader_resources
        }))
}

fn append_reader_closed_trace(
    path: &Path,
    ui: &AppWindow,
    generation: u64,
    reader_resources: &serde_json::Value,
    native_mode: Option<NativeOutputMode>,
) -> Result<()> {
    append_trace_record(path, serde_json::json!({
            "process_id": std::process::id(), "event": "reader-closed",
            "native_epdc_mode": native_mode_name(native_mode),
            "reader_generation": generation, "route": ui.global::<ShellModel>().get_route().as_str(),
            "reader_resources": reader_resources
        }))
}

fn save_png(ui: &AppWindow, path: &Path) -> Result<(u32, u32)> {
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let snapshot = ui.window().take_snapshot()?;
    image::save_buffer(
        path,
        snapshot.as_bytes(),
        snapshot.width(),
        snapshot.height(),
        image::ColorType::Rgba8,
    )?;
    Ok((snapshot.width(), snapshot.height()))
}

fn ui_state(ui: &AppWindow) -> serde_json::Value {
    let catalog = ui.global::<CatalogModel>();
    let downloads = ui.global::<DownloadModel>();
    let candidates = catalog.get_candidates();
    let candidates = (0..candidates.row_count())
        .filter_map(|index| candidates.row_data(index))
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    let items = downloads.get_items();
    let items = (0..items.row_count()).filter_map(|index| items.row_data(index)).map(|item| serde_json::json!({
        "id": item.id.as_str(), "status": item.status.as_str(), "progress": item.progress,
        "error": item.error, "active": item.active, "has_bar": item.has_bar, "selected": item.selected
    })).collect::<Vec<_>>();
    serde_json::json!({
        "route": ui.global::<ShellModel>().get_route().as_str(),
        "toast": ui.global::<ShellModel>().get_toast().as_str(),
        "account": {"section": ui.global::<AccountModel>().get_section().as_str()},
        "catalog": {
            "query": catalog.get_query().as_str(), "composition": catalog.get_composition().as_str(),
            "keyboard": catalog.get_keyboard_visible(), "candidate_page": catalog.get_candidate_page(),
            "candidate_pages": catalog.get_candidate_pages(), "candidates": candidates,
            "detail_id": catalog.get_detail().id.as_str(), "download_selecting": catalog.get_download_selecting(),
            "episode_page": catalog.get_episode_page(), "episode_pages": catalog.get_episode_pages(),
            "episode_filter": catalog.get_episode_filter().as_str(), "selection_count": catalog.get_selection_count(),
            "selection_size": catalog.get_selection_size().as_str()
        },
        "downloads": {
            "all_paused": downloads.get_all_paused(), "has_active": downloads.get_has_active(),
            "sheet_open": downloads.get_sheet_open(), "sheet_id": downloads.get_sheet_id().as_str(),
            "task_primary_action": downloads.get_task_primary_action().as_str(),
            "task_secondary_action": downloads.get_task_secondary_action().as_str(),
            "can_read": downloads.get_can_read(), "selected_count": downloads.get_selected_count(),
            "items": items
        },
        "reader": {
            "page": ui.global::<ReaderModel>().get_page_index(),
            "mode": ui.global::<ReaderModel>().get_mode().as_str(),
            "menu": ui.global::<ReaderModel>().get_menu(),
            "sheet": ui.global::<ReaderModel>().get_sheet().as_str(),
            "loading": ui.global::<ReaderModel>().get_loading(),
            "thumbnails_loading": ui.global::<ReaderModel>().get_thumbnails_loading(),
            "thumbnail_count": ui.global::<ReaderModel>().get_thumbnails().row_count(),
            "panel": ui.global::<ReaderModel>().get_panel_index(),
            "strip_screen": ui.global::<ReaderModel>().get_strip_screen(),
            "transition": ui.global::<ReaderModel>().get_transition()
        }
    })
}

fn append_ui_trace(
    path: &Path,
    state: serde_json::Value,
    native_mode: Option<NativeOutputMode>,
) -> Result<()> {
    append_trace_record(path, serde_json::json!({
            "process_id": std::process::id(), "event": "ui-state", "view_model_fixture_only": false,
            "native_epdc_mode": native_mode_name(native_mode),
            "demo": true, "state": state
        }))
}

fn capture_window(
    ui: &AppWindow,
    path: &Path,
    arguments: &Arguments,
    width: u32,
    height: u32,
    offline: bool,
    native_mode: Option<NativeOutputMode>,
) -> Result<()> {
    let snapshot_size = save_png(ui, path)?;
    let shell = ui.global::<ShellModel>();
    fs::write(
        path.with_extension("json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "process_id": std::process::id(),
            "native_epdc_mode": native_mode_name(native_mode), "pixel_semantics": "Logical Slint snapshot before framebuffer compensation",
            "profile": arguments.device.id(),
            "route": if shell.get_booting() { "startup".into() } else { shell.get_route() }.as_str(),
            "backend": if arguments.kindle { "kindle" } else { "desktop-software" },
            "demo": arguments.demo, "offline": offline, "offline_requested": arguments.offline,
            "logical_size": [width, height], "window_size": [ui.window().size().width, ui.window().size().height],
            "snapshot_size": [snapshot_size.0, snapshot_size.1],
            "landscape": arguments.landscape, "suspended": shell.get_suspended(),
            "startup": { "booting": shell.get_booting(), "phase": shell.get_startup_phase().as_str(),
                "frame": shell.get_startup_frame(), "error": shell.get_startup_error() },
            "reader": {
                "page": ui.global::<ReaderModel>().get_page_index(), "pages": ui.global::<ReaderModel>().get_page_count(),
                "comic_title": ui.global::<ReaderModel>().get_comic_title().as_str(), "episode_title": ui.global::<ReaderModel>().get_episode_title().as_str(),
                "mode": ui.global::<ReaderModel>().get_mode().as_str(), "direction": ui.global::<ReaderModel>().get_direction().as_str(),
                "menu": ui.global::<ReaderModel>().get_menu(), "sheet": ui.global::<ReaderModel>().get_sheet().as_str(),
                "loading": ui.global::<ReaderModel>().get_loading(), "transition": ui.global::<ReaderModel>().get_transition(),
                "thumbnails_loading": ui.global::<ReaderModel>().get_thumbnails_loading(),
                "thumbnail_count": ui.global::<ReaderModel>().get_thumbnails().row_count(),
                "panel": ui.global::<ReaderModel>().get_panel_index(), "strip_screen": ui.global::<ReaderModel>().get_strip_screen()
            },
            "catalog": { "shelf_view": ui.global::<CatalogModel>().get_shelf_view().as_str(), "query": ui.global::<CatalogModel>().get_query().as_str(),
                "composition": ui.global::<CatalogModel>().get_composition().as_str(), "keyboard": ui.global::<CatalogModel>().get_keyboard_visible(),
                "selection_count": ui.global::<CatalogModel>().get_selection_count() },
            "account_section": ui.global::<AccountModel>().get_section().as_str(), "download_selection": ui.global::<DownloadModel>().get_selecting(),
            "auth_step": ui.global::<AuthModel>().get_step(), "network_prompt": ui.global::<AuthModel>().get_net_visible(),
            "ui": ui_state(ui)
        }))?,
    )?;
    Ok(())
}

fn default_data_dir() -> Result<PathBuf> {
    if cfg!(target_os = "windows") {
        if let Some(path) = std::env::var_os("APPDATA").filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(path).join("bilicomics"));
        }
    } else if let Some(path) = std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path).join("bilicomics"));
    } else if let Some(path) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path).join(".local/share/bilicomics"));
    }
    bail!("No per-user data directory is available; supply --data-dir")
}

#[cfg(test)]
mod trace_tests {
    use super::*;

    #[test]
    fn checkpoint_trace_excludes_identity_and_error_payloads() {
        let resources = serde_json::json!({"reader_checkpoint": {
            "pending": true, "in_flight": false, "failed_attempts": 2,
            "account": "private-account", "comic": "private-comic",
            "anchor": {"page_id": "private-page"}, "error": "private-path",
        }});
        assert_eq!(anonymous_checkpoint_state(&resources), Some(serde_json::json!({
            "pending": true, "in_flight": false, "failed_attempts": 2,
        })));
    }

    #[test]
    fn absent_or_malformed_checkpoint_fields_do_not_produce_a_trace_payload() {
        assert_eq!(anonymous_checkpoint_state(&serde_json::json!({})), None);
        assert_eq!(anonymous_checkpoint_state(&serde_json::json!({"reader_checkpoint": {
            "pending": "private-value", "in_flight": {}, "failed_attempts": "private-error",
        }})), None);
    }
}

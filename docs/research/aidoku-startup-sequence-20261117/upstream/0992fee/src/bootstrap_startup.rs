//! Show a real scene before preparing fonts and metadata on background workers.
//!
//! This follows BiliComics' prepare/attach boundary, coalesced wake token, and
//! one-turn continuation. Existing Aidoku native ownership gates remain intact.

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use aidoku_kindle::{
    app::{App, AppOptions, PreparedApp},
    platform::{DeviceProfile, Platform, RefreshKind},
    ui::{AppWindow, Commands, State},
};
use anyhow::{Context, Result, anyhow};
use slint::ComponentHandle;

use crate::bootstrap_fonts::{Fonts, PreparedFont};

pub type Application = Rc<RefCell<App>>;
pub type ApplicationSlot = Rc<RefCell<Option<Application>>>;

enum StartupEvent {
    Font(PreparedFont),
    State(PreparedApp),
    Failed(String),
}

#[derive(Default)]
struct WakeGate {
    queued: AtomicBool,
    stopped: AtomicBool,
}

impl WakeGate {
    fn request(&self) -> bool {
        if self.stopped.load(Ordering::Acquire) || self.queued.swap(true, Ordering::AcqRel) {
            return false;
        }
        if self.stopped.load(Ordering::Acquire) {
            self.queued.store(false, Ordering::Release);
            return false;
        }
        true
    }

    fn begin_dispatch(&self) -> bool {
        self.queued.store(false, Ordering::Release);
        !self.stopped.load(Ordering::Acquire)
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.queued.store(false, Ordering::Release);
    }
}

#[derive(Clone)]
struct StartupWake {
    window: slint::Weak<AppWindow>,
    gate: Arc<WakeGate>,
}

impl StartupWake {
    fn new(window: slint::Weak<AppWindow>) -> Self {
        Self {
            window,
            gate: Arc::new(WakeGate::default()),
        }
    }

    fn request(&self) {
        if !self.gate.request() {
            return;
        }
        let gate = self.gate.clone();
        if self
            .window
            .upgrade_in_event_loop(move |ui| {
                if gate.begin_dispatch() {
                    ui.global::<Commands>().invoke_job_ready();
                }
            })
            .is_err()
        {
            self.gate.queued.store(false, Ordering::Release);
        }
    }
}

struct FirstScene {
    submitted_baseline: u64,
    yielded: bool,
}

impl FirstScene {
    fn submitted(
        &mut self,
        native: bool,
        submitted_count: u64,
        rendered: bool,
        notifier: bool,
    ) -> bool {
        if !std::mem::replace(&mut self.yielded, true) {
            return false;
        }
        if native {
            return submitted_count > self.submitted_baseline;
        }
        !notifier || rendered
    }
}

pub struct Startup {
    options: Option<AppOptions>,
    profile: DeviceProfile,
    import: Option<PathBuf>,
    sender: Sender<StartupEvent>,
    receiver: Receiver<StartupEvent>,
    completed: Receiver<()>,
    completion_sender: Sender<()>,
    handles: Vec<JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
    wake: StartupWake,
    continuation_pending: Rc<Cell<bool>>,
    first_scene: FirstScene,
    fonts: Fonts,
    fonts_registered: bool,
    prepared: Option<PreparedApp>,
    workers_started: bool,
    started: bool,
    ready: bool,
    failed: bool,
    activity_at: Instant,
    activity_frame: i32,
}

impl Startup {
    pub fn new(
        ui: &AppWindow,
        options: AppOptions,
        profile: DeviceProfile,
        import: Option<PathBuf>,
        submitted_baseline: u64,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let (completion_sender, completed) = mpsc::channel();
        Self {
            options: Some(options),
            profile,
            import,
            sender,
            receiver,
            completion_sender,
            completed,
            handles: Vec::new(),
            cancelled: Arc::new(AtomicBool::new(false)),
            wake: StartupWake::new(ui.as_weak()),
            continuation_pending: Rc::new(Cell::new(false)),
            first_scene: FirstScene {
                submitted_baseline,
                yielded: false,
            },
            fonts: Fonts::default(),
            fonts_registered: false,
            prepared: None,
            workers_started: false,
            started: false,
            ready: false,
            failed: false,
            activity_at: Instant::now(),
            activity_frame: 0,
        }
    }

    pub fn failed(&self) -> bool {
        self.failed
    }

    /// Prepare bytes and metadata while the accepted initial full frame settles.
    /// UI attachment still requires FirstScene and the normal event-loop turn.
    pub fn prepare_after_submission(&mut self) {
        if self.ready || self.failed || self.cancelled.load(Ordering::Acquire) {
            return;
        }
        if let Err(error) = self.start_workers() {
            let _ = publish(
                &self.sender,
                &self.wake,
                StartupEvent::Failed(format!("{error:#}")),
            );
        }
    }

    /// Called only while the native lifecycle allows active application work.
    pub fn advance(
        &mut self,
        ui: &AppWindow,
        platform: &Rc<Platform>,
        application: &ApplicationSlot,
        desktop_rendered: bool,
        notifier: bool,
    ) -> Result<bool> {
        if self.ready || self.failed || self.cancelled.load(Ordering::Acquire) {
            return Ok(self.ready);
        }
        if !self.started {
            self.update_activity(ui, platform);
            if !self.first_scene.submitted(
                platform.is_kindle(),
                platform.sleep_status().frame_send_count,
                desktop_rendered,
                notifier,
            ) {
                self.schedule();
                return Ok(false);
            }
            aidoku_kindle::reader_timing::mark("startup_scene_submitted");
            if platform.is_kindle() {
                aidoku_kindle::reader_timing::mark("startup_feedback_submitted");
            }
            self.start_workers()?;
            self.started = true;
        }
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                StartupEvent::Font(font) => self.fonts.accept(font)?,
                StartupEvent::State(prepared) => self.prepared = Some(prepared),
                StartupEvent::Failed(error) => return Err(anyhow!(error)),
            }
        }
        if !self.fonts_registered {
            if self.fonts.can_register_next() {
                self.fonts_registered = self.fonts.register_next()?;
                if self.fonts_registered {
                    self.fonts.finish(ui)?;
                }
            }
            if !self.fonts_registered {
                self.update_activity(ui, platform);
                if self.fonts.can_register_next() {
                    self.schedule();
                }
                return Ok(false);
            }
        }
        let attached = application.borrow().clone();
        if attached.is_none() {
            let Some(prepared) = self.prepared.take() else {
                ui.global::<State>().set_startup_stage(1);
                self.update_activity(ui, platform);
                return Ok(false);
            };
            // Keep the current loading scene until the business view is ready.
            // A separate attachment label would add an update just before it.
            aidoku_kindle::reader_timing::mark("startup_attachment_started");
            let app = App::attach_prepared(ui, prepared, platform.clone())?;
            *application.borrow_mut() = Some(app);
            aidoku_kindle::reader_timing::mark("application_attached");
            self.schedule();
            return Ok(false);
        }
        let app = attached.expect("The startup application is attached");
        if !app.borrow_mut().advance_startup()? {
            self.schedule();
            return Ok(false);
        }
        if let Some(path) = self.import.take() {
            app.borrow_mut().dispatch(
                "local",
                "import",
                path.to_str()
                    .context("The import path must be valid Unicode")?,
            )?;
        }
        self.ready = true;
        self.wake.gate.stop();
        ui.global::<State>().set_booting(false);
        ui.window().request_redraw();
        platform.refresh(RefreshKind::Ui);
        aidoku_kindle::reader_timing::mark("application_ready");
        aidoku_kindle::reader_timing::mark("startup_ready");
        eprintln!(
            "aidoku: {} window ready; profile={}, logical={}x{}, scale={}",
            if platform.is_kindle() {
                "native"
            } else {
                "desktop"
            },
            platform.profile().id(),
            ui.get_canvas_width(),
            ui.get_canvas_height(),
            ui.window().scale_factor()
        );
        Ok(true)
    }

    /// Animate resource waits without adding display work to final attachment.
    fn update_activity(&mut self, ui: &AppWindow, platform: &Platform) {
        let now = Instant::now();
        if now.saturating_duration_since(self.activity_at)
            >= crate::bootstrap_presentation::ACTIVITY_INTERVAL
        {
            self.activity_at = now;
            self.activity_frame = (self.activity_frame + 1) % 4;
            ui.global::<State>().set_startup_frame(self.activity_frame);
            ui.window().request_redraw();
            platform.refresh(RefreshKind::Fast);
            aidoku_kindle::reader_timing::mark("startup_activity_pulse");
        }
    }

    pub fn fail(&mut self, ui: &AppWindow, platform: &Platform) {
        self.failed = true;
        self.cancel();
        ui.global::<State>().set_startup_failed(true);
        ui.window().request_redraw();
        platform.refresh(RefreshKind::Ui);
    }

    pub fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.wake.gate.stop();
        for event in self.receiver.try_iter() {
            if let StartupEvent::State(prepared) = event {
                self.prepared = Some(prepared);
            }
        }
        if let Some(prepared) = &self.prepared {
            prepared.shutdown();
        }
    }

    /// Wait only after the event loop ends; never block its animation or input.
    pub fn finish(&mut self) -> bool {
        self.cancel();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut completed = 0;
        while completed < self.handles.len() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() || self.completed.recv_timeout(remaining).is_err() {
                break;
            }
            completed += 1;
        }
        let all_completed = completed == self.handles.len();
        let mut finished = true;
        for handle in self.handles.drain(..) {
            if all_completed || handle.is_finished() {
                let _ = handle.join();
            } else {
                finished = false;
            }
        }
        self.cancel();
        if let Some(prepared) = self.prepared.take() {
            finished &=
                prepared.finish_shutdown(deadline.saturating_duration_since(Instant::now()));
        }
        finished
    }

    pub fn schedule(&self) {
        if self.continuation_pending.replace(true) {
            return;
        }
        let pending = self.continuation_pending.clone();
        let wake = self.wake.clone();
        slint::Timer::single_shot(Duration::from_millis(1), move || {
            pending.set(false);
            wake.request();
        });
    }

    fn start_workers(&mut self) -> Result<()> {
        if self.workers_started {
            return Ok(());
        }
        // Latch before the first spawn: a partial failure must never spawn twice.
        self.workers_started = true;
        let options = self
            .options
            .take()
            .context("Startup preparation was already started")?;
        let profile = self.profile;
        self.spawn("aidoku-fonts", |cancelled, sender, wake| {
            Fonts::prepare_faces(cancelled, |face| {
                publish(sender, wake, StartupEvent::Font(face))
            })
        })?;
        self.spawn("aidoku-startup", move |cancelled, sender, wake| {
            if cancelled.load(Ordering::Acquire) {
                return Ok(());
            }
            let prepared = App::prepare(options, profile)?;
            if cancelled.load(Ordering::Acquire) {
                prepared.shutdown();
                let _ = prepared.finish_shutdown(Duration::from_secs(2));
                return Ok(());
            }
            publish(sender, wake, StartupEvent::State(prepared))
        })?;
        aidoku_kindle::reader_timing::mark("startup_workers_started");
        Ok(())
    }

    fn spawn(
        &mut self,
        name: &str,
        work: impl FnOnce(&AtomicBool, &Sender<StartupEvent>, &StartupWake) -> Result<()>
        + Send
        + 'static,
    ) -> Result<()> {
        let cancelled = self.cancelled.clone();
        let sender = self.sender.clone();
        let wake = self.wake.clone();
        let completed = self.completion_sender.clone();
        let handle = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    work(&cancelled, &sender, &wake)
                }));
                let error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(format!("{error:#}")),
                    Err(_) => Some("A startup preparation worker panicked".to_owned()),
                };
                if let Some(error) = error
                    && !cancelled.load(Ordering::Acquire)
                {
                    let _ = publish(&sender, &wake, StartupEvent::Failed(error));
                }
                let _ = completed.send(());
            })
            .context("Start a background preparation worker")?;
        self.handles.push(handle);
        Ok(())
    }
}

impl Drop for Startup {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn publish(sender: &Sender<StartupEvent>, wake: &StartupWake, event: StartupEvent) -> Result<()> {
    sender
        .send(event)
        .map_err(|_| anyhow!("The startup window is no longer available"))?;
    wake.request();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_attachment_requires_a_new_native_send_and_a_yielded_turn() {
        let mut scene = FirstScene {
            submitted_baseline: 7,
            yielded: false,
        };
        assert!(!scene.submitted(true, 8, true, true));
        assert!(!scene.submitted(true, 7, true, true));
        assert!(scene.submitted(true, 8, false, false));
    }

    #[test]
    fn a_startup_wake_burst_owns_one_slot_until_dispatch() {
        let gate = Arc::new(WakeGate::default());
        let accepted = std::thread::scope(|scope| {
            let threads = (0..16)
                .map(|_| {
                    let gate = gate.clone();
                    scope.spawn(move || usize::from(gate.request()))
                })
                .collect::<Vec<_>>();
            threads
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .sum::<usize>()
        });
        assert_eq!(accepted, 1);
        assert!(gate.begin_dispatch());
        assert!(gate.request());
        gate.stop();
        assert!(!gate.begin_dispatch());
        assert!(!gate.request());
    }
}

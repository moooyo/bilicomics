//! Opt-in private Perf05 App-library measurements on the Linux host.
//!
//! This file is intentionally not registered by production source. The remote
//! measurement checkout registers the same `#[cfg(test)]` module in baseline
//! and final builds. Run only the ignored test in a fresh test process.
//! Scripted App intents, software rendering, and Desktop DeviceControl are not
//! physical Kindle input, an EPDC waveform, SEND, or hardware timing evidence.

#![cfg(target_os = "linux")]

use super::*;
use crate::{fonts::Fonts, performance, platform::ReaderRefreshScene, ui::SleepScreenModel};
use anyhow::ensure;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use std::{fs, io::Write};

const WIDTH: u32 = 1860;
const HEIGHT: u32 = 2480;
const NATURAL_CADENCE_MS: u64 = 1500;
const COMIC: &str = "999990005";
const EPISODE: &str = "99999000501";
const TIMEOUT: Duration = Duration::from_secs(90);

struct SoftwarePlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl slint::platform::Platform for SoftwarePlatform {
    fn create_window_adapter(&self) -> std::result::Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

struct Runner {
    window: Rc<MinimalSoftwareWindow>,
    ui: AppWindow,
    app: Rc<RefCell<App>>,
    pixels: slint::SharedPixelBuffer<slint::Rgb8Pixel>,
    events: Vec<Value>,
    checkpoints: Vec<Value>,
    pending: Vec<&'static str>,
    refresh_mode: String,
    trace_path: PathBuf,
}

fn stamp() -> Value {
    serde_json::to_value(performance::Stamp::now()).expect("Serialize numeric timing")
}

fn env_path(name: &str) -> Result<PathBuf> {
    let path = PathBuf::from(std::env::var(name).with_context(|| format!("Set {name}"))?);
    ensure!(path.is_absolute(), "The private input and output paths must be absolute");
    Ok(path)
}

fn file_hash(path: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}

fn load_trace(path: &Path) -> Result<Vec<Value>> {
    let bytes = fs::read(path)?;
    // An appending writer can leave one currently incomplete line. Final trace
    // completeness is checked after performance::finish by the Python owner.
    bytes.split_inclusive(|byte| *byte == b'\n').filter(|line| line.ends_with(b"\n") && !line.is_empty())
        .map(|line| serde_json::from_slice(line).map_err(Into::into)).collect()
}

impl Runner {
    fn observe_pending(&mut self) {
        let current: Vec<_> = ["cover-sleep-local", "cover-sleep-source", "cover-sleep-prepare"]
            .into_iter().filter(|key| self.app.borrow().pending.contains_key(*key)).collect();
        if current != self.pending {
            self.events.push(json!({"event": "cover_pending_changed", "stamp": stamp(), "keys": current,
                "scope": "Pending includes queueing, worker execution, and UI delivery"}));
            self.pending = current;
        }
    }

    fn draw(&mut self) {
        let started = performance::Span::start();
        let drawn = self.window.draw_if_needed(|renderer| {
            renderer.render(self.pixels.make_mut_slice(), WIDTH as usize);
        });
        if drawn {
            self.events.push(json!({"event": "software_draw", "stamp": stamp(), "elapsed": started.finish()}));
        }
    }

    fn tick(&mut self) -> Result<()> {
        slint::platform::update_timers_and_animations();
        self.observe_pending();
        self.app.borrow_mut().tick()?;
        if !self.app.borrow().is_starting() {
            self.ui.global::<ShellModel>().set_booting(false);
        }
        self.observe_pending();
        self.draw();
        Ok(())
    }

    fn intent(&mut self, phase: &'static str, action: &str, value: &str) -> Result<()> {
        let before = stamp();
        self.app.borrow_mut().dispatch("reader", action, value)?;
        self.events.push(json!({"event": "scripted_intent", "phase": phase, "action": action,
            "before": before, "after": stamp(), "delivery": "Direct App::dispatch on the owning UI thread"}));
        self.observe_pending();
        Ok(())
    }

    fn wait(&mut self, description: &str, mut predicate: impl FnMut(&Self) -> Result<bool>) -> Result<()> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            self.tick()?;
            if predicate(self)? { return Ok(()); }
            ensure!(Instant::now() < deadline, "Timed out waiting for {description}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn idle(&mut self, duration: Duration) -> Result<()> {
        self.events.push(json!({"event": "idle_begin", "requested_ms": duration.as_millis(), "stamp": stamp()}));
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            self.tick()?;
            std::thread::sleep(Duration::from_millis(5));
        }
        self.events.push(json!({"event": "idle_end", "stamp": stamp()}));
        Ok(())
    }

    fn ready_state(&self, page: u64) -> Option<Value> {
        let app = self.app.borrow();
        if app.is_starting() || app.route != "reader" { return None; }
        let view = self.ui.global::<ReaderModel>();
        if view.get_loading() || view.get_page().size().width == 0 { return None; }
        let state = app.reader_lifecycle_snapshot()?;
        (state["page_number"] == page && state["rendering"] == false && state["menu"] == false
            && state["sheet"] == "" && state["transition"] == false).then_some(state)
    }

    fn wait_page(&mut self, page: u64) -> Result<()> {
        self.wait("a current completed reader page", |runner| Ok(runner.ready_state(page).is_some()))
    }

    fn checkpoint(&mut self, name: &'static str, page: u64) -> Result<()> {
        self.wait_page(page)?;
        let fast = self.refresh_mode == "fast";
        let started = Instant::now();
        let deadline = started + Duration::from_secs(30);
        let mut previous_hash = None;
        let mut identical = 0;
        let mut logged_failure = false;
        let (state, rgb_sha256) = loop {
            // Model publication precedes a real draw. Require the current
            // content-only scene and visible overlay flags as well as two
            // actual matching software paints; never threshold a gray frame.
            self.tick()?;
            self.ui.window().request_redraw();
            self.draw();
            let view = self.ui.global::<ReaderModel>();
            let shell = self.ui.global::<ShellModel>();
            let scene = self.app.borrow().platform.reader_refresh_scene();
            let window = self.ui.window().size();
            let image = view.get_page().size();
            let scale = self.ui.window().scale_factor();
            let overlays_hidden = shell.get_route() == "reader" && !shell.get_booting()
                && !shell.get_suspended() && shell.get_toast().is_empty()
                && !self.ui.global::<AuthModel>().get_net_visible() && !view.get_guidance()
                && !view.get_preparing_feedback() && !view.get_failed() && !view.get_loading()
                && !view.get_menu() && view.get_sheet().is_empty() && !view.get_transition()
                && !self.ui.global::<SleepScreenModel>().get_active();
            let scene_matches = scene == if fast { ReaderRefreshScene::FastContent } else { ReaderRefreshScene::Normal };
            let geometry_matches = window.width == WIDTH && window.height == HEIGHT && scale == 2.0
                && image.width == WIDTH && image.height == HEIGHT;
            let mut unexpected_pixels = 0_u64;
            let mut nongray_pixels = 0_u64;
            let mut minimum = 255_u8;
            let mut maximum = 0_u8;
            for pixel in self.pixels.as_slice() {
                let gray = pixel.r == pixel.g && pixel.g == pixel.b;
                let expected_tone = if fast { pixel.r == 0 || pixel.r == 255 } else { pixel.r % 17 == 0 };
                nongray_pixels += u64::from(!gray);
                unexpected_pixels += u64::from(!gray || !expected_tone);
                minimum = minimum.min(pixel.r);
                maximum = maximum.max(pixel.r);
            }
            let current = self.ready_state(page);
            let valid = current.is_some() && overlays_hidden && scene_matches && geometry_matches
                && unexpected_pixels == 0 && minimum < maximum;
            if valid {
                let hash = format!("{:x}", Sha256::digest(self.pixels.as_bytes()));
                identical = if previous_hash.as_ref() == Some(&hash) { identical + 1 } else { 1 };
                previous_hash = Some(hash.clone());
                if identical >= 2 { break (current.context("The current page disappeared")?, hash); }
            } else {
                identical = 0;
                previous_hash = None;
            }
            if (!valid && !logged_failure) || Instant::now() >= deadline {
                // Failure diagnostics contain numeric geometry and flags only.
                // Never export toast text, source pixels, or a comic screenshot.
                eprintln!("perf05_checkpoint_diagnostics {}", json!({
                    "checkpoint_number": self.checkpoints.len() + 1, "expected_page": page,
                    "elapsed_ms": started.elapsed().as_millis(), "window_physical": [window.width, window.height],
                    "expected_logical": [WIDTH / 2, HEIGHT / 2], "scale_factor": scale,
                    "reader_image_physical": [image.width, image.height], "geometry_matches": geometry_matches,
                    "page_ready": current.is_some(), "shell_reader_route": shell.get_route() == "reader",
                    "booting": shell.get_booting(), "suspended": shell.get_suspended(),
                    "toast_visible": !shell.get_toast().is_empty(), "net_visible": self.ui.global::<AuthModel>().get_net_visible(),
                    "guidance": view.get_guidance(), "preparing_feedback": view.get_preparing_feedback(),
                    "failed": view.get_failed(), "loading": view.get_loading(), "menu": view.get_menu(),
                    "sheet_visible": !view.get_sheet().is_empty(), "transition": view.get_transition(),
                    "sleep_active": self.ui.global::<SleepScreenModel>().get_active(),
                    "scene_fast_content": scene == ReaderRefreshScene::FastContent,
                    "scene_fast_overlay": scene == ReaderRefreshScene::FastOverlay,
                    "scene_normal": scene == ReaderRefreshScene::Normal,
                    "unexpected_pixels": unexpected_pixels, "nongray_pixels": nongray_pixels,
                    "minimum": minimum, "maximum": maximum, "identical_paints": identical
                }));
                logged_failure = true;
            }
            ensure!(Instant::now() < deadline,
                "The actual quiet software frame did not reach its strict scene, overlay, geometry, and tone contract");
            std::thread::sleep(Duration::from_millis(5));
        };
        ensure!(state["fit"] == "page" && state["presented_fit"] == "page"
            && state["refresh_mode"] == self.refresh_mode && state["fast_mode"] == fast
            && state["presented_fast_mode"] == fast && state["dither"] == true
            && state["dither_method"] == "bayer8" && state["hardware_y4_requested"] == false
            && state["hardware_dither"] == false && state["frame_width"] == WIDTH
            && state["frame_height"] == HEIGHT && state["effective_spread"] == false,
            "The actual presented reader configuration differs from the controlled workload");
        self.checkpoints.push(json!({"name": name, "page_number": page,
            "rgb_sha256": rgb_sha256, "identical_paints": identical,
            "physical_size": [WIDTH, HEIGHT], "stamp": stamp(),
            "anchor": {"page_index": state["page_index"], "source_x": state["source_x"],
                "source_y": state["source_y"], "zoom_ratio": state["zoom_ratio"], "rotation": state["rotation"]},
            "reader_state": state, "resources": self.app.borrow().resource_snapshot()}));
        Ok(())
    }

    fn wait_prediction(&mut self) -> Result<()> {
        self.wait("an actually retained predicted frame", |runner| {
            Ok(runner.app.borrow().reader_resource_snapshot()["prepared_frame_state"] == "ready")
        })
    }

    fn cover_ready(&mut self) -> Result<bool> {
        // The probe observes the public sleep-overlay model; it does not set a
        // prepared flag, install a fabricated image, or bypass a worker gate.
        let app = &mut *self.app.borrow_mut();
        app.show_sleep_screen()?;
        let model = self.ui.global::<SleepScreenModel>();
        let size = model.get_cover().size();
        let ready = model.get_has_cover() && size.width > 480 && size.height > 640
            && size.width <= 1536 && size.height <= 2048;
        app.hide_sleep_screen();
        Ok(ready)
    }

    fn sleep_roundtrip(&mut self, enabled: bool) -> Result<Value> {
        self.idle(Duration::from_secs(3))?;
        if enabled {
            let deadline = Instant::now() + TIMEOUT;
            loop {
                self.tick()?;
                if self.pending.is_empty() && self.cover_ready()? { break; }
                ensure!(Instant::now() < deadline, "The real idle sleep-cover pipeline did not produce a high-resolution cover");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        self.checkpoint("idle_retained", 6)?;
        self.events.push(json!({"event": "sleep_begin", "stamp": stamp()}));
        self.app.borrow_mut().begin_suspend_intent_gate();
        self.app.borrow_mut().show_sleep_screen()?;
        self.draw();
        let model = self.ui.global::<SleepScreenModel>();
        let cover = model.get_cover().size();
        ensure!(model.get_active() && model.get_has_cover() == enabled,
            "The real sleep-overlay presentation differs from its single cover preference");
        let sleep = json!({"status": "observed", "cover_enabled": enabled,
            "cover_physical_size": [cover.width, cover.height],
            "rgb_sha256": format!("{:x}", Sha256::digest(self.pixels.as_bytes())), "stamp": stamp(),
            "scope": "Controlled host sleep overlay; no native display or power transition"});
        self.app.borrow_mut().hide_sleep_screen();
        self.app.borrow_mut().resume()?;
        self.checkpoint("sleep_resume", 6)?;
        self.events.push(json!({"event": "sleep_end", "stamp": stamp()}));
        Ok(sleep)
    }
}

fn execute() -> Result<()> {
    ensure!(cfg!(target_os = "linux") && std::env::var("BILICOMICS_REMOTE_VERIFICATION").ok().as_deref() == Some("1"),
        "Run this ignored measurement only on the designated Linux worker");
    let data = env_path("PERF05_DATA")?;
    let output = env_path("PERF05_OUTPUT")?;
    let fixture = env_path("PERF05_FIXTURE")?;
    ensure!(data.starts_with(&output) && data != output, "The measurement data must be a private run clone");
    ensure!(fs::read_link("/proc/self/ns/net")? != fs::read_link(format!("/proc/{}/ns/net", unsafe { libc::getppid() }))?,
        "Run the test executable in a private unshare -n network namespace");
    let manifest: Value = serde_json::from_slice(&fs::read(fixture.join("manifest.json"))?)?;
    ensure!(manifest["kind"] == "perf05_private_guest_png_v1" && manifest["page_count"] == 8
        && manifest["dimensions"] == json!([2480, 3508]), "Use the private original Perf05 receipt fixture");
    ensure!(manifest["synthetic_scope"]["account"] == "guest" && manifest["synthetic_scope"]["comic_id"] == COMIC
        && manifest["synthetic_scope"]["episode_id"] == EPISODE && manifest["synthetic_scope"]["remote_identity_claimed"] == false,
        "The fixture must bind the fixed synthetic numeric identifiers without an upstream identity claim");
    let refresh_mode = std::env::var("PERF05_REFRESH_MODE")?;
    ensure!(matches!(refresh_mode.as_str(), "fast" | "normal"), "Select the normalized reader refresh mode");
    let cover = std::env::var("PERF05_COVER")?;
    ensure!(matches!(cover.as_str(), "on" | "off"), "Select a single cover preference");
    let enabled = cover == "on";
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(SoftwarePlatform { window: window.clone() }))?;
    let ui = AppWindow::new()?;
    ui.window().dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 2.0 });
    ui.window().set_size(slint::PhysicalSize::new(WIDTH, HEIGHT));
    ui.window().try_dispatch_event(slint::platform::WindowEvent::Resized {
        size: slint::LogicalSize::new((WIDTH / 2) as f32, (HEIGHT / 2) as f32),
    })?;
    ui.global::<Theme>().set_scribe(true);
    ui.global::<ShellModel>().set_landscape(false);
    ui.global::<ShellModel>().set_booting(true);
    // Font work is outside the reader measurement window and identical in both
    // runner builds. No comic bytes or image buffers are generated by the test.
    let fonts = Fonts::load()?;
    fonts.register(&ui)?;
    ui.show()?;
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_time().build()?;
    let mut platform = Platform::new(DeviceProfile::Scribe, false)?;
    ensure!(!platform.capabilities().explicit_refresh && platform.native_viewport_size().is_none(),
        "The fixture must retain Desktop DeviceControl");
    // Metadata alone admits the real sleep-cover policy. The already-created
    // control service stays DesktopControl and cannot open native hardware.
    platform.is_kindle = true;
    let trace_path = output.join("performance.jsonl");
    performance::configure(&trace_path)?;
    let started = stamp();
    let options = AppOptions { data_dir: data, profile: DeviceProfile::Scribe, demo: false, offline: true,
        initial_screen: "reader".into(), landscape: false };
    let prepared = std::thread::spawn(move || App::prepare(options)).join()
        .map_err(|_| anyhow::anyhow!("The actual preparation worker panicked"))??;
    let app = App::attach_prepared(&ui, prepared, Arc::new(platform), runtime.handle().clone())?;
    ensure!(app.borrow().demo.is_none() && app.borrow().account == "guest" && app.borrow().options.offline,
        "The library measurement must use actual isolated offline guest receipts");
    let mut runner = Runner { window, ui, app, pixels: slint::SharedPixelBuffer::new(WIDTH, HEIGHT),
        events: vec![json!({"event": "reader_start", "stamp": started})], checkpoints: Vec::new(), pending: Vec::new(),
        refresh_mode, trace_path };
    runner.checkpoint("first_read", 1)?;
    ensure!(runner.app.borrow().preferences.preload == 1 && runner.app.borrow().preferences.sleep_cover == enabled,
        "The fixture preferences did not survive real application startup");
    // Natural forward reads must not wait for optional prediction admission or
    // completion. Keep this cold-cover contention sequence separate from the
    // later deliberately retained-frame hit check.
    for (page, phase) in [(2, "natural_next_2"), (3, "natural_next_3"), (4, "natural_next_4")] {
        runner.idle(Duration::from_millis(NATURAL_CADENCE_MS))?;
        runner.intent(phase, "turn", "next")?;
        runner.checkpoint(phase, page)?;
    }
    runner.intent("return_to_start", "page", "1")?;
    runner.checkpoint("return_to_start", 1)?;
    runner.wait_prediction()?;
    let predicted_started = performance::Stamp::now().monotonic_ns.context("A Linux monotonic clock is required")?;
    runner.intent("predicted_next", "turn", "next")?;
    runner.checkpoint("predicted_next", 2)?;
    let predicted_trace = load_trace(&runner.trace_path)?;
    let predicted_operation = predicted_trace.iter().find(|row| row["kind"] == "reader"
        && row["stage"] == "navigation_request"
        && row["monotonic_ns"].as_u64().is_some_and(|at| at >= predicted_started))
        .and_then(|row| row["operation_id"].as_u64()).context("The forced-hit request has no real operation identity")?;
    ensure!(predicted_trace.iter().any(|row| row["stage"] == "prepared_frame_hit"
        && row["operation_id"] == predicted_operation),
        "A lookup candidate is insufficient: the real next-frame reuse must report its hit");
    runner.intent("repeat_back", "turn", "previous")?;
    runner.checkpoint("repeat_back", 1)?;
    runner.intent("repeat_next", "turn", "next")?;
    runner.checkpoint("repeat_next", 2)?;
    for _ in 0..4 {
        runner.intent("rapid_next", "turn", "next")?;
        let deadline = Instant::now() + Duration::from_millis(40);
        while Instant::now() < deadline {
            runner.tick()?;
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    runner.checkpoint("rapid_final", 6)?;
    let cancel_started = performance::Stamp::now().monotonic_ns.context("A Linux monotonic clock is required")?;
    runner.intent("cancel_target", "page", "8")?;
    // Do not drain the UI mailbox here: applying the target before sending the
    // next intent would test ordinary reader close instead of cancellation.
    let cancel_deadline = Instant::now() + TIMEOUT;
    loop {
        let trace = load_trace(&runner.trace_path)?;
        if trace.iter().any(|row| row["kind"] == "reader" && row["stage"] == "worker_begin"
            && row["monotonic_ns"].as_u64().is_some_and(|at| at >= cancel_started)) { break; }
        ensure!(Instant::now() < cancel_deadline, "A real foreground worker did not begin before cancellation");
        std::thread::sleep(Duration::from_millis(1));
    }
    runner.intent("cancel_leave", "shelf", "")?;
    runner.wait("reader retirement after cancellation", |runner| Ok(runner.app.borrow().reading.is_none()))?;
    runner.intent("reopen_after_cancel", "open-episode", &format!("{COMIC}:{EPISODE}"))?;
    runner.checkpoint("reopen_after_cancel", 6)?;
    let sleep = runner.sleep_roundtrip(enabled)?;
    for (left, right) in [("first_read", "return_to_start"), ("first_read", "repeat_back"),
        ("natural_next_2", "predicted_next"), ("predicted_next", "repeat_next"),
        ("rapid_final", "reopen_after_cancel"), ("rapid_final", "idle_retained"), ("rapid_final", "sleep_resume")]
    {
        let first = runner.checkpoints.iter().find(|row| row["name"] == left).context("Missing reader checkpoint")?;
        let second = runner.checkpoints.iter().find(|row| row["name"] == right).context("Missing reader checkpoint")?;
        ensure!(first["rgb_sha256"] == second["rgb_sha256"] && first["anchor"] == second["anchor"],
            "Repeated, reopened, or retained actual reader pixels or anchors changed");
    }
    runner.app.borrow_mut().shutdown()?;
    runner.ui.hide()?;
    drop(runner.app.borrow_mut().downloads.take());
    runtime.shutdown_timeout(Duration::from_secs(30));
    performance::finish();
    let trace_status = serde_json::to_value(performance::status())?;
    ensure!(trace_status["stopped_reason"] == "finished" && trace_status["output_integrity_unknown"] == false,
        "The timing trace did not finish completely");
    let trace = load_trace(&runner.trace_path)?;
    let cancelled = trace.iter().any(|row| row["kind"] == "reader"
        && row["monotonic_ns"].as_u64().is_some_and(|at| at >= cancel_started)
        && matches!(row["stage"].as_str(), Some("job_cancelled" | "cancelled_before_delivery" | "reader_cancelled_result")));
    ensure!(cancelled, "The real started job did not report a cancellation outcome");
    let result = json!({"status": "passed", "scope": "controlled_host_app_library", "process_id": std::process::id(),
        "physical_device_used": false, "native_control_used": false, "send_measured": false,
        "input_delivery": "Scripted App::dispatch intents; actual XTest is a separate guest desktop group",
        "natural_cadence_ms": NATURAL_CADENCE_MS,
        "natural_cadence_scope": "Fixed host idle after each completed checkpoint; no prediction wait",
        "event_loop": "Five-ms host polling with actual software draws; not the production event-loop latency",
        "checkpoints": runner.checkpoints, "events": runner.events, "sleep_cover": sleep,
        "cancel": {"status": "observed", "worker_began_before_retirement": true}, "trace_status": trace_status,
        "fixture_manifest_sha256": file_hash(&fixture.join("manifest.json"))?,
        "privacy": "No source image, decoded page, path, credential, or comic screenshot is exported"});
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(output.join("library-result.json"))?;
    serde_json::to_writer_pretty(&mut file, &result)?;
    file.write_all(b"\n")?;
    Ok(())
}

#[test]
#[ignore = "Requires the original private Perf05 archive and an isolated remote measurement process"]
fn perf05_controlled_host() -> Result<()> {
    execute()
}

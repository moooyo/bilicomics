//! Offline software-renderer evidence for the sleep overlay and retained Reader.
//!
//! This test never opens native display/input devices or connects to an account.

use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use bilicomics::{
    app::{App, AppOptions},
    fonts::Fonts,
    i18n,
    platform::{DeviceProfile, Platform},
    ui::{AppWindow, ReaderModel, ShellModel, SleepScreenModel, Theme},
};
use serde_json::{Value, json};
use slint::{
    ComponentHandle, Rgba8Pixel, SharedPixelBuffer,
    platform::{EventLoopProxy, WindowAdapter, software_renderer::{MinimalSoftwareWindow, RepaintBufferType}},
};

type Callback = Box<dyn FnOnce() + Send>;
type EventQueue = Arc<Mutex<VecDeque<Callback>>>;

struct OffscreenPlatform {
    window: Rc<MinimalSoftwareWindow>,
    events: EventQueue,
    started: Instant,
}

struct OffscreenProxy(EventQueue);

impl EventLoopProxy for OffscreenProxy {
    fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> { Ok(()) }

    fn invoke_from_event_loop(&self, event: Callback) -> Result<(), slint::EventLoopError> {
        self.0.lock().unwrap().push_back(event);
        Ok(())
    }
}

impl slint::platform::Platform for OffscreenPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration { self.started.elapsed() }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(OffscreenProxy(self.events.clone())))
    }
}

fn drain_events(events: &EventQueue) {
    for _ in 0..10_000 {
        let next = events.lock().unwrap().pop_front();
        let Some(next) = next else { return; };
        next();
    }
    panic!("The offline control queue did not become idle");
}

fn snapshot(ui: &AppWindow) -> Result<SharedPixelBuffer<Rgba8Pixel>> {
    slint::platform::update_timers_and_animations();
    ui.window().request_redraw();
    ui.window().take_snapshot().map_err(|error| anyhow::anyhow!("Capture the software scene: {error}"))
}

fn save_evidence(root: Option<&Path>, name: &str, pixels: &SharedPixelBuffer<Rgba8Pixel>) -> Result<()> {
    if let Some(root) = root {
        image::save_buffer(root.join(format!("{name}.png")), pixels.as_bytes(),
            pixels.width(), pixels.height(), image::ColorType::Rgba8)?;
    }
    Ok(())
}

fn paper(pixel: &Rgba8Pixel) -> bool { pixel.r >= 250 && pixel.g >= 250 && pixel.b >= 250 }

fn bounds(pixels: &SharedPixelBuffer<Rgba8Pixel>) -> Option<(u32, u32, u32, u32)> {
    let (width, height) = (pixels.width(), pixels.height());
    let mut result: Option<(u32, u32, u32, u32)> = None;
    for y in 0..height {
        for x in 0..width {
            if paper(&pixels.as_slice()[(y * width + x) as usize]) { continue; }
            result = Some(match result {
                None => (x, y, x, y),
                Some((left, top, right, bottom)) => (left.min(x), top.min(y), right.max(x), bottom.max(y)),
            });
        }
    }
    result
}

fn corner_cover(width: u32, height: u32) -> slint::Image {
    let mut pixels = SharedPixelBuffer::<slint::Rgb8Pixel>::new(width, height);
    let values = [17, 85, 153, 221];
    for y in 0..height {
        for x in 0..width {
            let quadrant = usize::from(x >= width / 2) + 2 * usize::from(y >= height / 2);
            let value = values[quadrant];
            pixels.make_mut_slice()[(y * width + x) as usize] = slint::Rgb8Pixel { r: value, g: value, b: value };
        }
    }
    slint::Image::from_rgb8(pixels)
}

fn assert_complete_cover(pixels: &SharedPixelBuffer<Rgba8Pixel>, source: (u32, u32)) -> Result<Value> {
    let (width, height) = (pixels.width(), pixels.height());
    let (left, top, right, bottom) = bounds(pixels).context("The complete sleep cover must be visible")?;
    let actual = ((right - left + 1) as f64, (bottom - top + 1) as f64);
    let scale = (f64::from(width) / f64::from(source.0)).min(f64::from(height) / f64::from(source.1));
    let expected = (f64::from(source.0) * scale, f64::from(source.1) * scale);
    ensure!((actual.0 - expected.0).abs() <= 2.0 && (actual.1 - expected.1).abs() <= 2.0,
        "The cover must fit completely without cropping or reserved chrome: {actual:?} versus {expected:?}");
    ensure!((f64::from(left) - (f64::from(width) - expected.0) / 2.0).abs() <= 2.0
        && (f64::from(top) - (f64::from(height) - expected.1) / 2.0).abs() <= 2.0,
        "The proportional cover must remain centered");
    ensure!((actual.0 / actual.1 - f64::from(source.0) / f64::from(source.1)).abs() < 0.01,
        "The complete cover must preserve its source aspect ratio");
    for (x_fraction, y_fraction, expected_gray) in [(0.12, 0.12, 17), (0.88, 0.12, 85), (0.12, 0.88, 153), (0.88, 0.88, 221)] {
        let x = left + (actual.0 * x_fraction) as u32;
        let y = top + (actual.1 * y_fraction) as u32;
        let pixel = pixels.as_slice()[(y * width + x) as usize];
        ensure!(i16::from(pixel.r).abs_diff(expected_gray) <= 2
            && pixel.r == pixel.g && pixel.g == pixel.b,
            "Every corner marker must remain visible and grayscale");
    }
    // The entire outside area is checked, including the former status/progress edges.
    for y in 0..height {
        for x in 0..width {
            if x + 2 < left || x > right + 2 || y + 2 < top || y > bottom + 2 {
                ensure!(paper(&pixels.as_slice()[(y * width + x) as usize]),
                    "A sleep cover cannot expose the underlying Reader or edge badges");
            }
        }
    }
    Ok(json!({"source_size": [source.0, source.1], "visible_bounds": [left, top, right, bottom],
        "complete": true, "proportional": true, "all_corner_markers_visible": true, "outside_is_paper": true}))
}

fn assert_text_card(pixels: &SharedPixelBuffer<Rgba8Pixel>) -> Result<Value> {
    let (width, height) = (pixels.width(), pixels.height());
    let expected = (f64::from(width) * 0.12, f64::from(height) * 0.13,
        f64::from(width) * 0.88, f64::from(height) * 0.87);
    let (left, top, right, bottom) = bounds(pixels).context("The text fallback must contain a visible card")?;
    ensure!((f64::from(left) - expected.0).abs() <= 2.0 && (f64::from(top) - expected.1).abs() <= 2.0
        && (f64::from(right) - expected.2).abs() <= 2.0 && (f64::from(bottom) - expected.3).abs() <= 2.0,
        "Long fallback text must stay inside the intended card");
    let mut body_pixels = 0;
    for y in 0..height {
        for x in 0..width {
            let pixel = &pixels.as_slice()[(y * width + x) as usize];
            if x + 3 < left || x > right + 3 || y + 3 < top || y > bottom + 3 {
                ensure!(paper(pixel), "The text fallback cannot reveal underlying UI or overflow its card");
            }
            if x > left + 20 && x + 20 < right && y > top + 20 && y + 20 < bottom && !paper(pixel) {
                body_pixels += 1;
            }
        }
    }
    ensure!(body_pixels > 100, "Fallback title and author must render visible glyphs");
    Ok(json!({"visible_bounds": [left, top, right, bottom], "body_pixels": body_pixels,
        "text_clipped_to_card": true, "outside_is_paper": true}))
}

struct GlyphMask {
    width: u32,
    height: u32,
    top: u32,
    pixels: Vec<bool>,
}

fn text_region(pixels: &SharedPixelBuffer<Rgba8Pixel>, title: bool) -> (u32, u32, u32, u32) {
    let (width, height) = (f64::from(pixels.width()), f64::from(pixels.height()));
    let (start, end) = if title { (0.18, 0.60) } else { (0.69, 0.84) };
    ((width * 0.12 + 24.0).floor() as u32, (height * (0.13 + 0.74 * start)).floor() as u32,
        (width * 0.88 - 24.0).ceil() as u32, (height * (0.13 + 0.74 * end)).ceil() as u32)
}

fn glyph_mask(pixels: &SharedPixelBuffer<Rgba8Pixel>, title: bool, rows: Option<u32>) -> Result<GlyphMask> {
    let (left, top, right, bottom) = text_region(pixels, title);
    let ink = |x, y| {
        let pixel = pixels.as_slice()[(y * pixels.width() + x) as usize];
        pixel.r < 128 && pixel.g < 128 && pixel.b < 128
    };
    let first = (top..bottom).find(|y| (left..right).any(|x| ink(x, *y)))
        .context("The fallback prefix must contain visible glyphs")?;
    let end = rows.map_or(bottom, |rows| (first + rows).min(bottom));
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for y in first..end {
        for x in left..right {
            if !ink(x, y) { continue; }
            bounds = Some(match bounds {
                None => (x, y, x, y),
                Some((left, top, right, bottom)) => (left.min(x), top.min(y), right.max(x), bottom.max(y)),
            });
        }
    }
    let (left, top, right, bottom) = bounds.context("The first fallback line must be visible")?;
    let (width, height) = (right - left + 1, bottom - top + 1);
    let mut mask = Vec::with_capacity((width * height) as usize);
    for y in top..=bottom {
        for x in left..=right { mask.push(ink(x, y)); }
    }
    Ok(GlyphMask { width, height, top, pixels: mask })
}

fn glyph_matches(source: &GlyphMask, target: &GlyphMask) -> f64 {
    let mut matched = 0;
    let mut total = 0;
    for y in 0..source.height {
        for x in 0..source.width {
            if !source.pixels[(y * source.width + x) as usize] { continue; }
            total += 1;
            let nearby = (-1..=1).any(|dy| (-1..=1).any(|dx| {
                let (x, y) = (i64::from(x) + dx, i64::from(y) + dy);
                x >= 0 && y >= 0 && x < i64::from(target.width) && y < i64::from(target.height)
                    && target.pixels[(y as u32 * target.width + x as u32) as usize]
            }));
            matched += usize::from(nearby);
        }
    }
    matched as f64 / total.max(1) as f64
}

fn assert_leading_prefixes(reference: &SharedPixelBuffer<Rgba8Pixel>, long: &SharedPixelBuffer<Rgba8Pixel>) -> Result<Value> {
    let mut scores = Vec::new();
    for title in [true, false] {
        let expected = glyph_mask(reference, title, None)?;
        let observed = glyph_mask(long, title, Some(expected.height + 2))?;
        ensure!(expected.width.abs_diff(observed.width) <= 2 && expected.height.abs_diff(observed.height) <= 2,
            "The first visible fallback line must retain the complete leading prefix");
        let score = glyph_matches(&expected, &observed).min(glyph_matches(&observed, &expected));
        ensure!(score >= 0.97, "The fallback must display its original leading glyphs instead of a vertically cropped middle line");
        let (_, top, _, bottom) = text_region(reference, title);
        let center_offset = (f64::from(expected.top) + f64::from(expected.height) / 2.0
            - f64::from(top + bottom) / 2.0).abs();
        ensure!(center_offset <= f64::from(expected.height), "A short fallback label must remain centered in its original region");
        scores.push(json!({"role": if title { "title" } else { "author" },
            "leading_glyph_match": score, "short_label_center_offset": center_offset}));
    }
    Ok(json!({"leading_prefixes_visible": true, "labels": scores}))
}

fn retained_position(snapshot: &Value) -> Value {
    let keys = ["comic_id", "episode_id", "reader_generation", "route", "revision", "page_id", "page_index",
        "crop_index", "source_x", "source_y", "zoom_ratio", "rotation", "mode", "fit", "direction"];
    Value::Object(keys.into_iter().map(|key| (key.to_owned(), snapshot[key].clone())).collect())
}

#[test]
fn sleep_screen_preserves_complete_covers_and_the_retained_reader_at_all_layouts() -> Result<()> {
    let events = EventQueue::default();
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(OffscreenPlatform {
        window: window.clone(), events: events.clone(), started: Instant::now(),
    })).map_err(|error| anyhow::anyhow!("Install the offline software platform: {error}"))?;
    let ui = AppWindow::new()?;
    let fonts = Fonts::load()?;
    fonts.register(&ui)?;
    ui.show()?;
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let _runtime_guard = runtime.enter();
    let directory = tempfile::tempdir()?;
    let evidence = std::env::var_os("BILICOMICS_SLEEP_SCREEN_EVIDENCE").map(PathBuf::from);
    if let Some(root) = evidence.as_deref() { fs::create_dir_all(root)?; }
    let mut reports = Vec::new();

    for profile in [DeviceProfile::Paperwhite5, DeviceProfile::Scribe] {
        for landscape in [false, true] {
            let (mut width, mut height) = profile.logical_size();
            if landscape { std::mem::swap(&mut width, &mut height); }
            let name = format!("{}-{}", profile.id(), if landscape { "landscape" } else { "portrait" });
            window.set_size(slint::PhysicalSize::new(width, height));
            ui.global::<Theme>().set_scribe(profile.is_scribe());
            ui.global::<ShellModel>().set_landscape(landscape);
            ui.global::<ShellModel>().set_booting(false);
            ui.global::<ShellModel>().set_route("reader".into());
            let reader = ui.global::<ReaderModel>();
            let mut underlying = SharedPixelBuffer::<slint::Rgb8Pixel>::new(20, 30);
            underlying.make_mut_slice().fill(slint::Rgb8Pixel { r: 255, g: 0, b: 255 });
            reader.set_page(slint::Image::from_rgb8(underlying));
            reader.set_loading(false);
            reader.set_menu(false);
            reader.set_guidance(false);
            reader.set_transition(false);
            reader.set_sheet("".into());
            let model = ui.global::<SleepScreenModel>();
            for source in [(200, 300), (300, 160)] {
                model.set_cover(corner_cover(source.0, source.1));
                model.set_has_cover(true);
                model.set_active(true);
                let pixels = snapshot(&ui)?;
                let checks = assert_complete_cover(&pixels, source)?;
                let capture = format!("{name}-cover-{}x{}", source.0, source.1);
                save_evidence(evidence.as_deref(), &capture, &pixels)?;
                reports.push(json!({"case": capture, "size": [width, height], "checks": checks}));
            }
            model.set_has_cover(false);
            let title_prefix = format!("{} ABC", i18n::text("network_open_comic"));
            let author_prefix = format!("{} / Author", i18n::text("app_name"));
            model.set_title(title_prefix.clone().into());
            model.set_author(author_prefix.clone().into());
            let prefix_reference = snapshot(&ui)?;
            let title = format!("{title_prefix}\n{}", "A deliberately long comic title with readable words ".repeat(12))
                .chars().take(256).collect::<String>();
            let author = format!("{author_prefix}\n{}", "Long author name ".repeat(28))
                .chars().take(128).collect::<String>();
            ensure!(title.chars().count() == 256 && author.chars().count() == 128,
                "Direct-model fixtures must respect the application's title and author scalar budgets");
            model.set_title(title.into());
            model.set_author(author.into());
            let text = snapshot(&ui)?;
            let checks = assert_text_card(&text)?;
            let prefixes = assert_leading_prefixes(&prefix_reference, &text)?;
            save_evidence(evidence.as_deref(), &format!("{name}-long-text"), &text)?;
            reports.push(json!({"case": format!("{name}-long-text"), "checks": checks, "prefixes": prefixes}));
            model.set_active(false);

            let options = AppOptions { data_dir: directory.path().join(&name), profile, demo: true,
                offline: true, initial_screen: "reader".into(), landscape };
            let prepared = App::prepare(options)?;
            let app = App::attach_prepared(&ui, prepared, Arc::new(Platform::new(profile, false)?), runtime.handle().clone())?;
            while !app.borrow_mut().advance_startup()? {}
            ui.global::<ShellModel>().set_booting(false);
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                drain_events(&events);
                app.borrow_mut().tick()?;
                if !reader.get_loading() && reader.get_page().size().width > 0
                    && app.borrow().reader_lifecycle_snapshot().is_some() { break; }
                ensure!(Instant::now() < deadline, "The offline Reader did not prepare a frame for {name}");
                std::thread::sleep(Duration::from_millis(5));
            }
            app.borrow_mut().dispatch("account", "sleep-cover", "cover")?;
            let before = snapshot(&ui)?;
            let position = retained_position(&app.borrow().reader_lifecycle_snapshot().context("The Reader must have a source anchor")?);
            app.borrow_mut().begin_suspend_intent_gate();
            app.borrow_mut().show_sleep_screen()?;
            ensure!(model.get_active(), "The real application must show its prepared overlay after the intent gate");
            let prepared_image_ready = model.get_has_cover();
            let sleeping = snapshot(&ui)?;
            save_evidence(evidence.as_deref(), &format!("{name}-app-sleep"), &sleeping)?;
            app.borrow_mut().hide_sleep_screen();
            let restored = snapshot(&ui)?;
            ensure!(before.as_bytes() == restored.as_bytes(), "Hiding the sleep overlay must restore the exact Reader pixels for {name}");
            ensure!(position == retained_position(&app.borrow().reader_lifecycle_snapshot().context("The retained Reader disappeared")?),
                "The overlay must preserve the same Reader source anchor for {name}");
            save_evidence(evidence.as_deref(), &format!("{name}-reader-restored"), &restored)?;
            reports.push(json!({"case": format!("{name}-app-roundtrip"), "pixels_restored_exactly": true,
                "source_position_retained": true, "sleep_image_ready": prepared_image_ready}));

            app.borrow_mut().resume()?;
            app.borrow_mut().dispatch("shell", "navigate", "shelf")?;
            app.borrow_mut().begin_suspend_intent_gate();
            app.borrow_mut().show_sleep_screen()?;
            ensure!(!model.get_has_cover() && model.get_title().as_str() == i18n::text("app_name")
                && model.get_author().as_str() == i18n::text("sleep_neutral"),
                "A non-Reader route must show the neutral application sleep screen");
            let neutral = snapshot(&ui)?;
            let checks = assert_text_card(&neutral)?;
            save_evidence(evidence.as_deref(), &format!("{name}-neutral"), &neutral)?;
            reports.push(json!({"case": format!("{name}-neutral"), "checks": checks}));
            app.borrow_mut().hide_sleep_screen();
            app.borrow_mut().shutdown()?;
            drop(app);
            drain_events(&events);
        }
    }
    if let Some(root) = evidence.as_deref() {
        fs::write(root.join("report.json"), serde_json::to_vec_pretty(&json!({
            "scope": "Offline Slint software rendering with synthetic covers and demo Reader data; no physical panel claim",
            "layouts": 4, "cases": reports,
        }))?)?;
    }
    ui.hide()?;
    Ok(())
}

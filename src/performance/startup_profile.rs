//! Bounded, numeric startup observations, serialized only after the app exits.

use super::{Stamp, monotonic_ns};
use serde::Serialize;
use std::sync::{Mutex, OnceLock, atomic::{AtomicBool, Ordering}};

#[cfg(all(feature = "kindle", target_os = "linux"))]
use slint_backend_kindle::{PerformanceFrameSample, PerformanceSampleKind, PerformanceTiming, ShelfRenderSample, ShelfState};

const MAX_SAMPLES: usize = 160;
#[cfg(all(feature = "kindle", target_os = "linux"))]
const MAX_FRAMES: usize = 24;

static RECORDER: OnceLock<Recorder> = OnceLock::new();

struct Recorder {
    active: AtomicBool,
    samples: Mutex<Samples>,
}

struct Samples {
    stages: Vec<Event>,
    #[cfg(all(feature = "kindle", target_os = "linux"))]
    frames: Vec<PerformanceFrameSample>,
    #[cfg(all(feature = "kindle", target_os = "linux"))]
    shelf: Option<ShelfRenderSample>,
    truncated: bool,
}

impl Default for Samples {
    fn default() -> Self {
        Self {
            stages: Vec::with_capacity(MAX_SAMPLES),
            #[cfg(all(feature = "kindle", target_os = "linux"))]
            frames: Vec::with_capacity(MAX_FRAMES),
            #[cfg(all(feature = "kindle", target_os = "linux"))]
            shelf: None,
            truncated: false,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Event {
    Mark { stage: &'static str, ns: Option<u64> },
    Span { stage: &'static str, start_ns: Option<u64>, end_ns: Option<u64>, cpu_ns: Option<u64>,
        outcome: &'static str },
}

fn active() -> Option<&'static Recorder> {
    RECORDER.get().filter(|recorder| recorder.active.load(Ordering::Acquire))
}

fn record(event: Event) {
    let Some(recorder) = active() else { return; };
    let Ok(mut samples) = recorder.samples.lock() else { return; };
    if !recorder.active.load(Ordering::Relaxed) { return; }
    if samples.stages.len() == MAX_SAMPLES {
        samples.truncated = true;
        return;
    }
    samples.stages.push(event);
}

pub fn mark(stage: &'static str) {
    if active().is_some() { mark_at(stage, monotonic_ns()); }
}

pub fn mark_at(stage: &'static str, ns: Option<u64>) {
    record(Event::Mark { stage, ns });
}

pub struct Span { stage: &'static str, start: Stamp }

pub fn span(stage: &'static str) -> Option<Span> {
    active().map(|_| Span { stage, start: Stamp::now() })
}

pub fn finish(span: Option<Span>, outcome: &'static str) {
    let Some(Span { stage, start }) = span else { return; };
    let end = Stamp::now();
    let cpu_ns = (start.owner == end.owner).then(|| end.thread_cpu_ns?.checked_sub(start.thread_cpu_ns?)).flatten();
    record(Event::Span { stage, start_ns: start.monotonic_ns, end_ns: end.monotonic_ns, cpu_ns, outcome });
}

/// The startup control may record marks from several threads. There is no
/// diagnostic file access under the collector lock or in a measured stage.
#[cfg(all(feature = "kindle", target_os = "linux"))]
pub fn configure(path: &std::path::Path, shelf_trace: &std::path::Path) -> std::io::Result<()> {
    profile_run(path, shelf_trace)?;
    RECORDER.set(Recorder { active: AtomicBool::new(true), samples: Mutex::new(Samples::default()) })
        .map_err(|_| std::io::Error::other("Startup phase observation is already configured"))
}

#[cfg(all(feature = "kindle", target_os = "linux"))]
fn profile_run<'a>(path: &std::path::Path, shelf_trace: &'a std::path::Path) -> std::io::Result<&'a str> {
    let run = super::shelf_trace_run_id(shelf_trace)?;
    if path.file_name().and_then(|name| name.to_str()).and_then(|name| name.strip_suffix(".startup-phases.json")) != Some(run)
        || path.parent() != shelf_trace.parent()
    {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Startup phase and Shelf trace paths do not identify the same private run"));
    }
    Ok(run)
}

#[cfg(all(feature = "kindle", target_os = "linux"))]
pub fn qualify(sample: ShelfRenderSample) {
    if !matches!(sample.shelf_state, ShelfState::Ready | ShelfState::Empty) || sample.rendered_monotonic_ns.is_none() {
        return;
    }
    if let Some(recorder) = active() && let Ok(mut samples) = recorder.samples.lock() {
        samples.shelf.get_or_insert(sample);
    }
}

#[cfg(all(feature = "kindle", target_os = "linux"))]
fn is_shelf_frame(sample: &PerformanceFrameSample, shelf: ShelfRenderSample) -> bool {
    let (Some(render), Some(ns)) = (sample.stages.slint_render, shelf.rendered_monotonic_ns) else { return false; };
    sample.rendered && render.started_monotonic_ns.is_some_and(|start| start <= ns)
        && render.finished_monotonic_ns.is_some_and(|end| ns <= end)
        && shelf.submission.is_none_or(|value| sample.accepted_update_marker == Some(value.update_marker))
}

/// Reuse the backend's existing wait/render clocks without enabling RGB fingerprints.
/// The backend emits the qualified Shelf immediately before its frame sample.
#[cfg(all(feature = "kindle", target_os = "linux"))]
pub fn native_frame(frame: PerformanceFrameSample) {
    let Some(recorder) = active() else { return; };
    let Ok(mut samples) = recorder.samples.lock() else { return; };
    if samples.frames.len() == MAX_FRAMES {
        samples.truncated = true;
        recorder.active.store(false, Ordering::Release);
        return;
    }
    if samples.shelf.is_some_and(|shelf| is_shelf_frame(&frame, shelf)) {
        recorder.active.store(false, Ordering::Release);
    }
    samples.frames.push(frame);
}

#[cfg(all(feature = "kindle", target_os = "linux"))]
fn timing(value: PerformanceTiming) -> serde_json::Value {
    serde_json::json!({"start_ns": value.started_monotonic_ns, "end_ns": value.finished_monotonic_ns,
        "cpu_ns": value.thread_cpu_ns})
}

#[cfg(all(feature = "kindle", target_os = "linux"))]
fn frame_json(frame: &PerformanceFrameSample) -> serde_json::Value {
    let stages = &frame.stages;
    serde_json::json!({
        "kind": match frame.kind { PerformanceSampleKind::InitialSetup => "setup", PerformanceSampleKind::Frame => "frame" },
        "sequence": frame.render_sequence, "timing": timing(frame.timing), "initial": frame.initial_frame,
        "rendered": frame.rendered, "complete_replacement": frame.complete_frame_replacement,
        "preceding_marker": frame.preceding_update_marker, "accepted_marker": frame.accepted_update_marker,
        "completed_marker": frame.completed_update_marker, "accepted": frame.refresh_accepted,
        "stages": {
            "active_io": stages.active_io.map(timing), "buffer_setup": stages.buffer_setup.map(timing),
            "window_setup": stages.window_setup.map(timing), "portrait_observer": stages.portrait_observer_setup.map(timing),
            "initial_fill": stages.initial_fill.map(timing), "preceding_wait": stages.preceding_update_wait.map(timing),
            "render": stages.slint_render.map(timing), "write": stages.framebuffer_write.map(timing),
            "submit": stages.refresh_submit.map(timing), "completion_wait": stages.refresh_complete_wait.map(timing),
            "lifecycle_complete": stages.lifecycle_complete.map(timing),
        },
    })
}

#[cfg(all(feature = "kindle", target_os = "linux"))]
pub fn write(path: &std::path::Path, shelf_trace: &std::path::Path) -> std::io::Result<()> {
    use std::{fs::{File, OpenOptions}, io::{self, Read, Write}, os::unix::fs::OpenOptionsExt};

    let run_id = profile_run(path, shelf_trace)?;
    let recorder = RECORDER.get().ok_or_else(|| io::Error::other("Startup phase recorder is unavailable"))?;
    let samples = recorder.samples.lock().map_err(|_| io::Error::other("Startup phase recorder lock failed"))?;
    let shelf = samples.shelf.filter(|&sample| samples.frames.last().is_some_and(|frame| is_shelf_frame(frame, sample)));
    let Some(shelf) = shelf.filter(|_| !samples.truncated) else {
        return Err(io::Error::other("Startup phase record has no bound complete Shelf sample"));
    };
    let mut boot = String::new();
    File::open("/proc/sys/kernel/random/boot_id")?.take(38).read_to_string(&mut boot)?;
    let header = super::ShelfRenderHeader::new(shelf_trace, &boot)?;
    let frames: Vec<_> = samples.frames.iter().map(frame_json).collect();
    let record = serde_json::json!({"schema": "kindle-startup-phases-v1", "run_id": run_id,
        "boot_id": header.boot_id, "process_id": std::process::id(), "clock": "CLOCK_MONOTONIC",
        "shelf_render_ns": shelf.rendered_monotonic_ns, "stages": samples.stages, "frames": frames});
    let mut bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
    bytes.push(b'\n');
    if bytes.len() > 96 * 1024 { return Err(io::Error::other("Startup phase record is oversized")); }
    OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?.write_all(&bytes)
}

#[cfg(all(test, feature = "kindle", target_os = "linux"))]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn phase_sidecar_must_share_its_directory_and_run_with_the_shelf_witness() {
        let shelf = Path::new("private/bili-shelf-0123456789abcdef0123456789abcdef.jsonl");
        let phases = Path::new("private/bili-shelf-0123456789abcdef0123456789abcdef.startup-phases.json");
        assert_eq!(profile_run(phases, shelf).unwrap(), "bili-shelf-0123456789abcdef0123456789abcdef");
        assert!(profile_run(shelf, shelf).is_err());
        assert!(profile_run(&Path::new("foreign").join(phases.file_name().unwrap()), shelf).is_err());
        assert!(profile_run(phases, &shelf.with_file_name("bili-shelf-not-a-run.jsonl")).is_err());
    }
}

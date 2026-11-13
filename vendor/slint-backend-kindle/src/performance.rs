//! Opt-in observations of existing rendering work, without display operations.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

use crate::{PhysicalRect, RefreshWaveform};

/// A sample describes completed software work, never physical panel completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PerformanceSampleKind {
    InitialSetup,
    Frame,
}

/// Monotonic wall time and CPU time from the one measured event-loop thread.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PerformanceTiming {
    pub started_monotonic_ns: Option<u64>,
    pub finished_monotonic_ns: Option<u64>,
    pub started_thread_cpu_ns: Option<u64>,
    pub finished_thread_cpu_ns: Option<u64>,
    pub wall_ns: Option<u64>,
    pub thread_cpu_ns: Option<u64>,
    pub same_thread: bool,
}

/// Absent stages did not run in this sample; failed clocks remain absent too.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PerformanceStageTimings {
    pub active_io: Option<PerformanceTiming>,
    pub buffer_setup: Option<PerformanceTiming>,
    pub window_setup: Option<PerformanceTiming>,
    pub portrait_observer_setup: Option<PerformanceTiming>,
    pub initial_fill: Option<PerformanceTiming>,
    pub preceding_update_wait: Option<PerformanceTiming>,
    pub slint_render: Option<PerformanceTiming>,
    pub rgb_fingerprint: Option<PerformanceTiming>,
    pub framebuffer_write: Option<PerformanceTiming>,
    pub refresh_submit: Option<PerformanceTiming>,
    pub refresh_complete_wait: Option<PerformanceTiming>,
    pub lifecycle_complete: Option<PerformanceTiming>,
}

/// One actual draw or refresh attempt can reuse an already presented Reader ID.
/// The sequence distinguishes those redraws; the marker does not request a draw.
#[derive(Clone, Debug)]
pub struct PerformanceFrameSample {
    pub kind: PerformanceSampleKind,
    pub render_sequence: u64,
    pub reader_operation_id: Option<u64>,
    pub timing: PerformanceTiming,
    pub stages: PerformanceStageTimings,
    pub framebuffer_size: Option<(u32, u32)>,
    pub framebuffer_stride: Option<usize>,
    pub framebuffer_bits_per_pixel: Option<u32>,
    pub rotation_degrees: Option<u32>,
    pub output_inverted: Option<bool>,
    pub black_and_white: Option<bool>,
    pub hardware_y4_requested: Option<bool>,
    pub hardware_y4_available: Option<bool>,
    /// The final accepted ioctl carried Y4; this is not panel-effect evidence.
    pub hardware_y4_applied: Option<bool>,
    pub software_dither_fallback: Option<bool>,
    pub rendered: bool,
    pub initial_frame: bool,
    pub forced_full: bool,
    pub complete_frame_replacement: bool,
    pub preceding_update_marker: Option<u32>,
    pub accepted_update_marker: Option<u32>,
    pub completed_update_marker: Option<u32>,
    /// FNV-1a 64 of complete row-major RGB8 pixels, only with explicit synthetic-scene opt-in.
    pub rgb_fingerprint: Option<u64>,
    pub dirty_region: Option<PhysicalRect>,
    pub refresh_region: Option<PhysicalRect>,
    pub refresh_waveform: Option<RefreshWaveform>,
    pub refresh_flashing: Option<bool>,
    pub refresh_ioctl_attempts: u32,
    pub refresh_abi: Option<&'static str>,
    pub refresh_accepted: Option<bool>,
    pub refresh_errno: Option<i32>,
    pub completion_errno: Option<i32>,
    /// The driver acknowledged this sample's accepted marker, not an optical observation.
    pub driver_completion_observed: bool,
    pub panel_completion_observed: bool,
}

type Callback = Arc<dyn Fn(PerformanceFrameSample) + Send + Sync>;

#[derive(Default)]
struct State {
    callback: Option<Callback>,
    callback_generation: u64,
    reader_operation_id: Option<u64>,
    render_sequence: u64,
    rgb_fingerprints_enabled: bool,
}

#[derive(Default)]
struct Inner {
    enabled: AtomicBool,
    state: Mutex<State>,
}

/// Disabled until a callback is installed. Sampling while disabled takes no
/// clocks, locks, callback clones, or per-frame allocations.
#[derive(Clone, Default)]
pub struct PerformanceTraceHandle {
    inner: Arc<Inner>,
}

impl PerformanceTraceHandle {
    pub fn set_callback(&self, callback: Arc<dyn Fn(PerformanceFrameSample) + Send + Sync>) {
        let previous = {
            let mut state = self.inner.state.lock().unwrap_or_else(|error| error.into_inner());
            state.callback_generation = state.callback_generation.wrapping_add(1);
            state.rgb_fingerprints_enabled = false;
            let previous = state.callback.replace(callback);
            self.inner.enabled.store(true, Ordering::Release);
            previous
        };
        drop(previous);
    }

    /// Retire samples that have not passed the callback-generation dispatch check.
    /// This does not wait for a callback already selected for dispatch.
    pub fn clear_callback(&self) {
        let previous = {
            let mut state = self.inner.state.lock().unwrap_or_else(|error| error.into_inner());
            self.inner.enabled.store(false, Ordering::Release);
            state.callback_generation = state.callback_generation.wrapping_add(1);
            state.reader_operation_id = None;
            state.rgb_fingerprints_enabled = false;
            state.callback.take()
        };
        drop(previous);
    }

    /// Associate subsequent actual draws with the currently published UI image.
    /// None clears the association on reader closure or lifecycle suspension.
    pub fn set_reader_operation_id(&self, operation_id: Option<u64>) {
        if !self.inner.enabled.load(Ordering::Acquire) {
            return;
        }
        let mut state = self.inner.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.callback.is_some() {
            state.reader_operation_id = operation_id;
        }
    }

    /// Opt in only for synthetic fixtures, never account or acquired content.
    /// Replacing or clearing the callback revokes this separate authorization.
    pub fn set_rgb_fingerprints_enabled(&self, enabled: bool) {
        let mut state = self.inner.state.lock().unwrap_or_else(|error| error.into_inner());
        state.rgb_fingerprints_enabled = enabled && state.callback.is_some();
    }

    pub(crate) fn begin(&self, kind: PerformanceSampleKind) -> Option<Recording> {
        self.begin_with_clock(kind, ClockSample::now)
    }

    fn begin_with_clock(&self, kind: PerformanceSampleKind, clock: impl FnOnce() -> ClockSample)
        -> Option<Recording>
    {
        if !self.inner.enabled.load(Ordering::Acquire) {
            return None;
        }
        let (callback, callback_generation, reader_operation_id, render_sequence, rgb_fingerprints_enabled) = {
            let mut state = self.inner.state.lock().unwrap_or_else(|error| error.into_inner());
            let callback = state.callback.as_ref()?.clone();
            if kind == PerformanceSampleKind::Frame {
                state.render_sequence = state.render_sequence.checked_add(1)?;
            }
            (callback, state.callback_generation,
                (kind == PerformanceSampleKind::Frame).then_some(state.reader_operation_id).flatten(),
                if kind == PerformanceSampleKind::Frame { state.render_sequence } else { 0 },
                kind == PerformanceSampleKind::Frame && state.rgb_fingerprints_enabled)
        };
        let started = clock();
        Some(Recording {
            handle: self.clone(), callback, callback_generation, started, rgb_fingerprints_enabled,
            sample: PerformanceFrameSample {
                kind, render_sequence, reader_operation_id,
                timing: PerformanceTiming::default(), stages: PerformanceStageTimings::default(),
                framebuffer_size: None, framebuffer_stride: None, framebuffer_bits_per_pixel: None,
                rotation_degrees: None, output_inverted: None, black_and_white: None,
                hardware_y4_requested: None, hardware_y4_available: None,
                hardware_y4_applied: None, software_dither_fallback: None,
                rendered: false, initial_frame: false, forced_full: false,
                complete_frame_replacement: false, preceding_update_marker: None,
                accepted_update_marker: None, completed_update_marker: None,
                rgb_fingerprint: None, dirty_region: None,
                refresh_region: None, refresh_waveform: None, refresh_flashing: None,
                refresh_ioctl_attempts: 0, refresh_abi: None, refresh_accepted: None, refresh_errno: None,
                completion_errno: None, driver_completion_observed: false,
                panel_completion_observed: false,
            },
        })
    }
}

/// Disabled or unauthorized observations do not traverse pixels or read clocks.
pub(crate) fn observe_rgb_frame(recording: Option<&mut Recording>, pixels: &[slint::Rgb8Pixel]) {
    observe_rgb_frame_with(recording, || rgb_fingerprint(pixels));
}

fn observe_rgb_frame_with(recording: Option<&mut Recording>, fingerprint: impl FnOnce() -> u64) {
    let Some(recording) = recording else { return; };
    if !recording.rgb_fingerprints_enabled { return; }
    let authorized = {
        let state = recording.handle.inner.state.lock().unwrap_or_else(|error| error.into_inner());
        state.callback_generation == recording.callback_generation && state.rgb_fingerprints_enabled
    };
    if !authorized { return; }
    let started = ClockSample::now();
    recording.sample.rgb_fingerprint = Some(fingerprint());
    recording.sample.stages.rgb_fingerprint = Some(started.finish());
}

fn rgb_fingerprint(pixels: &[slint::Rgb8Pixel]) -> u64 {
    let mut value = 0xcbf29ce484222325u64;
    for pixel in pixels {
        for channel in [pixel.r, pixel.g, pixel.b] {
            value = (value ^ u64::from(channel)).wrapping_mul(0x100000001b3);
        }
    }
    value
}

pub(crate) struct Recording {
    handle: PerformanceTraceHandle,
    callback: Callback,
    callback_generation: u64,
    started: ClockSample,
    rgb_fingerprints_enabled: bool,
    pub(crate) sample: PerformanceFrameSample,
}

impl Recording {
    /// Call only after all framebuffer/lifecycle borrows have been released.
    pub(crate) fn finish(mut self) {
        self.sample.timing = self.started.finish();
        let current = {
            let state = self.handle.inner.state.lock().unwrap_or_else(|error| error.into_inner());
            state.callback.is_some() && state.callback_generation == self.callback_generation
        };
        if current {
            (self.callback)(self.sample);
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ClockSample {
    monotonic_ns: Option<u64>,
    thread_cpu_ns: Option<u64>,
    thread: ThreadId,
}

impl ClockSample {
    pub(crate) fn now() -> Self {
        Self {
            monotonic_ns: clock_ns(libc::CLOCK_MONOTONIC),
            thread_cpu_ns: clock_ns(libc::CLOCK_THREAD_CPUTIME_ID),
            thread: std::thread::current().id(),
        }
    }

    pub(crate) fn finish(self) -> PerformanceTiming {
        self.until(Self::now())
    }

    fn until(self, finished: Self) -> PerformanceTiming {
        PerformanceTiming {
            started_monotonic_ns: self.monotonic_ns,
            finished_monotonic_ns: finished.monotonic_ns,
            started_thread_cpu_ns: self.thread_cpu_ns,
            finished_thread_cpu_ns: finished.thread_cpu_ns,
            wall_ns: difference(self.monotonic_ns, finished.monotonic_ns),
            thread_cpu_ns: if self.thread == finished.thread {
                difference(self.thread_cpu_ns, finished.thread_cpu_ns)
            } else { None },
            same_thread: self.thread == finished.thread,
        }
    }
}

fn difference(start: Option<u64>, end: Option<u64>) -> Option<u64> {
    end?.checked_sub(start?)
}

fn clock_ns(clock: libc::clockid_t) -> Option<u64> {
    let mut stamp = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // Both clocks are read-only observations and never wait for display work.
    if unsafe { libc::clock_gettime(clock, &mut stamp) } != 0 {
        return None;
    }
    u64::try_from(stamp.tv_sec).ok()?.checked_mul(1_000_000_000)?
        .checked_add(u64::try_from(stamp.tv_nsec).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_sampling_does_not_read_clocks_or_capture_markers() {
        let trace = PerformanceTraceHandle::default();
        trace.set_reader_operation_id(Some(7));
        assert!(trace.begin_with_clock(PerformanceSampleKind::Frame,
            || panic!("a disabled sample must not read any clock")).is_none());
        assert_eq!(trace.inner.state.lock().unwrap().reader_operation_id, None);
        observe_rgb_frame_with(None, || panic!("disabled observations must not hash pixels"));
    }

    #[test]
    fn rgb_observation_covers_every_pixel_and_channel_in_order() {
        let pixels = [slint::Rgb8Pixel { r: 1, g: 2, b: 3 },
            slint::Rgb8Pixel { r: 4, g: 5, b: 6 }];
        let expected = rgb_fingerprint(&pixels);
        assert_eq!(expected, rgb_fingerprint(&pixels.clone()));
        for index in 0..pixels.len() {
            for channel in 0..3 {
                let mut changed = pixels;
                let pixel = &mut changed[index];
                match channel { 0 => pixel.r ^= 1, 1 => pixel.g ^= 1, _ => pixel.b ^= 1 }
                assert_ne!(expected, rgb_fingerprint(&changed));
            }
        }
        assert_ne!(expected, rgb_fingerprint(&[pixels[1], pixels[0]]));
    }

    #[test]
    fn enabled_rgb_observation_records_identity_without_claiming_panel_completion() {
        let trace = PerformanceTraceHandle::default();
        let samples = Arc::new(Mutex::new(Vec::new()));
        let output = samples.clone();
        trace.set_callback(Arc::new(move |sample| output.lock().unwrap().push(sample)));
        trace.set_rgb_fingerprints_enabled(true);
        let mut recording = trace.begin(PerformanceSampleKind::Frame).unwrap();
        let pixels = [slint::Rgb8Pixel { r: 7, g: 8, b: 9 }];
        observe_rgb_frame(Some(&mut recording), &pixels);
        recording.finish();
        let samples = samples.lock().unwrap();
        assert_eq!(samples[0].rgb_fingerprint, Some(rgb_fingerprint(&pixels)));
        assert!(samples[0].stages.rgb_fingerprint.is_some());
        assert!(!samples[0].driver_completion_observed);
        assert!(!samples[0].panel_completion_observed);
    }

    #[test]
    fn rgb_fingerprints_require_opt_in_and_callback_changes_revoke_it() {
        let trace = PerformanceTraceHandle::default();
        trace.set_rgb_fingerprints_enabled(true);
        assert!(!trace.inner.state.lock().unwrap().rgb_fingerprints_enabled);
        trace.set_callback(Arc::new(|_| {}));
        let mut disabled = trace.begin(PerformanceSampleKind::Frame).unwrap();
        observe_rgb_frame_with(Some(&mut disabled), || panic!("ordinary traces must not hash content"));
        assert_eq!(disabled.sample.rgb_fingerprint, None);

        trace.set_rgb_fingerprints_enabled(true);
        let mut enabled = trace.begin(PerformanceSampleKind::Frame).unwrap();
        observe_rgb_frame_with(Some(&mut enabled), || 17);
        assert_eq!(enabled.sample.rgb_fingerprint, Some(17));
        let mut retired = trace.begin(PerformanceSampleKind::Frame).unwrap();
        trace.set_callback(Arc::new(|_| {}));
        assert!(!trace.inner.state.lock().unwrap().rgb_fingerprints_enabled);
        trace.set_rgb_fingerprints_enabled(true);
        observe_rgb_frame_with(Some(&mut retired), || panic!("a new callback cannot authorize an old sample"));

        let mut revoked = trace.begin(PerformanceSampleKind::Frame).unwrap();
        trace.set_rgb_fingerprints_enabled(false);
        observe_rgb_frame_with(Some(&mut revoked), || panic!("revoked authorization must stop hashing"));
        trace.set_rgb_fingerprints_enabled(true);
        trace.clear_callback();
        assert!(!trace.inner.state.lock().unwrap().rgb_fingerprints_enabled);
    }

    #[test]
    fn repeated_draws_keep_the_image_marker_and_get_distinct_sequences() {
        let trace = PerformanceTraceHandle::default();
        let samples = Arc::new(Mutex::new(Vec::new()));
        let output = samples.clone();
        trace.set_callback(Arc::new(move |sample| output.lock().unwrap().push(sample)));
        trace.set_reader_operation_id(Some(41));
        let first = trace.begin(PerformanceSampleKind::Frame).unwrap();
        trace.set_reader_operation_id(Some(42));
        first.finish();
        trace.begin(PerformanceSampleKind::Frame).unwrap().finish();
        trace.begin(PerformanceSampleKind::Frame).unwrap().finish();
        trace.set_reader_operation_id(None);
        trace.begin(PerformanceSampleKind::Frame).unwrap().finish();
        let samples = samples.lock().unwrap();
        assert_eq!(samples.iter().map(|sample| sample.reader_operation_id).collect::<Vec<_>>(),
            vec![Some(41), Some(42), Some(42), None]);
        assert_eq!(samples.iter().map(|sample| sample.render_sequence).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        assert!(samples.iter().all(|sample| !sample.panel_completion_observed));
    }

    #[test]
    fn callback_retirement_discards_a_sample_before_dispatch() {
        let trace = PerformanceTraceHandle::default();
        trace.set_callback(Arc::new(|_| panic!("a retired callback must not run")));
        let pending = trace.begin(PerformanceSampleKind::Frame).unwrap();
        trace.clear_callback();
        pending.finish();
        assert!(trace.begin_with_clock(PerformanceSampleKind::Frame,
            || panic!("a disabled sample must not read any clock")).is_none());
    }

    #[test]
    fn callback_is_invoked_without_the_trace_state_lock() {
        let trace = PerformanceTraceHandle::default();
        let callback_handle = trace.clone();
        trace.set_callback(Arc::new(move |_| callback_handle.clear_callback()));
        trace.begin(PerformanceSampleKind::InitialSetup).unwrap().finish();
        assert!(!trace.inner.enabled.load(Ordering::Acquire));
    }

    #[test]
    fn missing_or_regressing_clocks_are_not_reported_as_zero_duration() {
        let thread = std::thread::current().id();
        let start = ClockSample { monotonic_ns: Some(20), thread_cpu_ns: None, thread };
        let end = ClockSample { monotonic_ns: Some(10), thread_cpu_ns: Some(7), thread };
        assert_eq!(start.until(end).wall_ns, None);
        assert_eq!(start.until(end).thread_cpu_ns, None);
        let later = ClockSample { monotonic_ns: Some(30), thread_cpu_ns: Some(8), thread };
        assert_eq!(end.until(later).wall_ns, Some(20));
        assert_eq!(end.until(later).thread_cpu_ns, Some(1));
    }

    #[test]
    fn cpu_time_from_different_threads_is_not_subtracted() {
        let start = ClockSample { monotonic_ns: Some(10), thread_cpu_ns: Some(4),
            thread: std::thread::current().id() };
        let end = std::thread::spawn(|| ClockSample { monotonic_ns: Some(30), thread_cpu_ns: Some(8),
            thread: std::thread::current().id() }).join().unwrap();
        let timing = start.until(end);
        assert_eq!(timing.wall_ns, Some(20));
        assert_eq!(timing.thread_cpu_ns, None);
    }
}

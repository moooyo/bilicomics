use super::{
    DeviceControl, DeviceProfile, PlatformCapabilities, PlatformError,
    validate_native_launch_approval,
};
use std::fs::OpenOptions;
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Serialize only numeric display diagnostics after the backend releases its
/// framebuffer/lifecycle borrows. No content or input-coordinate data enters
/// this bridge, and a submitted ioctl is not a panel-completion observation.
pub(super) fn configure_performance_trace(trace: &slint_backend_kindle::PerformanceTraceHandle) {
    use slint_backend_kindle::PerformanceSampleKind;
    trace.set_callback(std::sync::Arc::new(|sample| {
        if !crate::performance::enabled() { return; }
        let payload = native_performance_payload(&sample);
        match sample.kind {
            PerformanceSampleKind::InitialSetup => {
                crate::performance::startup("native_initial_setup", &payload);
            },
            PerformanceSampleKind::Frame => {
                crate::performance::record_for_id(
                    sample.reader_operation_id.unwrap_or(0), "native", "frame", &payload);
            },
        }
    }));
}

fn native_performance_payload(sample: &slint_backend_kindle::PerformanceFrameSample) -> serde_json::Value {
    use slint_backend_kindle::RefreshWaveform;
    let stages = &sample.stages;
    let waveform = sample.refresh_waveform.map(|waveform| match waveform {
        RefreshWaveform::Auto => "auto",
        RefreshWaveform::Gc16 => "gc16",
        RefreshWaveform::Du => "du",
        RefreshWaveform::A2 => "a2",
    });
    serde_json::json!({
        "render_sequence": sample.render_sequence,
        "reader_operation_id": sample.reader_operation_id,
        "timing": native_timing_payload(sample.timing),
        "stages": {
            "active_io": stages.active_io.map(native_timing_payload),
            "buffer_setup": stages.buffer_setup.map(native_timing_payload),
            "window_setup": stages.window_setup.map(native_timing_payload),
            "portrait_observer_setup": stages.portrait_observer_setup.map(native_timing_payload),
            "initial_fill": stages.initial_fill.map(native_timing_payload),
            "preceding_update_wait": stages.preceding_update_wait.map(native_timing_payload),
            "slint_render": stages.slint_render.map(native_timing_payload),
            "rgb_fingerprint": stages.rgb_fingerprint.map(native_timing_payload),
            "framebuffer_write": stages.framebuffer_write.map(native_timing_payload),
            "refresh_submit": stages.refresh_submit.map(native_timing_payload),
            "refresh_complete_wait": stages.refresh_complete_wait.map(native_timing_payload),
            "lifecycle_complete": stages.lifecycle_complete.map(native_timing_payload),
        },
        "framebuffer_size": sample.framebuffer_size,
        "framebuffer_stride": sample.framebuffer_stride,
        "framebuffer_bits_per_pixel": sample.framebuffer_bits_per_pixel,
        "rotation_degrees": sample.rotation_degrees,
        "output_inverted": sample.output_inverted,
        "black_and_white": sample.black_and_white,
        "hardware_y4_requested": sample.hardware_y4_requested,
        "hardware_y4_available": sample.hardware_y4_available,
        "hardware_y4_applied": sample.hardware_y4_applied,
        "software_dither_fallback": sample.software_dither_fallback,
        "rendered": sample.rendered,
        "initial_frame": sample.initial_frame,
        "forced_full": sample.forced_full,
        "complete_frame_replacement": sample.complete_frame_replacement,
        "preceding_update_marker": sample.preceding_update_marker,
        "accepted_update_marker": sample.accepted_update_marker,
        "completed_update_marker": sample.completed_update_marker,
        "rgb_fingerprint": sample.rgb_fingerprint.map(|value| format!("{value:016x}")),
        "rgb_fingerprint_algorithm": sample.rgb_fingerprint.map(|_| "fnv1a64-rgb8-row-major"),
        "dirty_region": sample.dirty_region.map(native_rect_payload),
        "refresh_region": sample.refresh_region.map(native_rect_payload),
        "refresh_waveform": waveform,
        "refresh_abi": sample.refresh_abi,
        "refresh_flashing": sample.refresh_flashing,
        "refresh_ioctl_returned": sample.refresh_ioctl_attempts > 0,
        "refresh_ioctl_attempts": sample.refresh_ioctl_attempts,
        "refresh_accepted": sample.refresh_accepted,
        "refresh_errno": sample.refresh_errno,
        "completion_errno": sample.completion_errno,
        "driver_completion_observed": sample.driver_completion_observed,
        "panel_completion_observed": sample.panel_completion_observed,
    })
}

fn native_timing_payload(timing: slint_backend_kindle::PerformanceTiming) -> serde_json::Value {
    serde_json::json!({
        "started_monotonic_ns": timing.started_monotonic_ns,
        "finished_monotonic_ns": timing.finished_monotonic_ns,
        "started_thread_cpu_ns": timing.started_thread_cpu_ns,
        "finished_thread_cpu_ns": timing.finished_thread_cpu_ns,
        "wall_ns": timing.wall_ns,
        "thread_cpu_ns": timing.thread_cpu_ns,
        "same_thread": timing.same_thread,
    })
}

fn native_rect_payload(rect: slint_backend_kindle::PhysicalRect) -> serde_json::Value {
    serde_json::json!({ "x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height })
}

/// Linux's stable fb_var_screeninfo layout consists of forty 32-bit words.
/// Visible resolution and panning offsets are checked without writing display state.
#[repr(C)]
struct FramebufferInfo {
    words: [u32; 40],
}

pub(super) struct NativeControl {
    get_property: Option<PathBuf>,
    set_property: Option<PathBuf>,
    native_viewport: (u32, u32),
}

impl NativeControl {
    pub fn new(profile: DeviceProfile) -> Result<Self, PlatformError> {
        validate_native_launch_approval(profile)?;
        let expected = profile.physical_size();
        let native_viewport = validate_framebuffer(expected)?;
        eprintln!(
            "bilicomics: native display geometry: expected={}x{}, actual={}x{}, offset=0,0, scale=2",
            expected.0, expected.1, native_viewport.0, native_viewport.1,
        );
        Ok(Self {
            get_property: find_command("lipc-get-prop"),
            set_property: find_command("lipc-set-prop"),
            native_viewport,
        })
    }

    fn get_integer(&self, property: &str) -> Result<u8, PlatformError> {
        let program = self
            .get_property
            .as_deref()
            .ok_or(PlatformError::Unsupported("lipc"))?;
        let output = run_command(program, &["com.lab126.powerd", property])?;
        output
            .trim()
            .parse::<u8>()
            .map_err(|_| PlatformError::Operation("powerd returned a nonnumeric value".into()))
    }

    fn set_integer(&self, property: &str, value: u8) -> Result<(), PlatformError> {
        let program = self
            .set_property
            .as_deref()
            .ok_or(PlatformError::Unsupported("lipc"))?;
        run_command(
            program,
            &["-i", "com.lab126.powerd", property, &value.to_string()],
        )?;
        Ok(())
    }
}

impl DeviceControl for NativeControl {
    fn native_viewport(&self) -> Option<(u32, u32)> {
        Some(self.native_viewport)
    }

    fn capabilities(&self) -> PlatformCapabilities {
        let writable = self.set_property.is_some();
        PlatformCapabilities {
            brightness: writable && self.get_property.is_some(),
            warmth: writable && self.get_property.is_some(),
            keep_awake: writable,
            // These require additional reviewed backend interfaces.
            hardware_inversion: false,
            pen: false,
            explicit_refresh: false,
        }
    }

    fn brightness(&self) -> Result<u8, PlatformError> {
        self.get_integer("flIntensity")
    }

    fn set_brightness(&self, value: u8) -> Result<(), PlatformError> {
        if value > 24 {
            return Err(PlatformError::InvalidValue(
                "brightness must be between 0 and 24",
            ));
        }
        self.set_integer("flIntensity", value)
    }

    fn warmth(&self) -> Result<u8, PlatformError> {
        self.get_integer("currentAmberLevel")
    }

    fn set_warmth(&self, value: u8) -> Result<(), PlatformError> {
        if value > 24 {
            return Err(PlatformError::InvalidValue(
                "warmth must be between 0 and 24",
            ));
        }
        self.set_integer("currentAmberLevel", value)
    }

    fn battery_percentage(&self) -> Result<u8, PlatformError> {
        let value = self.get_integer("battLevel")?;
        if value > 100 {
            return Err(PlatformError::InvalidValue(
                "battery percentage must be in 0..=100",
            ));
        }
        Ok(value)
    }

    fn set_inverted(&self, _: bool) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("hardware_inversion"))
    }

    fn reset_idle_timer(&self) -> Result<(), PlatformError> {
        self.set_integer("touchScreenSaverTimeout", 1)
    }
}

fn validate_framebuffer(expected: (u32, u32)) -> Result<(u32, u32), PlatformError> {
    let file = OpenOptions::new().read(true).open("/dev/fb0")?;
    let mut information = FramebufferInfo { words: [0; 40] };
    // SAFETY: FBIOGET_VSCREENINFO copies the Linux fb_var_screeninfo structure
    // into an aligned 160-byte writable buffer. It does not retain the pointer.
    let result = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            0x4600,
            &mut information as *mut FramebufferInfo,
        )
    };
    if result == -1 {
        return Err(std::io::Error::last_os_error().into());
    }
    validate_framebuffer_info(&information, expected)
}

fn validate_framebuffer_info(
    information: &FramebufferInfo,
    expected: (u32, u32),
) -> Result<(u32, u32), PlatformError> {
    let actual = (information.words[0], information.words[1]);
    if actual != expected {
        return Err(PlatformError::GeometryMismatch { expected, actual });
    }
    let offset = (information.words[4], information.words[5]);
    if offset != (0, 0) {
        return Err(PlatformError::FramebufferOffset { actual: offset });
    }
    Ok(actual)
}

pub(super) fn find_command(name: &str) -> Option<PathBuf> {
    ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
        .into_iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| path.is_file())
}

const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const COMMAND_OUTPUT_LIMIT: usize = 4096;

fn run_command(program: &Path, arguments: &[&str]) -> Result<String, PlatformError> {
    slint_backend_kindle::run_bounded_command(
        program,
        arguments,
        COMMAND_TIMEOUT,
        COMMAND_OUTPUT_LIMIT,
    )
    .map_err(|error| match error.kind() {
        io::ErrorKind::TimedOut => PlatformError::Operation("LIPC operation timed out".into()),
        io::ErrorKind::WouldBlock => {
            PlatformError::Operation("LIPC command capacity is exhausted".into())
        }
        io::ErrorKind::InvalidData | io::ErrorKind::Other => {
            PlatformError::Operation(format!("LIPC operation failed: {error}"))
        }
        _ => error.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framebuffer_info(size: (u32, u32), offset: (u32, u32)) -> FramebufferInfo {
        let mut information = FramebufferInfo { words: [0; 40] };
        information.words[0] = size.0;
        information.words[1] = size.1;
        information.words[2] = size.0;
        information.words[3] = size.1 * 2;
        information.words[4] = offset.0;
        information.words[5] = offset.1;
        information
    }

    #[test]
    fn each_profile_requires_its_exact_framebuffer_geometry() {
        for profile in [
            DeviceProfile::Paperwhite5,
            DeviceProfile::Paperwhite6,
            DeviceProfile::Scribe,
            DeviceProfile::Scribe2025,
        ] {
            assert_eq!(
                validate_framebuffer_info(
                    &framebuffer_info(profile.physical_size(), (0, 0)),
                    profile.physical_size(),
                )
                .unwrap(),
                profile.physical_size(),
            );
        }
    }

    #[test]
    fn visible_geometry_is_checked_independently_of_virtual_framebuffer_height() {
        let information = framebuffer_info((1272, 1696), (0, 0));
        assert_eq!(
            validate_framebuffer_info(&information, (1272, 1696)).unwrap(),
            (1272, 1696),
        );
        let error = validate_framebuffer_info(&information, (1264, 1680)).unwrap_err();
        assert!(matches!(
            error,
            PlatformError::GeometryMismatch {
                expected: (1264, 1680),
                actual: (1272, 1696),
            },
        ));
    }

    #[test]
    fn nonzero_framebuffer_offsets_are_refused() {
        for offset in [(1, 0), (0, 1696)] {
            let information = framebuffer_info((1272, 1696), offset);
            assert!(matches!(
                validate_framebuffer_info(&information, (1272, 1696)),
                Err(PlatformError::FramebufferOffset { actual }) if actual == offset,
            ));
        }
    }
}

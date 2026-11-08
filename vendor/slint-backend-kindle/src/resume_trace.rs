//! Optional diagnostics for a rendered resumed scene and its refresh request.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub(crate) fn append_resume_frame(path: &Path, output_inverted: bool) -> io::Result<()> {
    let record = frame_record(std::process::id(), read_uptime_seconds(), output_inverted);
    // Nonblocking and no-follow flags prevent a special-file or symlink trace
    // destination from turning this optional diagnostic into a UI wait.
    let mut file = OpenOptions::new().append(true).create(true).mode(0o600)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "resume trace must be a regular file"));
    }
    file.write_all(record.as_bytes())
}

fn read_uptime_seconds() -> Option<f64> {
    let mut text = String::new();
    File::open("/proc/uptime").ok()?.take(128).read_to_string(&mut text).ok()?;
    parse_uptime_seconds(&text)
}

fn parse_uptime_seconds(text: &str) -> Option<f64> {
    text.split_whitespace().next()?.parse::<f64>().ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn frame_record(process_id: u32, uptime: Option<f64>, output_inverted: bool) -> String {
    let uptime = uptime.filter(|value| value.is_finite() && *value >= 0.0)
        .map_or_else(|| "null".to_owned(), |value| value.to_string());
    format!(
        "{{\"event\":\"native-resume-frame-rendered\",\"process_id\":{process_id},\"uptime_seconds\":{uptime},\"output_inverted\":{output_inverted},\"framebuffer_pixels_written\":true,\"refresh_requested\":true,\"physical_panel_update_confirmed\":false}}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_parser_accepts_only_a_finite_nonnegative_first_field() {
        assert_eq!(parse_uptime_seconds("123.45 67.89\n"), Some(123.45));
        for value in ["", "oops 1", "NaN 1", "inf 1", "-1 0"] {
            assert_eq!(parse_uptime_seconds(value), None);
        }
    }

    #[test]
    fn rendered_frame_record_reports_request_timing_without_panel_completion() {
        assert_eq!(frame_record(42, Some(123.45), true),
            "{\"event\":\"native-resume-frame-rendered\",\"process_id\":42,\"uptime_seconds\":123.45,\"output_inverted\":true,\"framebuffer_pixels_written\":true,\"refresh_requested\":true,\"physical_panel_update_confirmed\":false}\n");
        for uptime in [None, Some(f64::NAN), Some(f64::INFINITY), Some(-1.0)] {
            let record = frame_record(7, uptime, false);
            assert!(record.contains("\"uptime_seconds\":null"));
            assert!(record.contains("\"output_inverted\":false"));
            assert_eq!(record.lines().count(), 1);
        }
    }
}

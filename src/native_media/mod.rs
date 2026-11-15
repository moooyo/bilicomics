// SPDX-License-Identifier: GPL-3.0-or-later
// Adapted from AidokuForKindle's 2026-11-15 media candidate.
// Source identity, original snapshots, and changes: vendor/aidoku-native-media/README.md.

//! Optional, qualified JPEG processing in a separate process.
//!
//! A caller timeout never kills a process that may still own DMA buffers.
//! Unsupported images retain the ordinary software Reader path.

mod jpeg;
#[cfg(all(feature = "kindle", target_os = "linux"))]
mod worker;

use std::{
    path::{Path, PathBuf},
    sync::{
        Once,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    time::Duration,
};

use image::GrayImage;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Acceleration {
    #[default]
    Off,
    /// Automatic production path: only use a combined decode/resize transaction.
    DecodeAndResize,
}

impl Acceleration {
    pub fn is_off(&self) -> bool {
        *self == Self::Off
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Availability {
    Available = 1,
    UnsupportedPlatform = 2,
    UnverifiedDevice = 3,
    // Wire value 4 is reserved for the retired software-version gate.
    MissingHelper = 5,
    MissingDriver = 6,
    MissingDiagnostics = 7,
    Busy = 8,
    Failed = 9,
}

impl Availability {
    pub fn available(self) -> bool {
        self == Self::Available
    }
    pub fn reason(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::UnsupportedPlatform => "platform",
            Self::UnverifiedDevice => "device",
            Self::MissingHelper => "helper",
            Self::MissingDriver => "driver",
            Self::MissingDiagnostics => "diagnostics",
            Self::Busy => "busy",
            Self::Failed => "failed",
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Available,
            2 => Self::UnsupportedPlatform,
            3 => Self::UnverifiedDevice,
            5 => Self::MissingHelper,
            6 => Self::MissingDriver,
            7 => Self::MissingDiagnostics,
            8 => Self::Busy,
            _ => Self::Failed,
        }
    }
}

static INITIALIZE: Once = Once::new();
static STATUS: AtomicU8 = AtomicU8::new(Availability::UnsupportedPlatform as u8);
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static COMPLETED: AtomicU64 = AtomicU64::new(0);
static RESIZED: AtomicU64 = AtomicU64::new(0);
static SOFTWARE_FALLBACKS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(test, target_os = "linux"))]
static TRANSPORT_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Statistics {
    pub attempts: u64,
    pub completed: u64,
    pub resized: u64,
    pub software_fallbacks: u64,
}

pub fn statistics() -> Statistics {
    Statistics {
        attempts: ATTEMPTS.load(Ordering::Relaxed),
        completed: COMPLETED.load(Ordering::Relaxed),
        resized: RESIZED.load(Ordering::Relaxed),
        software_fallbacks: SOFTWARE_FALLBACKS.load(Ordering::Relaxed),
    }
}

pub fn availability() -> Availability {
    Availability::from_code(STATUS.load(Ordering::Acquire))
}

fn helper_path() -> Option<PathBuf> {
    Some(
        std::env::current_exe()
            .ok()?
            .parent()?
            .join("kindle-jpeg-worker"),
    )
}

/// A bounded metadata-only query. The worker opens no buffers or streams.
pub fn initialize(native: bool) -> Availability {
    if !native {
        return Availability::UnsupportedPlatform;
    }
    INITIALIZE.call_once(|| {
        let qualification = qualified_device();
        let status = if !qualification.available() {
            qualification
        } else if let Some(path) = helper_path().filter(|p| p.is_file()) {
            match exchange(&path, 0, &[], 0, 0, Duration::from_secs(2)) {
                Ok(response) if response.status == 0 => Availability::Available,
                Ok(response) => Availability::from_code(response.status as u8),
                Err(ExchangeError::Busy) => Availability::Busy,
                Err(ExchangeError::Failed(_)) => Availability::Failed,
            }
        } else {
            Availability::MissingHelper
        };
        STATUS.store(status as u8, Ordering::Release);
    });
    availability()
}

fn qualified_device() -> Availability {
    #[cfg(all(feature = "kindle", target_os = "linux"))]
    {
        worker::qualified_device()
    }
    #[cfg(not(all(feature = "kindle", target_os = "linux")))]
    {
        Availability::UnsupportedPlatform
    }
}

/// Admit the complete baseline encoding using entropy validation without IDCT.
/// This narrower policy does not alter the software Reader's image limits.
pub fn information(bytes: &[u8]) -> Option<(u32, u32)> {
    let parsed = jpeg::parse(bytes).ok()?;
    if parsed.information.sampling != jpeg::Sampling::Gray {
        return None;
    }
    parsed.validate_entropy(bytes).then_some((
        parsed.information.width,
        parsed.information.height,
    ))
}

/// Admit only the complete source/target pairs observed without driver faults.
/// A shared ratio or one matching dimension does not qualify another pair.
pub(crate) fn qualified_geometry(source: (u32, u32), target: (u32, u32)) -> bool {
    matches!(
        (source.0, source.1, target.0, target.1),
        (1920, 2560, 1860, 2480) | (1920, 1280, 1860, 1240)
    )
}

fn eligible_target(info: jpeg::Information, target: (u32, u32)) -> bool {
    info.sampling == jpeg::Sampling::Gray
        && qualified_geometry((info.width, info.height), target)
}

/// Return owned Gray8 pixels only after the worker has completed cleanup.
/// Cancellation belongs to the Reader generation and never kills the worker.
pub fn decode_and_resize(bytes: &[u8], target: (u32, u32)) -> Option<GrayImage> {
    let parsed = match jpeg::parse(bytes) {
        Ok(value) if value.validate_entropy(bytes) => value,
        _ => {
            SOFTWARE_FALLBACKS.fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };
    let info = parsed.information;
    let (width, height) = target;
    // Both processes enforce the same downscale-only, non-small-image policy.
    if !eligible_target(info, target) {
        return None;
    }
    if !availability().available() {
        return None;
    }
    let path = helper_path()?;
    let response = exchange(&path, 1, bytes, width, height, Duration::from_secs(3));
    match response {
        Ok(response)
            if response.status == 0 && response.width == width && response.height == height =>
        {
            let image = GrayImage::from_raw(width, height, response.pixels)?;
            COMPLETED.fetch_add(1, Ordering::Relaxed);
            if width != info.width || height != info.height {
                RESIZED.fetch_add(1, Ordering::Relaxed);
            }
            Some(image)
        }
        Ok(response) if response.status == 10 => {
            SOFTWARE_FALLBACKS.fetch_add(1, Ordering::Relaxed);
            None
        }
        Err(ExchangeError::Busy) => {
            // Prefetch may overlap the active page. It must not submit another
            // transaction or turn a healthy in-process owner into a failure.
            SOFTWARE_FALLBACKS.fetch_add(1, Ordering::Relaxed);
            None
        }
        error => {
            let status = match &error {
                Ok(response) if response.status == Availability::Busy as u32 => Availability::Busy,
                _ => Availability::Failed,
            };
            STATUS.store(status as u8, Ordering::Release);
            SOFTWARE_FALLBACKS.fetch_add(1, Ordering::Relaxed);
            match error {
                Ok(response) => eprintln!(
                    "bilicomics: native JPEG disabled for this run: {} (errno {})",
                    response.stage, response.errno
                ),
                Err(ExchangeError::Failed(error)) => {
                    eprintln!("bilicomics: native JPEG disabled for this run: {error}");
                }
                Err(ExchangeError::Busy) => unreachable!("handled above"),
            }
            None
        }
    }
}

const REQUEST_MAGIC: &[u8; 8] = b"KJPR0001";
const RESPONSE_MAGIC: &[u8; 8] = b"KJPG0001";

struct Response {
    status: u32,
    errno: i32,
    width: u32,
    height: u32,
    stage: String,
    pixels: Vec<u8>,
}

#[derive(Debug)]
enum ExchangeError {
    Busy,
    Failed(String),
}

fn exchange(
    path: &Path,
    operation: u32,
    bytes: &[u8],
    width: u32,
    height: u32,
    timeout: Duration,
) -> Result<Response, ExchangeError> {
    if IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(ExchangeError::Busy);
    }
    let path = path.to_path_buf();
    let bytes = bytes.to_vec();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("jpeg-worker-io".into())
        .spawn(move || {
            struct Release;
            impl Drop for Release {
                fn drop(&mut self) {
                    IN_FLIGHT.store(false, Ordering::Release);
                }
            }
            let _release = Release;
            let result = exchange_blocking(&path, operation, &bytes, width, height);
            let _ = sender.send(result);
        });
    if let Err(error) = spawned {
        IN_FLIGHT.store(false, Ordering::Release);
        return Err(ExchangeError::Failed(error.to_string()));
    }
    // Dropping this receiver leaves the I/O owner and child alive. The child
    // also owns an interprocess lock until its driver transaction has drained.
    receiver
        .recv_timeout(timeout)
        .map_err(|_| {
            ExchangeError::Failed(
                "Native worker deadline exceeded; retaining its DMA owner".into(),
            )
        })?
        .map_err(ExchangeError::Failed)
}

fn exchange_blocking(
    path: &Path,
    operation: u32,
    bytes: &[u8],
    width: u32,
    height: u32,
) -> Result<Response, String> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let mut command = Command::new(path);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
                    libc::signal(signal, libc::SIG_IGN);
                }
                Ok(())
            });
        }
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    if operation == 1 {
        ATTEMPTS.fetch_add(1, Ordering::Relaxed);
    }
    let mut request = [0_u8; 32];
    request[..8].copy_from_slice(REQUEST_MAGIC);
    for (offset, value) in [
        (8, operation),
        (16, width),
        (20, height),
        (24, bytes.len() as u32),
    ] {
        request[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    let result = (|| -> Result<Response, String> {
        let mut input = child.stdin.take().ok_or("Missing worker input")?;
        input
            .write_all(&request)
            .and_then(|_| input.write_all(bytes))
            .map_err(|e| e.to_string())?;
        drop(input);
        let mut output = child.stdout.take().ok_or("Missing worker output")?;
        let mut header = [0_u8; 96];
        output.read_exact(&mut header).map_err(|e| e.to_string())?;
        if &header[..8] != RESPONSE_MAGIC {
            return Err("Invalid worker response identity".into());
        }
        let word =
            |offset: usize| u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap());
        let (status, w, h, length) = (word(8), word(16), word(20), word(24) as usize);
        if status > 10
            || word(28) != 0
            || (status == 0 && word(12) != 0)
            || length > jpeg::MAX_PIXELS
            || (status == 0
                && operation == 1
                && (w != width || h != height || length != width as usize * height as usize))
            || (status != 0 && length != 0)
            || (operation == 0 && (length != 0 || w != 0 || h != 0))
        {
            return Err("Invalid worker image bounds".into());
        }
        let mut pixels = vec![0; length];
        output.read_exact(&mut pixels).map_err(|e| e.to_string())?;
        let mut extra = [0; 1];
        if output.read(&mut extra).map_err(|e| e.to_string())? != 0 {
            return Err("Trailing worker output".into());
        }
        let stage = String::from_utf8_lossy(&header[32..])
            .trim_end_matches('\0')
            .to_string();
        Ok(Response {
            status,
            errno: word(12) as i32,
            width: w,
            height: h,
            stage,
            pixels,
        })
    })();
    // Reap on protocol and pipe failures too. Waiting remains in the detached
    // I/O owner; it never turns a caller timeout into a kill or an early free.
    let exit = child.wait().map_err(|e| e.to_string())?;
    if !exit.success() {
        return Err("Native worker exited abnormally".into());
    }
    result
}

#[cfg(test)]
mod admission_tests {
    use super::*;

    #[test]
    fn retired_firmware_wire_value_is_reserved_and_acceleration_defaults_off() {
        assert_eq!(Availability::from_code(4), Availability::Failed);
        assert_eq!(Availability::MissingHelper as u8, 5);
        assert_eq!(Availability::MissingDriver as u8, 6);
        assert_eq!(Availability::MissingDiagnostics as u8, 7);
        assert_eq!(Availability::Busy as u8, 8);
        assert_eq!(Availability::Failed as u8, 9);
        assert_eq!(Acceleration::default(), Acceleration::Off);
    }

    #[test]
    fn production_geometry_only_admits_the_two_complete_measured_pairs() {
        let gray = jpeg::Information {
            width: 1920,
            height: 2560,
            sampling: jpeg::Sampling::Gray,
        };
        assert!(eligible_target(gray, (1860, 2480)));
        let wide = jpeg::Information { width: 1920, height: 1280, ..gray };
        assert!(eligible_target(wide, (1860, 1240)));
        for target in [(930, 1240), (1800, 2400), (1920, 1240), (1920, 2560),
                       (1921, 1240), (930, 2561), (15, 1240), (930, 0)] {
            assert!(!eligible_target(gray, target));
        }
        let tall = jpeg::Information { width: 1280, height: 2560, ..gray };
        assert!(!eligible_target(tall, (1240, 2480)));
        let unknown = jpeg::Information { width: 1600, height: 2400, ..gray };
        assert!(!eligible_target(unknown, (1550, 2325)));
        for sampling in [jpeg::Sampling::Yuv422, jpeg::Sampling::Yuv444] {
            let color = jpeg::Information { sampling, ..gray };
            assert!(!eligible_target(color, (1860, 2480)));
        }
        let small = jpeg::Information { width: 128, height: 128, ..gray };
        assert!(!eligible_target(small, (64, 64)));
    }

    #[test]
    fn information_requires_complete_entropy_without_decoding_pixels() {
        let image = image::GrayImage::from_fn(256, 256, |x, y| {
            image::Luma([(x.wrapping_mul(7) + y.wrapping_mul(11)) as u8])
        });
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
            .encode_image(&image)
            .unwrap();
        assert_eq!(information(&bytes), Some((256, 256)));
        // Keep the accepted container and tables while removing the coded image.
        let scan = bytes.windows(2).position(|pair| pair == [255, 218]).unwrap();
        let scan_length = usize::from(u16::from_be_bytes([bytes[scan + 2], bytes[scan + 3]]));
        let entropy_start = scan + 2 + scan_length;
        let entropy_end = bytes.len() - 2;
        bytes[entropy_start..entropy_end].fill(0);
        assert!(jpeg::parse(&bytes).is_ok());
        assert_eq!(information(&bytes), None);
    }

    fn complete_baseline_fixture(width: u16, height: u16, horizontal: u8, color: bool) -> Vec<u8> {
        fn segment(bytes: &mut Vec<u8>, marker: u8, body: &[u8]) {
            bytes.extend_from_slice(&[255, marker]);
            bytes.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
            bytes.extend_from_slice(body);
        }
        let mut bytes = vec![255, 216];
        let mut quantization = vec![0];
        quantization.extend_from_slice(&[1; 64]);
        segment(&mut bytes, 0xdb, &quantization);
        let components = if color { 3 } else { 1 };
        let mut frame = vec![8];
        frame.extend_from_slice(&height.to_be_bytes());
        frame.extend_from_slice(&width.to_be_bytes());
        frame.push(components);
        for component in 0..components {
            frame.extend_from_slice(&[component + 1, if component == 0 { horizontal << 4 | 1 } else { 0x11 }, 0]);
        }
        segment(&mut bytes, 0xc0, &frame);
        let mut huffman = vec![0, 1];
        huffman.extend_from_slice(&[0; 15]);
        huffman.push(0);
        huffman.extend_from_slice(&[0x10, 1]);
        huffman.extend_from_slice(&[0; 15]);
        huffman.push(0);
        segment(&mut bytes, 0xc4, &huffman);
        let mut scan = vec![components];
        for component in 0..components { scan.extend_from_slice(&[component + 1, 0]); }
        scan.extend_from_slice(&[0, 63, 0]);
        segment(&mut bytes, 0xda, &scan);
        // Every zero-valued block encodes DC=0 and EOB. These are complete
        // grayscale or YCbCr images, independently accepted by the CPU decoder.
        let blocks = if color { usize::from(horizontal) + 2 } else { 1 };
        let entropy_bits = usize::from(width).div_ceil(8 * usize::from(horizontal))
            * usize::from(height).div_ceil(8) * blocks * 2;
        bytes.resize(bytes.len() + entropy_bits.div_ceil(8), 0);
        if entropy_bits % 8 != 0 {
            *bytes.last_mut().unwrap() = (1_u8 << (8 - entropy_bits % 8)) - 1;
        }
        bytes.extend_from_slice(&[255, 217]);
        bytes
    }

    #[test]
    fn complete_color_jpegs_use_software_without_a_worker_attempt() {
        #[cfg(target_os = "linux")]
        let _serial = TRANSPORT_TESTS.lock().unwrap_or_else(|error| error.into_inner());
        for horizontal in [1, 2] {
            let bytes = complete_baseline_fixture(256, 256, horizontal, true);
            let parsed = jpeg::parse(&bytes).unwrap();
            assert!(parsed.validate_entropy(&bytes));
            assert_ne!(parsed.information.sampling, jpeg::Sampling::Gray);
            assert!(crate::image_integrity::validate_image(&bytes, None).is_ok());
            assert_eq!(information(&bytes), None);
            let before = statistics();
            assert!(decode_and_resize(&bytes, (128, 128)).is_none());
            let after = statistics();
            assert_eq!(after.attempts, before.attempts);
            assert_eq!(after.completed, before.completed);
            assert_eq!(after.resized, before.resized);
        }
    }

    #[test]
    fn unqualified_complete_gray_pairs_keep_metadata_but_never_start_a_worker() {
        #[cfg(target_os = "linux")]
        let _serial = TRANSPORT_TESTS.lock().unwrap_or_else(|error| error.into_inner());
        for (source, target) in [
            ((1280, 2560), (1240, 2480)),
            ((1600, 2400), (1550, 2325)),
            ((1920, 2560), (1800, 2400)),
            ((1920, 2560), (930, 1240)),
        ] {
            let bytes = complete_baseline_fixture(source.0, source.1, 1, false);
            assert!(crate::image_integrity::validate_image(&bytes, None).is_ok());
            assert_eq!(information(&bytes), Some((u32::from(source.0), u32::from(source.1))));
            let before = statistics();
            assert!(decode_and_resize(&bytes, target).is_none());
            let after = statistics();
            assert_eq!(after.attempts, before.attempts);
            assert_eq!(after.completed, before.completed);
            assert_eq!(after.resized, before.resized);
        }
    }
}

/// Entry point for the separate package helper, not the application process.
#[doc(hidden)]
pub fn worker_main() -> i32 {
    #[cfg(all(feature = "kindle", target_os = "linux"))]
    {
        worker::main()
    }
    #[cfg(not(all(feature = "kindle", target_os = "linux")))]
    {
        2
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    fn wait_for_io_owner() {
        let deadline = Instant::now() + Duration::from_secs(3);
        while IN_FLIGHT.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!IN_FLIGHT.load(Ordering::Acquire));
    }

    fn response_header(status: u32, width: u32, height: u32, length: u32) -> Vec<u8> {
        let mut header = vec![0; 96];
        header[..8].copy_from_slice(RESPONSE_MAGIC);
        for (offset, value) in [(8, status), (16, width), (20, height), (24, length)] {
            header[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        header
    }

    fn response_helper(path: &Path, response: &[u8], exit_code: i32) {
        let bytes = response.iter().map(u8::to_string).collect::<Vec<_>>().join(",");
        let script = format!(
            "#!/usr/bin/python3\nimport sys\nrequest=sys.stdin.buffer.read()\n\
             assert len(request)==32 and request[:8]==b'KJPR0001'\n\
             sys.stdout.buffer.write(bytes([{bytes}]))\n\
             sys.stdout.buffer.flush()\nsys.exit({exit_code})\n"
        );
        std::fs::write(path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn independent_lock_probe(path: &Path) -> bool {
        let script = r#"
import fcntl,os,sys
descriptor=os.open(sys.argv[1], os.O_RDWR | os.O_CLOEXEC | os.O_NOFOLLOW)
try:
    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError:
    sys.exit(3)
sys.exit(0)
"#;
        let status = Command::new("/usr/bin/python3")
            .args(["-c", script])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()
            .unwrap();
        match status.code() {
            Some(0) => true,
            Some(3) => false,
            other => panic!("independent lock probe failed: {other:?}"),
        }
    }

    #[test]
    fn worker_protocol_rejects_malformed_responses_and_abnormal_exit() {
        let _serial = TRANSPORT_TESTS.lock().unwrap_or_else(|error| error.into_inner());
        let directory = tempfile::tempdir().unwrap();
        let helper = directory.path().join("fake-jpeg-worker");
        let mut complete = response_header(0, 32, 32, 1024);
        complete.extend_from_slice(&[37; 1024]);
        let mut wrong_magic = complete.clone();
        wrong_magic[0] = b'X';
        let mut errno_on_success = complete.clone();
        errno_on_success[12..16].copy_from_slice(&5_u32.to_le_bytes());
        let mut reserved_word = complete.clone();
        reserved_word[28..32].copy_from_slice(&1_u32.to_le_bytes());
        let mut trailing = complete.clone();
        trailing.push(0);
        let mut short_pixels = response_header(0, 32, 32, 1024);
        short_pixels.extend_from_slice(&[37; 7]);
        let cases = [
            ("identity", wrong_magic, 0),
            ("header length", complete[..20].to_vec(), 0),
            ("pixel limit", response_header(0, 32, 32, jpeg::MAX_PIXELS as u32 + 1), 0),
            ("geometry", response_header(0, 31, 32, 1024), 0),
            ("error payload", response_header(9, 0, 0, 1), 0),
            ("pixel length", short_pixels, 0),
            ("trailing bytes", trailing, 0),
            ("status", response_header(11, 0, 0, 0), 0),
            ("success errno", errno_on_success, 0),
            ("reserved word", reserved_word, 0),
            ("process exit", complete.clone(), 7),
        ];
        for (name, response, exit_code) in cases {
            wait_for_io_owner();
            response_helper(&helper, &response, exit_code);
            assert!(
                matches!(
                    exchange(&helper, 1, &[], 32, 32, Duration::from_secs(2)),
                    Err(ExchangeError::Failed(_))
                ),
                "the transport accepted an invalid {name}"
            );
        }
        wait_for_io_owner();
        response_helper(&helper, &complete, 0);
        let response = exchange(&helper, 1, &[], 32, 32, Duration::from_secs(2)).unwrap();
        assert_eq!((response.status, response.width, response.height), (0, 32, 32));
        assert_eq!(response.pixels, vec![37; 1024]);
        wait_for_io_owner();
    }

    #[test]
    fn caller_deadline_preserves_and_reaps_the_independent_worker() {
        let _serial = TRANSPORT_TESTS.lock().unwrap_or_else(|error| error.into_inner());
        let directory = tempfile::tempdir().unwrap();
        let helper = directory.path().join("fake-jpeg-worker");
        std::fs::write(
            &helper,
            r#"#!/usr/bin/python3
import fcntl,os,signal,sys,time
from pathlib import Path
root=Path(__file__).parent
assert os.getsid(0)==os.getpid() and os.getpgrp()==os.getpid()
assert all(signal.getsignal(number)==signal.SIG_IGN for number in [signal.SIGTERM,signal.SIGINT,signal.SIGHUP])
assert len(sys.stdin.buffer.read(32))==32
descriptor=os.open(str(root/'hardware.lock'), os.O_RDWR | os.O_CREAT | os.O_CLOEXEC | os.O_NOFOLLOW, 0o600)
fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
(root/'started').write_text('ready')
deadline=time.monotonic()+10
while not (root/'release').exists():
    if time.monotonic()>deadline: sys.exit(2)
    time.sleep(.01)
sys.stdout.buffer.write(b'KJPG0001'+bytes(88))
sys.stdout.buffer.flush()
(root/'done').write_text('completed')
"#,
        )
        .unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(exchange(&helper, 0, &[], 0, 0, Duration::from_millis(10)).is_err());
        let deadline = Instant::now() + Duration::from_secs(3);
        while !directory.path().join("started").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(directory.path().join("started").exists());
        assert!(IN_FLIGHT.load(Ordering::Acquire));
        assert!(!directory.path().join("done").exists());
        assert!(matches!(
            exchange(&helper, 0, &[], 0, 0, Duration::from_millis(10)),
            Err(ExchangeError::Busy)
        ));
        // This peer has no access to the first parent's IN_FLIGHT flag. It
        // models another application or a restart while the owner drains.
        assert!(!independent_lock_probe(&directory.path().join("hardware.lock")));
        std::fs::write(directory.path().join("release"), b"finish").unwrap();
        wait_for_io_owner();
        assert!(directory.path().join("done").exists());
        assert!(independent_lock_probe(&directory.path().join("hardware.lock")));
    }
}

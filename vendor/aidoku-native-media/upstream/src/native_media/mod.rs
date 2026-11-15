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
    /// Explicit decoder-only comparison for research; never selected by user preferences.
    Decode,
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
    UnverifiedFirmware = 4,
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
            Self::UnverifiedFirmware => "firmware",
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
            4 => Self::UnverifiedFirmware,
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
                Err(_) => Availability::Failed,
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

pub fn information(bytes: &[u8]) -> Option<(u32, u32)> {
    let info = jpeg::parse(bytes).ok()?.information;
    Some((info.width, info.height))
}

/// Return owned Gray8 pixels only after the worker has completed cleanup.
/// Cancellation belongs to the Reader generation and never kills the worker.
pub fn decode(bytes: &[u8], target: Option<(u32, u32)>) -> Option<GrayImage> {
    if !availability().available() {
        return None;
    }
    let parsed = match jpeg::parse(bytes) {
        Ok(value) => value,
        Err(_) => {
            SOFTWARE_FALLBACKS.fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };
    let info = parsed.information;
    // Small images do not justify process and DMA setup, even when decodable.
    if u64::from(info.width) * u64::from(info.height) < 256 * 256 {
        return None;
    }
    let (width, height) = target.unwrap_or((info.width, info.height));
    if width < 16
        || height < 16
        || width > info.width
        || height > info.height
        || width > 2048
        || height > 4096
        || (info.sampling != jpeg::Sampling::Gray && width % 2 != 0)
    {
        return None;
    }
    let path = helper_path()?;
    ATTEMPTS.fetch_add(1, Ordering::Relaxed);
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
        error => {
            STATUS.store(Availability::Failed as u8, Ordering::Release);
            SOFTWARE_FALLBACKS.fetch_add(1, Ordering::Relaxed);
            match error {
                Ok(response) => eprintln!(
                    "aidoku: native JPEG disabled for this run: {} (errno {})",
                    response.stage, response.errno
                ),
                Err(error) => eprintln!("aidoku: native JPEG disabled for this run: {error}"),
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

fn exchange(
    path: &Path,
    operation: u32,
    bytes: &[u8],
    width: u32,
    height: u32,
    timeout: Duration,
) -> Result<Response, String> {
    if IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("A previous native worker is still draining".into());
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
        return Err(error.to_string());
    }
    // Dropping this receiver leaves the I/O owner and child alive. The child
    // also owns an interprocess lock until its driver transaction has drained.
    receiver
        .recv_timeout(timeout)
        .map_err(|_| "Native worker deadline exceeded; retaining its DMA owner".to_string())?
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
        if length > jpeg::MAX_PIXELS
            || (status == 0
                && operation == 1
                && (w != width || h != height || length != width as usize * height as usize))
            || (status != 0 && length != 0)
            || (operation == 0 && length != 0)
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
    use std::time::Instant;

    #[test]
    fn caller_deadline_preserves_and_reaps_the_independent_worker() {
        let directory = tempfile::tempdir().unwrap();
        let helper = directory.path().join("fake-jpeg-worker");
        std::fs::write(
            &helper,
            r#"#!/usr/bin/python3
import sys,time
from pathlib import Path
root=Path(__file__).parent
assert len(sys.stdin.buffer.read(32))==32
(root/'started').write_text('ready')
deadline=time.monotonic()+5
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
        assert!(exchange(&helper, 0, &[], 0, 0, Duration::from_millis(10)).is_err());
        std::fs::write(directory.path().join("release"), b"finish").unwrap();
        while IN_FLIGHT.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!IN_FLIGHT.load(Ordering::Acquire));
        assert!(directory.path().join("done").exists());
    }
}

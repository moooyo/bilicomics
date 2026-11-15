//! The only application-side owner of JDEC descriptors and USERPTR memory.

use super::{Availability, REQUEST_MAGIC, RESPONSE_MAGIC, jpeg};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawFd,
    },
};

pub(super) fn qualified_device() -> Availability {
    if !cfg!(target_arch = "arm") {
        return Availability::UnsupportedPlatform;
    }
    let mut identity = [0_u8; 16];
    if File::open("/proc/usid")
        .and_then(|mut file| file.read_exact(&mut identity))
        .is_err()
        || identity[0] != b'G'
        || !identity
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        || !matches!(
            &identity[3..6],
            b"27J" | b"2BL" | b"263" | b"227" | b"2BM" | b"23L" | b"23M" | b"270"
        )
    {
        return Availability::UnverifiedDevice;
    }
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    let version = std::fs::read_to_string("/etc/version.txt").unwrap_or_default();
    if kernel.trim() != "4.9.77-lab126"
        || version.lines().next()
            != Some("System Software Version: 058-juno_1905_barolo_bellatrix3-479432")
    {
        return Availability::UnverifiedFirmware;
    }
    Availability::Available
}

#[repr(C)]
struct Report {
    abi: u32,
    size: u32,
    width: u32,
    height: u32,
    bytes: u32,
    events: u32,
    error: i32,
    cleanup_error: i32,
    driver_ms: f64,
    compute_ms: f64,
    stage: [u8; 64],
}

unsafe extern "C" {
    fn aidoku_jpeg_worker_query() -> libc::c_int;
    fn aidoku_jpeg_worker_decode(
        input: *const u8,
        length: usize,
        width: u32,
        height: u32,
        target_width: u32,
        target_height: u32,
        output: *mut u8,
        output_length: usize,
        report: *mut Report,
    ) -> libc::c_int;
}

struct KernelLog(File);
impl KernelLog {
    fn open() -> std::io::Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open("/dev/kmsg")?;
        file.seek(SeekFrom::End(0))?;
        Ok(Self(file))
    }

    fn clean(&mut self) -> bool {
        let mut buffer = [0; 8192];
        let mut total = 0;
        loop {
            match self.0.read(&mut buffer) {
                Ok(0) => return true,
                Ok(count) => {
                    total += count;
                    let line = String::from_utf8_lossy(&buffer[..count]).to_ascii_lowercase();
                    if total > 65536
                        || [
                            "fault type=",
                            "dma-api",
                            "timeout",
                            "unhandled fault",
                            "kernel panic",
                            "bug:",
                            "oops",
                            "smi bus hang",
                        ]
                        .iter()
                        .any(|term| line.contains(term))
                    {
                        return false;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return true,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return false,
            }
        }
    }
}

fn lock() -> Result<File, Availability> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open("/tmp/kindle-jpeg-hardware.lock")
        .map_err(|_| Availability::Busy)?;
    let metadata = file.metadata().map_err(|_| Availability::Busy)?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
        return Err(Availability::Busy);
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(Availability::Busy);
    }
    Ok(file)
}

fn reply(
    status: u32,
    error: i32,
    width: u32,
    height: u32,
    stage: &[u8],
    pixels: &[u8],
) -> std::io::Result<()> {
    let mut header = [0_u8; 96];
    header[..8].copy_from_slice(RESPONSE_MAGIC);
    for (offset, value) in [
        (8, status),
        (12, error as u32),
        (16, width),
        (20, height),
        (24, pixels.len() as u32),
    ] {
        header[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    let length = stage.len().min(64);
    header[32..32 + length].copy_from_slice(&stage[..length]);
    let mut output = std::io::stdout().lock();
    output.write_all(&header)?;
    output.write_all(pixels)?;
    output.flush()
}

fn run() -> std::io::Result<()> {
    let mut input = std::io::stdin().lock();
    let mut request = [0; 32];
    input.read_exact(&mut request)?;
    let word = |offset: usize| u32::from_le_bytes(request[offset..offset + 4].try_into().unwrap());
    let (operation, width, height, length) = (word(8), word(16), word(20), word(24) as usize);
    if &request[..8] != REQUEST_MAGIC
        || operation > 1
        || length > jpeg::MAX_ENCODED_BYTES
        || word(12) != 0
        || word(28) != 0
    {
        return reply(10, libc::EINVAL, 0, 0, b"invalid_request", &[]);
    }
    let qualified = qualified_device();
    if !qualified.available() {
        return reply(
            qualified as u32,
            libc::ENOTSUP,
            0,
            0,
            qualified.reason().as_bytes(),
            &[],
        );
    }
    let _lock = match lock() {
        Ok(lock) => lock,
        Err(reason) => return reply(reason as u32, libc::EBUSY, 0, 0, b"busy", &[]),
    };
    let mut log = match KernelLog::open() {
        Ok(log) => log,
        Err(_) => {
            return reply(
                Availability::MissingDiagnostics as u32,
                libc::EACCES,
                0,
                0,
                b"kernel_diagnostics",
                &[],
            );
        }
    };
    if operation == 0 {
        if length != 0 || width != 0 || height != 0 {
            return reply(10, libc::EINVAL, 0, 0, b"invalid_probe", &[]);
        }
        let error = unsafe { aidoku_jpeg_worker_query() };
        let clean = log.clean();
        return reply(
            if error == 0 && clean {
                0
            } else {
                Availability::MissingDriver as u32
            },
            error,
            0,
            0,
            b"driver_query",
            &[],
        );
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    let parsed = match jpeg::parse(&bytes) {
        Ok(parsed) if parsed.validate_entropy(&bytes) => parsed,
        _ => return reply(10, libc::ENOTSUP, 0, 0, b"jpeg_admission", &[]),
    };
    let information = parsed.information;
    let length = u64::from(width) * u64::from(height);
    if width < 16
        || height < 16
        || width > information.width
        || height > information.height
        || length > jpeg::MAX_PIXELS as u64
        || (information.sampling != jpeg::Sampling::Gray && width % 2 != 0)
    {
        return reply(10, libc::EINVAL, 0, 0, b"target_geometry", &[]);
    }
    let mut pixels = vec![0; length as usize];
    let mut report = Report {
        abi: 0,
        size: 0,
        width: 0,
        height: 0,
        bytes: 0,
        events: 0,
        error: 0,
        cleanup_error: 0,
        driver_ms: 0.0,
        compute_ms: 0.0,
        stage: [0; 64],
    };
    assert_eq!(std::mem::size_of::<Report>(), 112);
    // No asynchronous Rust cancellation, allocator activity, or early return
    // crosses this call. The C owner retains buffers through confirmed stop.
    let status = unsafe {
        aidoku_jpeg_worker_decode(
            bytes.as_ptr(),
            bytes.len(),
            information.width,
            information.height,
            width,
            height,
            pixels.as_mut_ptr(),
            pixels.len(),
            &mut report,
        )
    };
    let clean = log.clean();
    if status != 0
        || report.abi != 1
        || report.size != 112
        || report.width != width
        || report.height != height
        || report.bytes as usize != pixels.len()
        || report.error != 0
        || report.cleanup_error != 0
        || report.driver_ms >= 900.0
        || !clean
    {
        return reply(
            Availability::Failed as u32,
            if clean {
                report.error.max(report.cleanup_error)
            } else {
                libc::EIO
            },
            0,
            0,
            if clean {
                &report.stage
            } else {
                b"kernel_fault"
            },
            &[],
        );
    }
    reply(0, 0, width, height, b"complete", &pixels)
}

pub(super) fn main() -> i32 {
    unsafe {
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::signal(signal, libc::SIG_IGN);
        }
    }
    match run() {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
// Adapted from AidokuForKindle's 2026-11-15 media candidate.
// Source identity, original snapshots, and changes: vendor/aidoku-native-media/README.md.

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
        || !qualified_model(&identity)
    {
        return Availability::UnverifiedDevice;
    }
    Availability::Available
}

fn qualified_model(identity: &[u8; 16]) -> bool {
    identity[0] == b'G'
        && identity.iter().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        && matches!(
            &identity[3..6],
            b"27J" | b"2BL" | b"263" | b"227" | b"2BM" | b"23L" | b"23M" | b"270"
        )
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
    lock_at(std::path::Path::new("/tmp/kindle-jpeg-hardware.lock"))
}

fn lock_at(path: &std::path::Path) -> Result<File, Availability> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
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
        Ok(parsed)
            if parsed.information.sampling == jpeg::Sampling::Gray
                && parsed.validate_entropy(&bytes) => parsed,
        _ => return reply(10, libc::ENOTSUP, 0, 0, b"jpeg_admission", &[]),
    };
    let information = parsed.information;
    if !super::eligible_target(information, (width, height)) {
        return reply(10, libc::EINVAL, 0, 0, b"target_geometry", &[]);
    }
    let length = u64::from(width) * u64::from(height);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        os::unix::fs::{PermissionsExt, symlink},
        path::Path,
        process::{Child, ChildStdin, Command, Stdio},
        sync::mpsc::{self, Receiver},
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    };

    struct LockPeer {
        child: Child,
        input: Option<ChildStdin>,
        lines: Receiver<String>,
        reader: Option<JoinHandle<()>>,
    }

    impl LockPeer {
        fn start(path: &Path) -> Self {
            // This process models another application using the shared flock.
            // It never opens a device or loads the native codec implementation.
            let script = r#"
import fcntl
import os
import sys

descriptor = os.open(sys.argv[1], os.O_RDWR | os.O_CLOEXEC | os.O_NOFOLLOW)
try:
    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError:
    print("busy", flush=True)
else:
    print("unexpected_owner", flush=True)
    sys.exit(2)

if sys.stdin.buffer.readline() != b"acquire\n":
    sys.exit(3)
fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
print("owned", flush=True)
if sys.stdin.buffer.readline() != b"exit\n":
    sys.exit(4)
# Process exit closes the descriptor and releases its flock.
"#;
            let mut child = Command::new("python3")
                .args(["-u", "-c", script])
                .arg(path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("start an independent shared-lock peer");
            let input = child.stdin.take();
            let output = child.stdout.take().unwrap();
            let (sender, lines) = mpsc::channel();
            let reader = thread::spawn(move || {
                for line in BufReader::new(output).lines() {
                    let Ok(line) = line else {
                        break;
                    };
                    if sender.send(line).is_err() {
                        break;
                    }
                }
            });
            Self {
                child,
                input,
                lines,
                reader: Some(reader),
            }
        }

        fn expect_line(&self, expected: &str) {
            assert_eq!(
                self.lines
                    .recv_timeout(Duration::from_secs(5))
                    .expect("receive bounded shared-lock peer response"),
                expected
            );
        }

        fn command(&mut self, command: &[u8]) {
            let input = self.input.as_mut().unwrap();
            input.write_all(command).unwrap();
            input.flush().unwrap();
        }

        fn finish(mut self) {
            self.command(b"exit\n");
            self.input.take();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    assert!(status.success(), "shared-lock peer failed: {status}");
                    return;
                }
                assert!(Instant::now() < deadline, "shared-lock peer did not exit");
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Drop for LockPeer {
        fn drop(&mut self) {
            self.input.take();
            if !matches!(self.child.try_wait(), Ok(Some(_))) {
                // Only this host-side Python simulation is terminated on a
                // failed test; no codec worker or hardware owner is involved.
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }

    #[test]
    fn model_only_admission_excludes_later_scribes_and_malformed_identifiers() {
        // The hardware predicate has no kernel, firmware, or version input.
        let model_only: fn(&[u8; 16]) -> bool = qualified_model;
        for code in ["27J", "2BL", "263", "227", "2BM", "23L", "23M", "270"] {
            let identity: [u8; 16] = format!("G00{code}0000000000").into_bytes().try_into().unwrap();
            assert!(model_only(&identity));
        }
        for code in ["3V0", "3V1", "3X5", "3UV", "2DK", "000"] {
            let identity: [u8; 16] = format!("G00{code}0000000000").into_bytes().try_into().unwrap();
            assert!(!model_only(&identity));
        }
        assert!(!model_only(b"B0023L0000000000"));
        assert!(!model_only(b"G0023L000000000!"));
    }

    #[test]
    fn hardware_lock_is_shared_across_processes_and_reopens_after_owner_exit() {
        let directory = tempfile::Builder::new()
            .prefix("bilicomics-native-media-lock-")
            .tempdir()
            .unwrap();
        let path = directory.path().join("shared.lock");
        let owner = lock_at(&path).unwrap();
        let inode = owner.metadata().unwrap().ino();
        let mut peer = LockPeer::start(&path);
        peer.expect_line("busy");
        drop(owner);
        peer.command(b"acquire\n");
        peer.expect_line("owned");

        // Each attempt opens a fresh descriptor, as an application restart
        // does, while the independent simulated worker remains alive.
        for _ in 0..2 {
            assert!(matches!(lock_at(&path), Err(Availability::Busy)));
        }
        peer.finish();
        let reopened = lock_at(&path).unwrap();
        assert_eq!(reopened.metadata().unwrap().ino(), inode);
    }

    #[test]
    fn hardware_lock_creates_a_private_regular_close_on_exec_file() {
        let directory = tempfile::Builder::new()
            .prefix("bilicomics-native-media-lock-")
            .tempdir()
            .unwrap();
        let owner = lock_at(&directory.path().join("shared.lock")).unwrap();
        let metadata = owner.metadata().unwrap();
        assert!(metadata.is_file());
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        assert_eq!(metadata.nlink(), 1);
        assert_eq!(metadata.mode() & 0o777, 0o600);
        let flags = unsafe { libc::fcntl(owner.as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0);
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
    }

    #[test]
    fn hardware_lock_rejects_symbolic_links_and_hard_linked_inodes() {
        let directory = tempfile::Builder::new()
            .prefix("bilicomics-native-media-lock-")
            .tempdir()
            .unwrap();
        let path = directory.path().join("shared.lock");
        drop(lock_at(&path).unwrap());
        let symbolic = directory.path().join("symbolic.lock");
        symlink(&path, &symbolic).unwrap();
        assert!(matches!(lock_at(&symbolic), Err(Availability::Busy)));
        let hard = directory.path().join("hard.lock");
        std::fs::hard_link(&path, &hard).unwrap();
        assert_eq!(path.metadata().unwrap().nlink(), 2);
        assert!(matches!(lock_at(&path), Err(Availability::Busy)));
        assert!(matches!(lock_at(&hard), Err(Availability::Busy)));
    }

    #[test]
    fn hardware_lock_rejects_group_or_other_permissions() {
        let directory = tempfile::Builder::new()
            .prefix("bilicomics-native-media-lock-")
            .tempdir()
            .unwrap();
        let path = directory.path().join("shared.lock");
        drop(lock_at(&path).unwrap());
        for mode in [0o640, 0o604, 0o666] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(matches!(lock_at(&path), Err(Availability::Busy)));
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(lock_at(&path).is_ok());
    }
}

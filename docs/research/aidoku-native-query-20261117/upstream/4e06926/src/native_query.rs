//! Bounded transport to an optional firmware-ABI, read-only LIPC helper.
//!
//! An unavailable helper falls back before any query is sent. Once connected,
//! malformed or incomplete replies poison the session; they are never retried
//! as part of a portrait transaction. Callers must finish before publication.

use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::AsRawFd,
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};

const MAX_VALUE: usize = 128;
const HELLO: &[u8; 4] = b"AQ1\n";

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub(crate) enum Property {
    PowerState = 1,
    DisplayMode = 2,
    Portrait = 3,
    WifiState = 4,
    Battery = 5,
    Charging = 6,
    Brightness = 7,
    Warmth = 8,
}

impl Property {
    pub(crate) fn legacy_arguments(self) -> &'static [&'static str] {
        match self {
            Self::PowerState => &["-s", "com.lab126.powerd", "state"],
            Self::DisplayMode => &["-s", "com.lab126.winmgr", "epdcMode"],
            Self::Portrait => &["com.lab126.winmgr", "accelerometer"],
            Self::WifiState => &["com.lab126.wifid", "cmState"],
            Self::Battery => &["com.lab126.powerd", "battLevel"],
            Self::Charging => &["com.lab126.powerd", "isCharging"],
            Self::Brightness => &["com.lab126.powerd", "flIntensity"],
            Self::Warmth => &["com.lab126.powerd", "currentAmberLevel"],
        }
    }
}

pub(crate) struct Session {
    child: Child,
    input: Option<ChildStdin>,
    output: ChildStdout,
    broken: bool,
}

impl Session {
    pub(crate) fn try_open(deadline: Instant, cancelled: &impl Fn() -> bool) -> Option<Self> {
        if cancelled() || Instant::now() >= deadline || !system_abi_supported() {
            return None;
        }
        let application = std::env::current_exe().ok()?;
        let helper = application.parent()?.join("aidoku-native-query");
        if !helper.is_file() {
            return None;
        }
        Self::open_at(
            &helper,
            deadline.min(Instant::now() + Duration::from_millis(250)),
            cancelled,
        )
        .ok()
    }

    fn open_at(helper: &Path, deadline: Instant, cancelled: &impl Fn() -> bool) -> Result<Self> {
        check_deadline(deadline, cancelled)?;
        let mut child = Command::new(helper)
            .arg("--session")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("Start the native query helper")?;
        // Both pipes are guaranteed by the Command configuration above.
        let input = child.stdin.take().expect("piped native query stdin");
        let output = child.stdout.take().expect("piped native query stdout");
        let mut session = Self {
            child,
            input: Some(input),
            output,
            broken: false,
        };
        nonblocking(session.input.as_ref().unwrap().as_raw_fd())?;
        nonblocking(session.output.as_raw_fd())?;
        let mut hello = [0; 4];
        let fd = session.output.as_raw_fd();
        read_exact(&mut session.output, fd, &mut hello, deadline, cancelled)?;
        ensure!(
            &hello == HELLO,
            "The native query helper protocol is unavailable"
        );
        crate::reader_timing::mark("native_query_session_ready");
        Ok(session)
    }

    pub(crate) fn query(
        &mut self,
        property: Property,
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
    ) -> Result<String> {
        ensure!(!self.broken, "The native query session is no longer usable");
        check_deadline(deadline, cancelled)?;
        self.broken = true;
        let input = self
            .input
            .as_mut()
            .context("The native query session is closed")?;
        loop {
            check_deadline(deadline, cancelled)?;
            match input.write(&[property as u8]) {
                Ok(1) => break,
                Ok(_) => bail!("The native query request was not written"),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    wait_io(input.as_raw_fd(), libc::POLLOUT, deadline, cancelled)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        let fd = self.output.as_raw_fd();
        let mut header = [0; 4];
        read_exact(&mut self.output, fd, &mut header, deadline, cancelled)?;
        let length = usize::from(u16::from_be_bytes([header[2], header[3]]));
        ensure!(
            header[0] == property as u8 && header[1] <= 2 && length <= MAX_VALUE,
            "Invalid native query reply framing"
        );
        ensure!(
            header[1] == 0 || length == 0,
            "A failed native query returned data"
        );
        let mut bytes = vec![0; length];
        read_exact(&mut self.output, fd, &mut bytes, deadline, cancelled)?;
        let value = std::str::from_utf8(&bytes)?;
        ensure!(!value.contains('\0'), "A native query value contains NUL");
        // A well-framed native property error does not poison later UI reads.
        self.broken = false;
        ensure!(
            header[1] == 0,
            "Native property {property:?} is unavailable"
        );
        crate::reader_timing::mark("native_query_response");
        Ok(value.trim_ascii().to_owned())
    }

    /// Require clean EOF and successful termination before trusting a batch.
    pub(crate) fn finish(mut self, deadline: Instant, cancelled: &impl Fn() -> bool) -> Result<()> {
        ensure!(!self.broken, "The native query session failed");
        self.input.take();
        let mut eof = false;
        loop {
            check_deadline(deadline, cancelled)?;
            if !eof {
                match self.output.read(&mut [0; 1]) {
                    Ok(0) => eof = true,
                    Ok(_) => bail!("The native query helper returned trailing data"),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.into()),
                }
            }
            if let Some(status) = self.child.try_wait()? {
                ensure!(
                    status.success(),
                    "The native query helper failed with {status}"
                );
                if eof {
                    crate::reader_timing::mark("native_query_session_finished");
                    return Ok(());
                }
            }
            if eof {
                std::thread::sleep(Duration::from_millis(1));
            } else {
                wait_io(self.output.as_raw_fd(), libc::POLLIN, deadline, cancelled)?;
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // An unreaped owned child cannot have its PID reused between this check
        // and kill. Do not signal a PID when waitpid reports unknown ownership.
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn system_abi_supported() -> bool {
    ["/usr/lib/liblipc.so.1", "/usr/lib/liblipc.so.1.0"]
        .iter()
        .any(|path| {
            let mut header = [0; 52];
            File::open(path)
                .and_then(|mut file| file.read_exact(&mut header))
                .is_ok()
                && armhf_library(&header)
        })
}

fn armhf_library(header: &[u8]) -> bool {
    header.len() >= 52
        && &header[..7] == b"\x7fELF\x01\x01\x01"
        && u16::from_le_bytes([header[16], header[17]]) == 3
        && u16::from_le_bytes([header[18], header[19]]) == 40
        && u32::from_le_bytes(header[36..40].try_into().unwrap()) & 0x600 == 0x400
}

fn nonblocking(fd: libc::c_int) -> Result<()> {
    // SAFETY: fd is an owned pipe; these operations only change its I/O flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

fn check_deadline(deadline: Instant, cancelled: &impl Fn() -> bool) -> Result<()> {
    ensure!(
        !cancelled() && Instant::now() < deadline,
        "The native query was cancelled or timed out"
    );
    Ok(())
}

fn wait_io(
    fd: libc::c_int,
    events: i16,
    deadline: Instant,
    cancelled: &impl Fn() -> bool,
) -> Result<()> {
    check_deadline(deadline, cancelled)?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    let timeout = remaining.as_millis().clamp(1, 5) as libc::c_int;
    let mut descriptor = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    // SAFETY: The pollfd points to our live pipe and stays valid for this call.
    let result = unsafe { libc::poll(&mut descriptor, 1, timeout) };
    if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
        return Err(io::Error::last_os_error().into());
    }
    ensure!(
        descriptor.revents & (libc::POLLERR | libc::POLLNVAL) == 0,
        "The native query pipe failed"
    );
    Ok(())
}

fn read_exact(
    reader: &mut impl Read,
    fd: libc::c_int,
    mut bytes: &mut [u8],
    deadline: Instant,
    cancelled: &impl Fn() -> bool,
) -> Result<()> {
    while !bytes.is_empty() {
        check_deadline(deadline, cancelled)?;
        match reader.read(bytes) {
            Ok(0) => bail!("The native query helper closed an incomplete reply"),
            Ok(count) => bytes = &mut bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                wait_io(fd, libc::POLLIN, deadline, cancelled)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn fixture(mode: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("query");
        fs::write(
            &script,
            format!(
                r#"#!/usr/bin/python3 -I
import os, sys, time
mode = {mode:?}
if mode == 'bad-hello':
    os.write(1, b'BAD!'); sys.exit(0)
os.write(1, b'AQ1\n')
while True:
    opcode = os.read(0, 1)
    if not opcode: break
    if mode == 'hang': time.sleep(20)
    if mode == 'short': os.write(1, opcode + b'\x00'); sys.exit(0)
    if mode == 'oversized': os.write(1, opcode + b'\x00\x00\x81'); continue
    if mode == 'wrong-tag': os.write(1, b'\xff\x00\x00\x00'); continue
    if mode == 'native-error' and opcode == b'\x01':
        os.write(1, opcode + b'\x01\x00\x00'); continue
    value = b'active' if opcode == b'\x01' else b'42'
    os.write(1, opcode + b'\x00' + len(value).to_bytes(2, 'big') + value)
    if mode == 'bad-exit': sys.exit(2)
if mode == 'trailing': os.write(1, b'extra')
"#
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        (root, script)
    }

    #[test]
    fn one_session_serves_ordered_properties_and_finishes_cleanly() {
        let (_root, path) = fixture("normal");
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut session = Session::open_at(&path, deadline, &|| false).unwrap();
        assert_eq!(
            session
                .query(Property::PowerState, deadline, &|| false)
                .unwrap(),
            "active"
        );
        assert_eq!(
            session
                .query(Property::Battery, deadline, &|| false)
                .unwrap(),
            "42"
        );
        session.finish(deadline, &|| false).unwrap();
    }

    #[test]
    fn malformed_or_incomplete_replies_poison_the_session() {
        for mode in ["short", "oversized", "wrong-tag"] {
            let (_root, path) = fixture(mode);
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut session = Session::open_at(&path, deadline, &|| false).unwrap();
            assert!(
                session
                    .query(Property::PowerState, deadline, &|| false)
                    .is_err()
            );
            assert!(
                session
                    .query(Property::Battery, deadline, &|| false)
                    .is_err()
            );
            assert!(session.finish(deadline, &|| false).is_err());
        }
    }

    #[test]
    fn native_property_errors_do_not_hide_later_values() {
        let (_root, path) = fixture("native-error");
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut session = Session::open_at(&path, deadline, &|| false).unwrap();
        assert!(
            session
                .query(Property::PowerState, deadline, &|| false)
                .is_err()
        );
        assert_eq!(
            session
                .query(Property::Battery, deadline, &|| false)
                .unwrap(),
            "42"
        );
        session.finish(deadline, &|| false).unwrap();
    }

    #[test]
    fn publication_requires_a_clean_successful_end() {
        for mode in ["bad-exit", "trailing"] {
            let (_root, path) = fixture(mode);
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut session = Session::open_at(&path, deadline, &|| false).unwrap();
            session
                .query(Property::PowerState, deadline, &|| false)
                .unwrap();
            assert!(session.finish(deadline, &|| false).is_err());
        }
    }

    #[test]
    fn blocked_reads_and_cancellation_remain_bounded() {
        for cancel in [false, true] {
            let (_root, path) = fixture("hang");
            let mut session =
                Session::open_at(&path, Instant::now() + Duration::from_secs(2), &|| false)
                    .unwrap();
            let start = Instant::now();
            assert!(
                session
                    .query(
                        Property::PowerState,
                        start + Duration::from_millis(40),
                        &|| cancel
                    )
                    .is_err()
            );
            drop(session);
            assert!(start.elapsed() < Duration::from_secs(1));
        }
    }

    #[test]
    fn unsupported_protocol_is_rejected_before_queries() {
        let (_root, path) = fixture("bad-hello");
        assert!(
            Session::open_at(&path, Instant::now() + Duration::from_secs(1), &|| false).is_err()
        );
    }

    #[test]
    fn firmware_abi_gate_rejects_other_architectures_and_float_abis() {
        let mut header = [0; 52];
        header[..7].copy_from_slice(b"\x7fELF\x01\x01\x01");
        header[16] = 3;
        header[18] = 40;
        header[36..40].copy_from_slice(&0x5000400u32.to_le_bytes());
        assert!(armhf_library(&header));
        assert!(!armhf_library(&header[..51]));
        header[36..40].copy_from_slice(&0x5000200u32.to_le_bytes());
        assert!(!armhf_library(&header));
        header[36..40].copy_from_slice(&0x5000400u32.to_le_bytes());
        header[18] = 183;
        assert!(!armhf_library(&header));
    }
}

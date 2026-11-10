//! One read-only transaction for the launcher's native display snapshot.

use super::{DeviceProfile, PlatformError};
use std::time::Duration;

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(2);
const RECORD_LIMIT: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PortraitMetadata {
    automatic: bool,
    rotation: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Query {
    Power,
    Mode,
    Holding,
}

trait CaptureSource {
    fn elapsed(&self) -> Duration;
    fn check_context(&mut self) -> Result<(), PlatformError>;
    fn query(&mut self, query: Query, remaining: Duration) -> Result<String, PlatformError>;
    fn metadata(&mut self, remaining: Duration) -> Result<PortraitMetadata, PlatformError>;
}

fn failure(message: &'static str) -> PlatformError {
    PlatformError::Operation(message.into())
}

fn timeout() -> PlatformError {
    failure("Native display capture timed out")
}

fn checkpoint(source: &mut impl CaptureSource) -> Result<Duration, PlatformError> {
    if source.elapsed() >= CAPTURE_TIMEOUT {
        return Err(timeout());
    }
    source.check_context()?;
    CAPTURE_TIMEOUT
        .checked_sub(source.elapsed())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(timeout)
}

/// Firmware values may have one final LF, but no other framing or control bytes.
fn record_text(raw: &str) -> Result<&str, PlatformError> {
    if raw.is_empty() || raw.len() > RECORD_LIMIT {
        return Err(failure("Native display value exceeded its record bounds"));
    }
    let value = raw.strip_suffix('\n').unwrap_or(raw);
    if value.is_empty() || !value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
        return Err(failure("Native display value has invalid record framing"));
    }
    Ok(value)
}

fn read_query(source: &mut impl CaptureSource, query: Query) -> Result<String, PlatformError> {
    let remaining = checkpoint(source)?;
    let raw = source.query(query, remaining)?;
    checkpoint(source)?;
    record_text(&raw)?;
    Ok(raw)
}

fn read_metadata(source: &mut impl CaptureSource) -> Result<PortraitMetadata, PlatformError> {
    let remaining = checkpoint(source)?;
    let metadata = source.metadata(remaining)?;
    checkpoint(source)?;
    if metadata.rotation > 3 {
        return Err(failure("Native framebuffer rotation is not a quarter-turn value"));
    }
    Ok(metadata)
}

fn holding(profile: DeviceProfile, raw: &str) -> Result<&'static str, PlatformError> {
    match record_text(raw)? {
        "U" => Ok("U"),
        "D" => Ok("D"),
        "L" if profile == DeviceProfile::Scribe => Ok("U"),
        "R" if profile == DeviceProfile::Scribe => Ok("D"),
        _ => Err(failure("Native portrait holding direction is unknown")),
    }
}

fn capture_with(
    profile: DeviceProfile,
    source: &mut impl CaptureSource,
) -> Result<String, PlatformError> {
    let power = read_query(source, Query::Power)?;
    if record_text(&power)? != "active" {
        return Ok("not-active".into());
    }
    let mode = read_query(source, Query::Mode)?;
    let mode = match record_text(&mode)? {
        "Y8" => "Y8",
        "Y8INV" => "Y8INV",
        _ => return Err(failure("Native display mode is unknown")),
    };
    let first = read_metadata(source)?;
    let direction = if first.automatic {
        holding(profile, &read_query(source, Query::Holding)?)?
    } else {
        "FIXED"
    };
    if read_metadata(source)? != first {
        return Err(failure("Native framebuffer or accelerometer availability changed"));
    }
    if first.automatic && holding(profile, &read_query(source, Query::Holding)?)? != direction {
        return Err(failure("Native portrait holding direction changed"));
    }
    let power = read_query(source, Query::Power)?;
    if record_text(&power)? != "active" {
        return Ok("not-active".into());
    }
    Ok(format!("ready:{mode}:{direction}:{}", first.rotation))
}

#[cfg(all(feature = "kindle", target_os = "linux"))]
pub(super) fn capture(profile: DeviceProfile) -> Result<String, PlatformError> {
    system::capture(profile)
}

#[cfg(all(feature = "kindle", target_os = "linux"))]
mod system {
    use super::*;
    use crate::native_presentation::NativeReadPermit;
    use crate::platform::{DeviceControl, native::NativeControl, validate_native_launch_approval};
    use std::{
        io::{self, Read},
        os::fd::AsRawFd,
        path::{Path, PathBuf},
        process::{Child, Command, ExitStatus, Stdio},
        sync::mpsc::{self, Sender},
        time::Instant,
    };

    const POLL_INTERVAL: Duration = Duration::from_millis(10);
    const CONTEXT_INTERVAL: Duration = Duration::from_millis(100);

    fn context_error(error: anyhow::Error) -> PlatformError {
        PlatformError::Operation(format!("Invalid native-read context: {error:#}"))
    }

    pub(super) fn capture(profile: DeviceProfile) -> Result<String, PlatformError> {
        let started = Instant::now();
        validate_native_launch_approval(profile)?;
        let permit = NativeReadPermit::acquire().map_err(context_error)?;
        permit.recheck().map_err(context_error)?;
        let lipc = super::super::native::find_command("lipc-get-prop")
            .ok_or(PlatformError::Unsupported("lipc"))?;
        let mut source = NativeSource {
            profile,
            started,
            permit,
            lipc,
            commands: Commands::new()?,
        };
        capture_with(profile, &mut source)
    }

    struct NativeSource {
        profile: DeviceProfile,
        started: Instant,
        permit: NativeReadPermit,
        lipc: PathBuf,
        commands: Commands,
    }

    impl CaptureSource for NativeSource {
        fn elapsed(&self) -> Duration {
            self.started.elapsed()
        }

        fn check_context(&mut self) -> Result<(), PlatformError> {
            self.permit.recheck().map_err(context_error)
        }

        fn query(&mut self, query: Query, _: Duration) -> Result<String, PlatformError> {
            let arguments: &[&str] = match query {
                Query::Power => &["-s", "com.lab126.powerd", "state"],
                Query::Mode => &["-s", "com.lab126.winmgr", "epdcMode"],
                Query::Holding => &["com.lab126.winmgr", "accelerometer"],
            };
            let deadline = self.started.checked_add(CAPTURE_TIMEOUT).ok_or_else(timeout)?;
            self.commands.run(&self.lipc, arguments, deadline, || {
                self.permit.recheck().map_err(context_error)
            })
        }

        fn metadata(&mut self, _: Duration) -> Result<PortraitMetadata, PlatformError> {
            // NativeControl::new itself opens /dev/fb0. Authenticate again
            // before that first native I/O, not merely before the later probe.
            checkpoint(self)?;
            let native = NativeControl::new(self.profile)?;
            checkpoint(self)?;
            let observed = slint_backend_kindle::probe_native_framebuffer(native.native_viewport())?;
            checkpoint(self)?;
            let automatic = slint_backend_kindle::native_accelerometer_available()?;
            Ok(PortraitMetadata {
                automatic,
                rotation: observed.rotation,
            })
        }
    }

    /// There is only one foreground command. An error stops the transaction;
    /// at most one killed, unreaped child can be handed to this owned worker.
    struct Commands {
        reaper: Sender<Child>,
    }

    impl Commands {
        fn new() -> Result<Self, PlatformError> {
            let (reaper, receiver) = mpsc::channel::<Child>();
            std::thread::Builder::new()
                .name("bilicomics-capture-reaper".into())
                .spawn(move || {
                    while let Ok(mut child) = receiver.recv() {
                        loop {
                            match child.wait() {
                                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                                _ => break,
                            }
                        }
                    }
                })?;
            Ok(Self { reaper })
        }

        fn run(
            &self,
            program: &Path,
            arguments: &[&str],
            deadline: Instant,
            mut check_context: impl FnMut() -> Result<(), PlatformError>,
        ) -> Result<String, PlatformError> {
            ensure_deadline(deadline)?;
            check_context()?;
            ensure_deadline(deadline)?;
            // Do not create a new group or session: the launcher's original
            // two-second watchdog must also own every command descendant.
            let child = Command::new(program)
                .args(arguments)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?;
            let mut owned = OwnedChild {
                child: Some(child),
                reaper: &self.reaper,
            };
            let mut stdout = owned
                .child
                .as_mut()
                .and_then(|child| child.stdout.take())
                .ok_or_else(|| failure("Native command stdout is unavailable"))?;
            let descriptor = stdout.as_raw_fd();
            // SAFETY: this is the owned stdout pipe; fcntl only updates its flags.
            let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
            if flags == -1
                || unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
            {
                return Err(io::Error::last_os_error().into());
            }
            let mut output = Vec::with_capacity(RECORD_LIMIT);
            let mut buffer = [0; RECORD_LIMIT + 1];
            let mut closed = false;
            let mut exited = false;
            let mut next_context_check = Instant::now() + CONTEXT_INTERVAL;
            loop {
                ensure_current(deadline, &mut next_context_check, &mut check_context)?;
                while !closed {
                    ensure_current(deadline, &mut next_context_check, &mut check_context)?;
                    let allowance = RECORD_LIMIT - output.len();
                    match stdout.read(&mut buffer[..allowance + 1]) {
                        Ok(0) => closed = true,
                        Ok(count) if count > allowance => {
                            return Err(failure("Native command output exceeded 128 bytes"));
                        }
                        Ok(count) => output.extend_from_slice(&buffer[..count]),
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        Err(error) => return Err(error.into()),
                    }
                }
                if !exited {
                    if let Some(status) = owned.try_wait()? {
                        if !status.success() {
                            return Err(failure("Native display getter was rejected"));
                        }
                        exited = true;
                    }
                }
                if closed && exited {
                    check_context()?;
                    ensure_deadline(deadline)?;
                    return String::from_utf8(output)
                        .map_err(|_| failure("Native command output is not valid UTF-8"));
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(timeout());
                }
                let mut descriptor = libc::pollfd {
                    fd: if closed { -1 } else { descriptor },
                    events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
                    revents: 0,
                };
                // SAFETY: poll borrows this single initialized descriptor.
                let result = unsafe {
                    libc::poll(
                        &mut descriptor,
                        1,
                        remaining.min(POLL_INTERVAL).as_millis().max(1) as libc::c_int,
                    )
                };
                if result == -1 {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::Interrupted {
                        return Err(error.into());
                    }
                } else if descriptor.revents & libc::POLLNVAL != 0 {
                    return Err(failure("Native command stdout became invalid"));
                }
            }
        }
    }

    fn ensure_deadline(deadline: Instant) -> Result<(), PlatformError> {
        if Instant::now() >= deadline {
            Err(timeout())
        } else {
            Ok(())
        }
    }

    fn ensure_current(
        deadline: Instant,
        next_check: &mut Instant,
        check_context: &mut impl FnMut() -> Result<(), PlatformError>,
    ) -> Result<(), PlatformError> {
        ensure_deadline(deadline)?;
        if Instant::now() >= *next_check {
            check_context()?;
            *next_check = Instant::now() + CONTEXT_INTERVAL;
        }
        ensure_deadline(deadline)
    }

    struct OwnedChild<'a> {
        child: Option<Child>,
        reaper: &'a Sender<Child>,
    }

    impl OwnedChild<'_> {
        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            let status = self.child.as_mut().expect("The native getter is still owned").try_wait()?;
            if status.is_some() {
                self.child.take();
            }
            Ok(status)
        }
    }

    impl Drop for OwnedChild<'_> {
        fn drop(&mut self) {
            let Some(mut child) = self.child.take() else { return };
            child.stdout.take();
            match child.try_wait() {
                Ok(Some(_)) => return,
                Err(error) if error.raw_os_error() == Some(libc::ECHILD) => return,
                _ => {}
            }
            // Only the unreaped Child is signalled here. The launcher retains
            // the enclosing group's identity and cleans all its descendants.
            let _ = child.kill();
            match child.try_wait() {
                Ok(Some(_)) => {}
                Err(error) if error.raw_os_error() == Some(libc::ECHILD) => {}
                _ => {
                    let _ = self.reaper.send(child);
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{cell::Cell, fs};

        fn run(script: &str) -> Result<String, PlatformError> {
            Commands::new()?.run(
                Path::new("/bin/sh"),
                &["-c", script],
                Instant::now() + Duration::from_secs(1),
                || Ok(()),
            )
        }

        fn wait_for_reaping(pid: u32) {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Path::new(&format!("/proc/{pid}/stat")).exists() {
                assert!(Instant::now() < deadline, "The owned getter was not reaped");
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        #[test]
        fn capture_commands_keep_the_watchdog_group_and_require_successful_exit() {
            let group = run(
                "IFS= read -r record </proc/$$/stat; tail=${record##*) }; set -- $tail; printf '%s' \"$3\"",
            ).unwrap();
            // SAFETY: getpgrp reads this process's existing group identity.
            assert_eq!(group, unsafe { libc::getpgrp() }.to_string());
            assert_eq!(run("printf 'Y8\\n'").unwrap(), "Y8\n");
            assert!(run("printf 'Y8\\n'; exit 7").unwrap_err().to_string().contains("rejected"));
        }

        #[test]
        fn capture_commands_reject_overflow_and_invalid_utf8() {
            assert_eq!(run("printf '%128s' ''").unwrap().len(), RECORD_LIMIT);
            assert!(run("printf '%129s' ''").unwrap_err().to_string().contains("128 bytes"));
            assert!(run(r"printf '\377'").unwrap_err().to_string().contains("UTF-8"));
        }

        #[test]
        fn context_and_deadline_failures_precede_command_creation() {
            let commands = Commands::new().unwrap();
            let error = commands.run(
                Path::new("/nonexistent-capture-before-context-fixture"),
                &[],
                Instant::now() + Duration::from_secs(1),
                || Err(failure("Injected context failure before spawn")),
            ).unwrap_err();
            assert!(error.to_string().contains("context failure before spawn"));
            let error = commands.run(
                Path::new("/nonexistent-capture-before-deadline-fixture"),
                &[],
                Instant::now(),
                || panic!("An expired command must not enter the context callback"),
            ).unwrap_err();
            assert!(error.to_string().contains("timed out"));
        }

        #[test]
        fn cancellation_and_timeout_kill_and_reap_only_the_owned_getter() {
            for cancelled in [false, true] {
                let temporary = tempfile::tempdir().unwrap();
                let pid_path = temporary.path().join("getter-pid");
                let checks = Cell::new(0);
                let commands = Commands::new().unwrap();
                let mut sentinel = Command::new("/bin/sleep").arg("60").spawn().unwrap();
                let started = Instant::now();
                let result = commands.run(
                    Path::new("/bin/sh"),
                    &["-c", "printf '%s' \"$$\" >\"$1\"; exec sleep 60", "sh", pid_path.to_str().unwrap()],
                    started + if cancelled { Duration::from_secs(1) } else { Duration::from_millis(150) },
                    || {
                        checks.set(checks.get() + 1);
                        if cancelled && checks.get() > 1 {
                            Err(failure("Injected active-command cancellation"))
                        } else {
                            Ok(())
                        }
                    },
                );
                let sentinel_live = sentinel.try_wait().unwrap().is_none();
                let _ = sentinel.kill();
                let _ = sentinel.wait();
                let error = result.unwrap_err();
                assert!(error.to_string().contains(if cancelled { "cancellation" } else { "timed out" }));
                assert!(started.elapsed() < Duration::from_secs(2));
                assert!(sentinel_live);
                let pid = fs::read_to_string(pid_path).unwrap().parse().unwrap();
                wait_for_reaping(pid);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Debug, Eq, PartialEq)]
    enum Read {
        Query(Query),
        Metadata,
    }

    struct Fixture {
        queries: VecDeque<(Query, String)>,
        metadata: VecDeque<PortraitMetadata>,
        reads: Vec<Read>,
        budgets: Vec<Duration>,
        elapsed: Duration,
        cost: Duration,
        cancel_after: Option<usize>,
        query_failure: Option<Query>,
    }

    impl Fixture {
        fn new(automatic: bool) -> Self {
            let mut queries = vec![(Query::Power, "active"), (Query::Mode, "Y8INV\n")];
            if automatic {
                queries.extend([(Query::Holding, "U\n"), (Query::Holding, "U")]);
            }
            queries.push((Query::Power, "active\n"));
            Self {
                queries: queries.into_iter().map(|(query, value)| (query, value.into())).collect(),
                metadata: vec![PortraitMetadata { automatic, rotation: 3 }; 2].into(),
                reads: Vec::new(),
                budgets: Vec::new(),
                elapsed: Duration::ZERO,
                cost: Duration::from_millis(5),
                cancel_after: None,
                query_failure: None,
            }
        }

        fn replace(&mut self, query: Query, index: usize, value: &str) {
            self.queries.iter_mut().filter(|(kind, _)| *kind == query)
                .nth(index).unwrap().1 = value.into();
        }
    }

    impl CaptureSource for Fixture {
        fn elapsed(&self) -> Duration { self.elapsed }

        fn check_context(&mut self) -> Result<(), PlatformError> {
            if self.cancel_after.is_some_and(|count| self.reads.len() >= count) {
                Err(failure("Injected native-read permission withdrawal"))
            } else {
                Ok(())
            }
        }

        fn query(&mut self, query: Query, remaining: Duration) -> Result<String, PlatformError> {
            self.reads.push(Read::Query(query));
            self.budgets.push(remaining);
            self.elapsed += self.cost;
            if self.query_failure == Some(query) {
                return Err(failure("Injected getter failure"));
            }
            let (expected, value) = self.queries.pop_front().unwrap();
            assert_eq!(query, expected);
            Ok(value)
        }

        fn metadata(&mut self, remaining: Duration) -> Result<PortraitMetadata, PlatformError> {
            self.reads.push(Read::Metadata);
            self.budgets.push(remaining);
            self.elapsed += self.cost;
            Ok(self.metadata.pop_front().unwrap())
        }
    }

    #[test]
    fn capture_preserves_the_original_sampling_order_and_one_deadline() {
        let mut fixture = Fixture::new(true);
        assert_eq!(capture_with(DeviceProfile::Scribe, &mut fixture).unwrap(), "ready:Y8INV:U:3");
        assert_eq!(fixture.reads, vec![
            Read::Query(Query::Power), Read::Query(Query::Mode), Read::Metadata,
            Read::Query(Query::Holding), Read::Metadata, Read::Query(Query::Holding),
            Read::Query(Query::Power),
        ]);
        assert!(fixture.budgets.windows(2).all(|pair| pair[0] > pair[1]));
    }

    #[test]
    fn fixed_devices_skip_holding_but_repeat_metadata_and_active_checks() {
        let mut fixture = Fixture::new(false);
        fixture.replace(Query::Mode, 0, "Y8");
        assert_eq!(capture_with(DeviceProfile::Paperwhite5, &mut fixture).unwrap(), "ready:Y8:FIXED:3");
        assert_eq!(fixture.reads, vec![
            Read::Query(Query::Power), Read::Query(Query::Mode), Read::Metadata,
            Read::Metadata, Read::Query(Query::Power),
        ]);
    }

    #[test]
    fn metadata_and_normalized_holding_must_remain_consistent() {
        for second in [
            PortraitMetadata { automatic: true, rotation: 1 },
            PortraitMetadata { automatic: false, rotation: 3 },
            PortraitMetadata { automatic: true, rotation: 4 },
        ] {
            let mut fixture = Fixture::new(true);
            fixture.metadata[1] = second;
            assert!(capture_with(DeviceProfile::Scribe, &mut fixture).is_err());
            assert_eq!(fixture.reads.len(), 5);
        }
        let mut fixture = Fixture::new(true);
        fixture.replace(Query::Holding, 1, "R");
        assert!(capture_with(DeviceProfile::Scribe, &mut fixture).is_err());
        assert_eq!(fixture.reads.len(), 6);
    }

    #[test]
    fn only_the_reviewed_scribe_profile_normalizes_lateral_values() {
        for (first, second, expected) in [("L", "U", "U"), ("D", "R\n", "D")] {
            let mut fixture = Fixture::new(true);
            fixture.replace(Query::Holding, 0, first);
            fixture.replace(Query::Holding, 1, second);
            assert_eq!(capture_with(DeviceProfile::Scribe, &mut fixture).unwrap(),
                format!("ready:Y8INV:{expected}:3"));
        }
        for profile in [DeviceProfile::Paperwhite5, DeviceProfile::Paperwhite6, DeviceProfile::Scribe2025] {
            for value in ["L", "R\n"] {
                let mut fixture = Fixture::new(true);
                fixture.replace(Query::Holding, 0, value);
                assert!(capture_with(profile, &mut fixture).is_err());
            }
        }
    }

    #[test]
    fn printable_non_active_power_is_distinct_from_malformed_power() {
        for value in ["screenSaver", "goingToScreenSaver\n", "pending dismissal", " "] {
            for index in 0..=1 {
                let mut fixture = Fixture::new(true);
                fixture.replace(Query::Power, index, value);
                assert_eq!(capture_with(DeviceProfile::Scribe, &mut fixture).unwrap(), "not-active");
                assert_eq!(fixture.reads.len(), if index == 0 { 1 } else { 7 });
            }
        }
    }

    #[test]
    fn command_records_reject_controls_trailing_garbage_unknown_values_and_overflow() {
        for value in ["", "\n", "active\r\n", "active\n\n", "active\0", "active\nextra", "\u{7f}", "\u{80}"] {
            let mut fixture = Fixture::new(true);
            fixture.replace(Query::Power, 0, value);
            assert!(capture_with(DeviceProfile::Scribe, &mut fixture).is_err(), "{value:?}");
            assert_eq!(fixture.reads.len(), 1);
        }
        for (query, values) in [
            (Query::Mode, vec!["Y8 extra", " Y8", "Y8\r\n", "Y8\n\n", "RGB", "Y8\0"]),
            (Query::Holding, vec!["U extra", " U", "U\r\n", "U\n\n", "U\nD", "D\0", ""]),
        ] {
            for value in values {
                let mut fixture = Fixture::new(true);
                fixture.replace(query, 0, value);
                assert!(capture_with(DeviceProfile::Scribe, &mut fixture).is_err(), "{value:?}");
            }
        }
        assert!(record_text(&"a".repeat(RECORD_LIMIT)).is_ok());
        assert!(record_text(&format!("{}\n", "a".repeat(RECORD_LIMIT - 1))).is_ok());
        for query in [Query::Power, Query::Mode, Query::Holding] {
            let mut fixture = Fixture::new(true);
            fixture.replace(query, 0, &"a".repeat(RECORD_LIMIT + 1));
            assert!(capture_with(DeviceProfile::Scribe, &mut fixture).is_err());
        }
    }

    #[test]
    fn context_failure_precedes_all_native_io_and_withdrawal_stops_the_transaction() {
        for count in 0..=7 {
            let mut fixture = Fixture::new(true);
            fixture.cancel_after = Some(count);
            assert!(capture_with(DeviceProfile::Scribe, &mut fixture).is_err());
            assert_eq!(fixture.reads.len(), count);
        }
    }

    #[test]
    fn an_expired_or_exhausted_total_deadline_never_admits_another_read() {
        let mut fixture = Fixture::new(true);
        fixture.elapsed = CAPTURE_TIMEOUT;
        assert!(capture_with(DeviceProfile::Scribe, &mut fixture).is_err());
        assert!(fixture.reads.is_empty());
        let mut fixture = Fixture::new(true);
        fixture.cost = Duration::from_millis(700);
        let error = capture_with(DeviceProfile::Scribe, &mut fixture).unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert_eq!(fixture.reads.len(), 3);
    }

    #[test]
    fn a_getter_error_is_preserved_without_retry_or_a_later_read() {
        for (query, reads) in [(Query::Power, 1), (Query::Mode, 2), (Query::Holding, 4)] {
            let mut fixture = Fixture::new(true);
            fixture.query_failure = Some(query);
            let error = capture_with(DeviceProfile::Scribe, &mut fixture).unwrap_err();
            assert!(error.to_string().contains("Injected getter failure"));
            assert_eq!(fixture.reads.len(), reads);
        }
    }
}

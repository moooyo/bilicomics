//! Bounded native helper commands with finite output and child ownership.

use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_IN_FLIGHT: usize = 8;
const MAX_OUTPUT_BYTES: usize = 4096;
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const REAPER_INTERVAL: Duration = Duration::from_millis(20);

static COMMAND_REAPER: OnceLock<io::Result<Arc<CommandReaper>>> = OnceLock::new();

struct CommandPermit {
    in_flight: Arc<AtomicUsize>,
}

impl Drop for CommandPermit {
    fn drop(&mut self) {
        self.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

struct PendingChild {
    child: Child,
    _permit: CommandPermit,
}

struct ReaperState {
    pending: Mutex<Vec<PendingChild>>,
    changed: Condvar,
    stopping: std::sync::atomic::AtomicBool,
}

struct CommandReaper {
    state: Arc<ReaperState>,
    in_flight: Arc<AtomicUsize>,
}

impl CommandReaper {
    fn new() -> io::Result<Arc<Self>> {
        Self::with_worker(|state| {
            let _worker = std::thread::Builder::new()
                .name("kindle-command-reaper".into())
                .spawn(move || reap_pending_commands(state))?;
            Ok(())
        })
    }

    fn with_worker(start: impl FnOnce(Arc<ReaperState>) -> io::Result<()>) -> io::Result<Arc<Self>> {
        let state = Arc::new(ReaperState {
            pending: Mutex::new(Vec::with_capacity(MAX_IN_FLIGHT)),
            changed: Condvar::new(),
            stopping: std::sync::atomic::AtomicBool::new(false),
        });
        start(state.clone())?;
        Ok(Arc::new(Self { state, in_flight: Arc::new(AtomicUsize::new(0)) }))
    }

    fn acquire(&self) -> io::Result<CommandPermit> {
        self.in_flight.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            if current < MAX_IN_FLIGHT { Some(current + 1) } else { None }
        }).map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "Native command capacity is exhausted"))?;
        Ok(CommandPermit { in_flight: self.in_flight.clone() })
    }

    fn defer(&self, child: PendingChild) {
        let mut pending = self.state.pending.lock().unwrap_or_else(|error| error.into_inner());
        // Every live or deferred child owns one of the same eight permits.
        // This child still owns its permit, so at most seven other entries
        // exist and the preallocated container cannot grow beyond its bound.
        pending.push(child);
        drop(pending);
        self.state.changed.notify_one();
    }
}

impl Drop for CommandReaper {
    fn drop(&mut self) {
        // The global instance lives for the process lifetime. Private test
        // instances let their worker exit after already killed tasks reap.
        let _pending = self.state.pending.lock().unwrap_or_else(|error| error.into_inner());
        self.state.stopping.store(true, Ordering::Release);
        self.state.changed.notify_one();
    }
}

fn reap_pending_commands(state: Arc<ReaperState>) {
    let mut pending = state.pending.lock().unwrap_or_else(|error| error.into_inner());
    loop {
        let mut index = 0;
        while index < pending.len() {
            let finished = match pending[index].child.try_wait() {
                Ok(Some(_)) => true,
                Err(error) if error.raw_os_error() == Some(libc::ECHILD) => true,
                // Keep both the handle and permit on a transient wait error.
                // No numeric PID or process-group signal is used by this worker.
                Ok(None) | Err(_) => false,
            };
            if finished { drop(pending.swap_remove(index)); } else { index += 1; }
        }
        if pending.is_empty() {
            if state.stopping.load(Ordering::Acquire) { return; }
            pending = state.changed.wait(pending).unwrap_or_else(|error| error.into_inner());
        } else {
            let waited = state.changed.wait_timeout(pending, REAPER_INTERVAL)
                .unwrap_or_else(|error| error.into_inner());
            pending = waited.0;
        }
    }
}

fn command_reaper() -> io::Result<Arc<CommandReaper>> {
    match COMMAND_REAPER.get_or_init(CommandReaper::new) {
        Ok(reaper) => Ok(reaper.clone()),
        Err(_) => Err(io::Error::other("Native command reaper is unavailable")),
    }
}

/// Observe without reaping, reserving the leader's PID and private group ID.
fn child_exited(child: &Child) -> io::Result<bool> {
    // SAFETY: waitid writes a siginfo_t into the aligned live buffer. The
    // owned child's PID is supplied, and WNOWAIT preserves its identity.
    let mut information: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            &mut information,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result == -1 { return Err(io::Error::last_os_error()); }
    // POSIX waitid reports SIGCHLD for an available child and zeroes this
    // header field when WNOHANG finds none. Unlike the PID union accessor,
    // si_signo has the same public layout on Linux and macOS.
    Ok(information.si_signo == libc::SIGCHLD)
}

struct CommandChild {
    child: Option<Child>,
    process_group: libc::pid_t,
    permit: Option<CommandPermit>,
    reaper: Arc<CommandReaper>,
}

impl CommandChild {
    fn spawn(reaper: Arc<CommandReaper>, program: &Path, arguments: &[&str]) -> io::Result<Self> {
        let permit = reaper.acquire()?;
        let child = Command::new(program)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        // Linux PIDs are positive pid_t values. process_group(0) puts the
        // child into a new private group before executing the command.
        let process_group = child.id() as libc::pid_t;
        Ok(Self { child: Some(child), process_group, permit: Some(permit), reaper })
    }

    fn exited(&self) -> io::Result<bool> {
        child_exited(self.child.as_ref().ok_or_else(|| io::Error::other("Native command was already reaped"))?)
    }

    fn finish_exited(&mut self) -> io::Result<ExitStatus> {
        let child = self.child.as_mut().ok_or_else(|| io::Error::other("Native command was already reaped"))?;
        if !child_exited(child)? {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "Native command is not waitable"));
        }
        signal_owned_group(child, self.process_group);
        match child.try_wait()? {
            Some(status) => {
                self.child.take();
                // Retain admission until output collection also finishes.
                Ok(status)
            }
            None => Err(io::Error::new(io::ErrorKind::WouldBlock, "Native command exit is not reapable")),
        }
    }
}

fn signal_owned_group(child: &mut Child, process_group: libc::pid_t) {
    // SAFETY: the exclusively owned, unreaped child reserves this private
    // group ID. Group cleanup must precede every try_wait that may reap it.
    unsafe { libc::kill(-process_group, libc::SIGKILL) };
    // Also handle a command that changed its own group after exec. The child
    // remains unreaped, so its PID cannot identify an unrelated process.
    let _ = child.kill();
}

impl Drop for CommandChild {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else { return; };
        let Some(permit) = self.permit.take() else { return; };
        child.stdout.take();
        if child_exited(&child).is_err_and(|error| error.raw_os_error() == Some(libc::ECHILD)) {
            // An external reaper has released this identity; do not signal
            // the old numeric group ID after it could have been reused.
            return;
        }
        signal_owned_group(&mut child, self.process_group);
        match child.try_wait() {
            Ok(Some(_)) => {}
            Err(error) if error.raw_os_error() == Some(libc::ECHILD) => {}
            // Never wait synchronously for a killed task that is not yet
            // reapable. Its permit is retained by the single bounded worker.
            Ok(None) | Err(_) => self.reaper.defer(PendingChild { child, _permit: permit }),
        }
    }
}

fn set_nonblocking(descriptor: libc::c_int) -> io::Result<()> {
    // SAFETY: fcntl reads and updates this owned pipe's descriptor flags; it
    // does not retain a pointer or change another process's descriptor.
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn timeout_error() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "Native command timed out")
}

fn collect_command_output(mut child: CommandChild, deadline: Instant, output_limit: usize) -> io::Result<String> {
    let mut stdout = child.child.as_mut().and_then(|process| process.stdout.take())
        .ok_or_else(|| io::Error::other("Native command stdout pipe is unavailable"))?;
    set_nonblocking(stdout.as_raw_fd())?;
    let mut output = Vec::with_capacity(output_limit.min(MAX_OUTPUT_BYTES));
    let mut buffer = [0; 1024];
    let mut stdout_closed = false;
    let mut command_exited = false;

    loop {
        if Instant::now() >= deadline { return Err(timeout_error()); }
        while !stdout_closed {
            // Continuous output or repeated EINTR must not bypass the same
            // deadline used for child observation and poll.
            if Instant::now() >= deadline { return Err(timeout_error()); }
            let remaining = output_limit - output.len();
            let read_limit = remaining.saturating_add(1).min(buffer.len());
            match stdout.read(&mut buffer[..read_limit]) {
                Ok(0) => { stdout_closed = true; break; }
                Ok(count) => {
                    if count > remaining {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "Native command output exceeded its byte limit"));
                    }
                    output.extend_from_slice(&buffer[..count]);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        if !command_exited {
            match child.exited() {
                Ok(true) => {
                    // The leader is waitable. Kill private-group descendants
                    // that inherited stdout before releasing that identity.
                    let status = child.finish_exited()?;
                    if !status.success() { return Err(io::Error::other("Native command was rejected")); }
                    command_exited = true;
                }
                Ok(false) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        if stdout_closed && command_exited {
            let text = String::from_utf8(output)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Native command output is not valid UTF-8"))?;
            if Instant::now() >= deadline { return Err(timeout_error()); }
            return Ok(text);
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() { return Err(timeout_error()); }
        let poll_timeout = remaining.min(POLL_INTERVAL).as_millis().max(1) as libc::c_int;
        let mut descriptor = libc::pollfd {
            fd: if stdout_closed { -1 } else { stdout.as_raw_fd() },
            events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
            revents: 0,
        };
        // SAFETY: poll borrows one initialized descriptor for this call.
        // A negative descriptor is ignored while only child exit is pending.
        let result = unsafe { libc::poll(&mut descriptor, 1, poll_timeout) };
        if result == -1 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted { return Err(error); }
        } else if descriptor.revents & libc::POLLNVAL != 0 {
            return Err(io::Error::other("Native command stdout descriptor became invalid"));
        }
    }
}

/// Run a native helper with one deadline for spawn, output and child exit.
///
/// Output must be UTF-8 and may contain at most `output_limit` bytes, with a
/// maximum configurable limit of 4096 bytes. No reader thread is created.
/// At most eight commands, including killed children awaiting reaping, may
/// be in flight. Exhausted capacity rejects a command before it is spawned.
/// Cleanup signals only the private command group and its unreaped leader;
/// descendants that deliberately leave that group are not broadly signalled.
pub fn run_bounded_command(program: &Path, arguments: &[&str], timeout: Duration, output_limit: usize) -> io::Result<String> {
    let started = Instant::now();
    let deadline = started.checked_add(timeout)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Native command timeout is too large"))?;
    if output_limit > MAX_OUTPUT_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Native command output limit exceeds 4096 bytes"));
    }
    if Instant::now() >= deadline { return Err(timeout_error()); }
    let reaper = command_reaper()?;
    if Instant::now() >= deadline { return Err(timeout_error()); }
    let child = CommandChild::spawn(reaper, program, arguments)?;
    collect_command_output(child, deadline, output_limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DeferredFixtureCleanup {
        reaper: Arc<CommandReaper>,
        pid: u32,
    }

    impl Drop for DeferredFixtureCleanup {
        fn drop(&mut self) {
            let mut pending = self.reaper.state.pending.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(pending) = pending.iter_mut().find(|value| value.child.id() == self.pid) {
                // The lock keeps this owned child unreaped until its private
                // group is signalled, including an earlier assertion unwind.
                signal_owned_group(&mut pending.child, self.pid as libc::pid_t);
            }
        }
    }

    fn spawn_fixture(reaper: &Arc<CommandReaper>, script: &str) -> CommandChild {
        CommandChild::spawn(reaper.clone(), Path::new("/bin/sh"), &["-c", script]).unwrap()
    }

    fn run_fixture(
        reaper: &Arc<CommandReaper>,
        script: &str,
        timeout: Duration,
        output_limit: usize,
    ) -> (u32, io::Result<String>) {
        let deadline = Instant::now() + timeout;
        let child = spawn_fixture(reaper, script);
        let pid = child.child.as_ref().unwrap().id();
        (pid, collect_command_output(child, deadline, output_limit))
    }

    fn wait_for_in_flight(reaper: &CommandReaper, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while reaper.in_flight.load(Ordering::Acquire) != expected && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(reaper.in_flight.load(Ordering::Acquire), expected);
    }

    fn assert_child_reaped(pid: u32) {
        let mut status = 0;
        // SAFETY: WNOHANG never blocks. The completed fixture must no longer
        // be a waitable child of this test process.
        let result = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
        assert_eq!(result, -1, "The fixture was not reaped by its owner");
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ECHILD));
    }

    fn wait_for_descendant_exit(pid: u32) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let state = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()
                .and_then(|value| value.rsplit_once(") ").map(|(_, fields)| fields.to_owned()))
                .and_then(|fields| fields.chars().next());
            if state.is_none() || matches!(state, Some('Z' | 'X')) { return; }
            assert!(Instant::now() < deadline, "The inherited-stdout descendant is still running");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn bounded_command_times_out_and_reaps_its_child() {
        let reaper = CommandReaper::new().unwrap();
        let started = Instant::now();
        let (pid, result) = run_fixture(&reaper, "exec sleep 60", Duration::from_millis(150), 64);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
        wait_for_in_flight(&reaper, 0);
        assert_child_reaped(pid);
    }

    #[test]
    fn bounded_command_eof_does_not_skip_the_exit_deadline() {
        let reaper = CommandReaper::new().unwrap();
        let (pid, result) = run_fixture(
            &reaper, "exec 1>&-; exec sleep 60", Duration::from_millis(150), 64,
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        wait_for_in_flight(&reaper, 0);
        assert_child_reaped(pid);
    }

    #[test]
    fn bounded_command_cleans_up_an_inherited_stdout_descendant() {
        let reaper = CommandReaper::new().unwrap();
        let (pid, result) = run_fixture(
            &reaper,
            "sleep 60 & printf '%s\\n' \"$!\"; printf '7\\n'; exit 0",
            Duration::from_secs(1),
            64,
        );
        let output = result.unwrap();
        let mut lines = output.lines();
        let descendant = lines.next().unwrap().parse::<u32>().unwrap();
        assert_eq!(lines.next(), Some("7"));
        assert_eq!(lines.next(), None);
        wait_for_in_flight(&reaper, 0);
        assert_child_reaped(pid);
        wait_for_descendant_exit(descendant);
    }

    #[test]
    fn bounded_command_accepts_exactly_the_output_limit() {
        let reaper = CommandReaper::new().unwrap();
        let (pid, result) = run_fixture(
            &reaper, "printf '%4096s' ''", Duration::from_secs(1), MAX_OUTPUT_BYTES,
        );
        assert_eq!(result.unwrap().len(), MAX_OUTPUT_BYTES);
        wait_for_in_flight(&reaper, 0);
        assert_child_reaped(pid);
    }

    #[test]
    fn bounded_command_rejects_excess_output_without_waiting_for_exit() {
        let reaper = CommandReaper::new().unwrap();
        let (pid, result) = run_fixture(
            &reaper, "while :; do printf '1234567890'; done", Duration::from_secs(1), MAX_OUTPUT_BYTES,
        );
        let error = result.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exceeded"));
        wait_for_in_flight(&reaper, 0);
        assert_child_reaped(pid);
    }

    #[test]
    fn bounded_command_rejects_invalid_utf8_and_reaps() {
        let reaper = CommandReaper::new().unwrap();
        let (pid, result) = run_fixture(&reaper, r"printf '\377'", Duration::from_secs(1), 64);
        let error = result.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("UTF-8"));
        wait_for_in_flight(&reaper, 0);
        assert_child_reaped(pid);
    }

    #[test]
    fn bounded_command_rejects_nonzero_exit_and_reaps() {
        let reaper = CommandReaper::new().unwrap();
        let (pid, result) = run_fixture(&reaper, "exit 7", Duration::from_secs(1), 64);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Other);
        wait_for_in_flight(&reaper, 0);
        assert_child_reaped(pid);
    }

    #[test]
    fn bounded_command_setup_error_reaps_the_guarded_child() {
        fn fail_after_spawn(child: CommandChild) -> io::Result<()> {
            let _child = child;
            set_nonblocking(-1)
        }
        let reaper = CommandReaper::new().unwrap();
        let child = spawn_fixture(&reaper, "exec sleep 60");
        let pid = child.child.as_ref().unwrap().id();
        assert_eq!(fail_after_spawn(child).unwrap_err().raw_os_error(), Some(libc::EBADF));
        wait_for_in_flight(&reaper, 0);
        assert_child_reaped(pid);
    }

    #[test]
    fn bounded_command_unwind_reaps_the_guarded_child() {
        let reaper = CommandReaper::new().unwrap();
        let child = spawn_fixture(&reaper, "exec sleep 60");
        let pid = child.child.as_ref().unwrap().id();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _child = child;
            panic!("Controlled command fixture unwind");
        }));
        assert!(result.is_err());
        wait_for_in_flight(&reaper, 0);
        assert_child_reaped(pid);
    }

    #[test]
    fn bounded_command_does_not_signal_an_unrelated_group() {
        let reaper = CommandReaper::new().unwrap();
        let sentinel = spawn_fixture(&reaper, "exec sleep 60");
        let sentinel_pid = sentinel.child.as_ref().unwrap().id();
        let (pid, result) = run_fixture(&reaper, "exec sleep 60", Duration::from_millis(150), 64);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        wait_for_in_flight(&reaper, 1);
        assert_child_reaped(pid);
        assert!(!sentinel.exited().unwrap());
        drop(sentinel);
        wait_for_in_flight(&reaper, 0);
        assert_child_reaped(sentinel_pid);
    }

    #[test]
    fn bounded_command_keeps_deferred_children_inside_the_capacity_limit() {
        let reaper = CommandReaper::new().unwrap();
        let reserved = (0..MAX_IN_FLIGHT - 1).map(|_| reaper.acquire().unwrap()).collect::<Vec<_>>();
        let mut guarded = spawn_fixture(&reaper, "exec sleep 60");
        let pid = guarded.child.as_ref().unwrap().id();
        let child = guarded.child.take().unwrap();
        let permit = guarded.permit.take().unwrap();
        reaper.defer(PendingChild { child, _permit: permit });
        let _cleanup = DeferredFixtureCleanup { reaper: reaper.clone(), pid };
        drop(guarded);

        assert_eq!(reaper.in_flight.load(Ordering::Acquire), MAX_IN_FLIGHT);
        assert_eq!(reaper.acquire().err().unwrap().kind(), io::ErrorKind::WouldBlock);
        let spawn_error = CommandChild::spawn(
            reaper.clone(), Path::new("/nonexistent-command-capacity-fixture"), &[],
        ).err().unwrap();
        assert_eq!(spawn_error.kind(), io::ErrorKind::WouldBlock);

        // Holding the pending lock preserves the child's waitable identity
        // until its private group has been signalled by this test owner.
        {
            let mut pending = reaper.state.pending.lock().unwrap();
            let pending = pending.iter_mut().find(|value| value.child.id() == pid).unwrap();
            signal_owned_group(&mut pending.child, pid as libc::pid_t);
        }
        wait_for_in_flight(&reaper, MAX_IN_FLIGHT - 1);
        assert_child_reaped(pid);
        let replacement = reaper.acquire().unwrap();
        assert_eq!(reaper.in_flight.load(Ordering::Acquire), MAX_IN_FLIGHT);
        drop(replacement);
        drop(reserved);
        wait_for_in_flight(&reaper, 0);
    }

    #[test]
    fn bounded_command_spawn_failure_releases_capacity() {
        let reaper = CommandReaper::new().unwrap();
        let error = CommandChild::spawn(
            reaper.clone(), Path::new("/nonexistent-command-spawn-fixture"), &[],
        ).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        wait_for_in_flight(&reaper, 0);
    }

    #[test]
    fn bounded_command_worker_initialization_failure_is_returned() {
        let error = CommandReaper::with_worker(|_| {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "Controlled worker initialization failure"))
        }).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn bounded_command_rejects_zero_timeout_before_spawn() {
        let error = run_bounded_command(
            Path::new("/nonexistent-command-deadline-fixture"), &[], Duration::ZERO, 64,
        ).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn bounded_command_rejects_unbounded_output_configuration_before_spawn() {
        let error = run_bounded_command(
            Path::new("/nonexistent-command-limit-fixture"), &[], Duration::from_secs(1), MAX_OUTPUT_BYTES + 1,
        ).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}

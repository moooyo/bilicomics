//! Linux primitives used by the single-transaction startup reader only.

use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::{self, Read},
    os::fd::{AsRawFd, FromRawFd, RawFd},
    time::{Duration, Instant},
};

pub(super) const SECOND: u64 = 1_000_000_000;
pub(super) const CENTISECOND: u64 = SECOND / 100;
const POLL: Duration = Duration::from_millis(10);

pub(super) fn clock(clock_id: libc::clockid_t) -> Result<u64> {
    let mut stamp = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: the writable timespec outlives the call.
    syscall(unsafe { libc::clock_gettime(clock_id, &mut stamp) })?;
    let seconds = u64::try_from(stamp.tv_sec)?;
    let nanos = u64::try_from(stamp.tv_nsec)?;
    seconds.checked_mul(SECOND).and_then(|s| s.checked_add(nanos)).context("Startup clock overflow")
}

pub(super) fn now() -> Result<u64> {
    // Linux /proc/uptime and the in-group watchdog share this monotone clock.
    // A guest suspend must not extend the permitted sampling window.
    clock(libc::CLOCK_BOOTTIME)
}

pub(super) fn remaining(until: u64) -> Result<Duration> {
    let left = until.checked_sub(now()?).filter(|left| *left > 0).context("Startup reader deadline elapsed")?;
    Ok(Duration::from_nanos(left))
}

pub(super) fn local_deadline(until: u64) -> Result<Instant> {
    // Start from an earlier observation; conversion must not extend the limit.
    let instant = Instant::now();
    instant.checked_add(remaining(until)?).context("Startup reader deadline overflow")
}

pub(super) fn pause(until: u64) -> Result<()> {
    std::thread::sleep(remaining(until)?.min(POLL));
    Ok(())
}

pub(super) fn syscall(ret: libc::c_int) -> io::Result<libc::c_int> {
    if ret == -1 { Err(io::Error::last_os_error()) } else { Ok(ret) }
}

pub(super) fn pipe() -> Result<(File, File)> {
    let mut ends = [-1; 2];
    // SAFETY: pipe2 returns two unique owned descriptors in this array.
    unsafe {
        syscall(libc::pipe2(ends.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK))?;
        Ok((File::from_raw_fd(ends[0]), File::from_raw_fd(ends[1])))
    }
}

pub(super) fn duplicate(fd: RawFd, from: RawFd) -> Result<File> {
    // SAFETY: F_DUPFD_CLOEXEC creates a new owned descriptor on success.
    let duplicate = syscall(unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, from) })?;
    Ok(unsafe { File::from_raw_fd(duplicate) })
}

pub(super) fn at_descriptor(file: File, fd: RawFd) -> Result<File> {
    let next = duplicate(file.as_raw_fd(), fd)?;
    ensure!(next.as_raw_fd() == fd, "A startup handoff descriptor is already in use");
    Ok(next)
}

pub(super) fn nonblocking(fd: RawFd) -> Result<()> {
    // SAFETY: only the file status flags of the borrowed descriptor are changed.
    let flags = syscall(unsafe { libc::fcntl(fd, libc::F_GETFL) })?;
    syscall(unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) })?;
    Ok(())
}

/// Read at most one byte beyond the wire limit; true requires actual EOF.
pub(super) fn drain(mut file: &File, bytes: &mut Vec<u8>, limit: usize) -> Result<bool> {
    let mut buffer = [0; 641];
    ensure!(limit < buffer.len(), "Invalid startup reader buffer limit");
    loop {
        let allowance = limit - bytes.len();
        match file.read(&mut buffer[..allowance + 1]) {
            Ok(0) => return Ok(true),
            Ok(n) if n <= allowance => bytes.extend_from_slice(&buffer[..n]),
            Ok(_) => anyhow::bail!("Startup reader output overflow"),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) => return Err(error.into()),
        }
    }
}

/// All callers use a fresh pipe and a frame smaller than PIPE_BUF.
pub(super) fn write_frame(fd: RawFd, bytes: &[u8]) -> Result<()> {
    ensure!(!bytes.is_empty() && bytes.len() <= 640, "Invalid startup reader frame length");
    // SAFETY: the buffer is readable for its full length during the call.
    let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
    if written < 0 { return Err(io::Error::last_os_error().into()); }
    ensure!(written as usize == bytes.len(), "Short startup reader frame");
    Ok(())
}

pub(super) fn become_subreaper() -> Result<()> {
    // SAFETY: this single-purpose process sets its own disposition before
    // spawning. SIG_IGN inherited from a caller could otherwise auto-reap L.
    ensure!(unsafe { libc::signal(libc::SIGCHLD, libc::SIG_DFL) } != libc::SIG_ERR,
        "Cannot retain the startup child wait status");
    // SAFETY: PR_SET_CHILD_SUBREAPER takes an integer value and no pointers.
    syscall(unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1 as libc::c_ulong,
        0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong) })?;
    Ok(())
}

/// The exact direct child remains waitable and its PID remains reserved.
pub(super) fn observe_child(pid: libc::pid_t) -> Result<Option<bool>> {
    // SAFETY: waitid initializes the aligned live buffer. WNOWAIT does not reap.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    syscall(unsafe {
        libc::waitid(libc::P_PID, pid as libc::id_t, &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT)
    })?;
    if info.si_signo == 0 { return Ok(None); }
    ensure!(info.si_signo == libc::SIGCHLD && unsafe { info.si_pid() } == pid,
        "Unexpected startup reader wait identity");
    // SAFETY: SIGCHLD made the child-specific member of siginfo_t valid.
    Ok(Some(info.si_code == libc::CLD_EXITED && unsafe { info.si_status() } == 0))
}

pub(super) fn stop_owned_child(pid: libc::pid_t, group_confirmed: bool) -> Result<()> {
    // We are the exact child's sole waiter and have not reaped it. Use its
    // private group only if its setsid identity was positively established.
    let exited = observe_child(pid)?.is_some();
    if group_confirmed || !exited {
        let target = if group_confirmed { -pid } else { pid };
        // SAFETY: the waitable child retains the ID, and any group signal is
        // restricted to the previously confirmed original private group.
        no_process_or_success(unsafe { libc::kill(target, libc::SIGKILL) })?;
    }
    Ok(())
}

fn no_process_or_success(ret: libc::c_int) -> Result<bool> {
    match syscall(ret) {
        Ok(_) => Ok(false),
        Err(err) if err.raw_os_error() == Some(libc::ESRCH) => Ok(true),
        Err(err) => Err(err.into()),
    }
}

pub(super) fn process_absent(pid: libc::pid_t) -> Result<bool> {
    // SAFETY: signal zero only checks existence; it cannot affect the target.
    no_process_or_success(unsafe { libc::kill(pid, 0) })
}

fn wait_nohang(pid: libc::pid_t) -> io::Result<libc::pid_t> {
    // SAFETY: with a null status pointer this only updates the caller's child state.
    syscall(unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) })
}

pub(super) fn retire(leader: Option<libc::pid_t>, until: u64) -> Result<()> {
    let mut unreaped = leader;
    for _ in 0..50 {
        remaining(until)?;
        if let Some(pid) = unreaped {
            match wait_nohang(pid)? {
                0 => {},
                got if got == pid => unreaped = None,
                _ => anyhow::bail!("Unexpected startup reader retirement identity"),
            }
        }
        if unreaped.is_none() {
            // Exclusive subreaper: there are no unrelated jobs in this subtree.
            // Do not use waitpid(-1) before the exact original leader is reaped.
            for _ in 0..32 {
                match wait_nohang(-1) {
                    Ok(0) => break,
                    Ok(_) => {},
                    Err(err) if err.raw_os_error() == Some(libc::ECHILD) => {
                        let group_gone = match leader {
                            Some(pid) => process_absent(-pid)?,
                            None => true,
                        };
                        if group_gone {
                            remaining(until)?;
                            return Ok(());
                        }
                        break;
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        }
        std::thread::sleep(remaining(until)?.min(Duration::from_millis(20)));
    }
    anyhow::bail!("Startup reader subtree did not retire")
}

/// Start an independent, armed, in-group watchdog. This role is single-threaded.
/// It inherits neither shell handoff pipe and drops the leader's gate and stdout.
pub(super) fn arm_watchdog(group: libc::pid_t, directory: RawFd, until: u64) -> Result<()> {
    let stamp = libc::timespec {
        tv_sec: (until / SECOND).try_into()?,
        #[allow(clippy::useless_conversion)]
        tv_nsec: (until % SECOND).try_into()?,
    };
    let (armed, signal) = pipe()?;
    let ready = signal.as_raw_fd();
    let read_end = armed.as_raw_fd();
    // SAFETY: the forked role calls only libc functions, no Rust runtime or allocation,
    // and exits with _exit. It acts only on the process group to which it belongs.
    unsafe {
        let pid = syscall(libc::fork())?;
        if pid == 0 {
            libc::close(read_end);
            libc::close(directory);
            libc::close(0);
            libc::close(1);
            libc::close(2);
            if libc::getpgrp() != group || libc::getsid(0) != group { libc::_exit(1); }
            // Losing the readiness listener must not disable the watchdog.
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
            let _ = libc::write(ready, b"A".as_ptr().cast(), 1);
            libc::close(ready);
            while libc::clock_nanosleep(libc::CLOCK_BOOTTIME, libc::TIMER_ABSTIME, &stamp,
                                        std::ptr::null_mut()) == libc::EINTR {}
            if libc::getpgrp() == group && libc::getsid(0) == group {
                libc::kill(0, libc::SIGKILL);
            }
            libc::_exit(1);
        }
    }
    drop(signal);
    let mut frame = Vec::with_capacity(1);
    loop {
        remaining(until)?;
        if drain(&armed, &mut frame, 1)? {
            ensure!(frame == b"A", "Startup watchdog did not arm");
            remaining(until)?;
            return Ok(());
        }
        pause(until)?;
    }
}

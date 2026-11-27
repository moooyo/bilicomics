//! A bounded, startup-only snapshot handoff. The shell still owns the launch lock.
//!
//! M owns only the control writer and result reader. O owns the opposite
//! anonymous pipe ends, and is the only waiter for its setsid child L and
//! adopted descendants. L acknowledges its independent in-group watchdog,
//! then starts and waits for the real sampler. Neither the sampler nor the
//! watchdog can cancel the transaction or publish its terminal result.

mod kernel;

use self::kernel::*;
use super::{
    ProcessIdentity, Writer, MAX_STATUS_BYTES, parse_pid, private_regular,
    same_identity, stable_file,
    startup_permit::{Member, Scope, StartupReadBinding, check_startup_base, sample},
};
use crate::platform::DeviceProfile;
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::{self, File, Metadata},
    io::{self, Read, Write},
    os::{fd::{AsRawFd, FromRawFd, RawFd}, unix::{fs::{FileTypeExt, MetadataExt, OpenOptionsExt}, process::CommandExt}},
    path::PathBuf,
    process::{Child, Command, ExitCode, Stdio},
};

const FLAG: &str = "--native-startup-snapshot";
const TAG: &str = "startup-display-v1";
const GRANT: &str = "native-startup-grant";
const CONTROL: RawFd = 64;
const RESULT: RawFd = 65;
const SHELL_CONTROL: RawFd = 7;
const SHELL_RESULT: RawFd = 8;
const OWNER_PROC: RawFd = 9;
const BODY_LIMIT: usize = 128;
const CLEAN_REFUSAL: u8 = 21;
const CLEAN_CANCEL: u8 = 22;
const UNPROVEN: u8 = 23;

pub(in crate::native_presentation) fn entry() -> ExitCode {
    ExitCode::from(dispatch().unwrap_or(UNPROVEN))
}

fn dispatch() -> Result<u8> {
    let args = std::env::args_os().skip(1).map(|arg| arg.into_string().map_err(|_| anyhow::anyhow!("Invalid startup reader argument")))
        .collect::<Result<Vec<_>>>()?;
    let [flag, role, profile, master, birth, seq, begin, extra @ ..] = args.as_slice() else {
        bail!("Incomplete startup reader arguments");
    };
    ensure!(flag == FLAG, "Invalid startup reader mode");
    let profile = DeviceProfile::parse(profile).filter(|value| value.id() == profile)
        .context("Unknown startup reader profile")?;
    crate::platform::validate_native_launch_approval(profile)?;
    let scope = Scope {
        master: parse_member(master, birth)?,
        sequence: decimal(seq)?,
        begin_cs: decimal(begin)?,
    };
    ensure!(scope.sequence > 0, "Invalid startup reader sequence");
    let writer = Writer::from_environment()?;
    match (role.as_str(), extra) {
        ("owner", []) => owner(&writer, profile, scope),
        ("receive", [pid, birth]) => {
            receive(&writer, scope, parse_member(pid, birth)?)?;
            Ok(0)
        }
        ("leader", [pid, birth, cutoff]) => {
            leader(&writer, profile, scope, parse_member(pid, birth)?, decimal(cutoff)?)?;
            Ok(0)
        }
        ("sample", [owner, owner_birth, cutoff, pid, birth, directory_dev, directory_ino, dev, ino]) => {
            let cutoff_ns = decimal(cutoff)?;
            ensure!(cutoff_ns == capture_end(scope)?, "Startup sampler cutoff changed");
            let binding = StartupReadBinding {
                scope,
                owner: parse_member(owner, owner_birth)?,
                leader: parse_member(pid, birth)?,
                cutoff_ns,
                directory_dev: decimal(directory_dev)?,
                directory_ino: decimal(directory_ino)?,
                dev: decimal(dev)?,
                ino: decimal(ino)?,
            };
            let record = sample(profile, binding, local_deadline(cutoff_ns)?)?;
            let body = format!("{record}\n");
            remaining(cutoff_ns)?;
            write_frame(libc::STDOUT_FILENO, body.as_bytes())?;
            Ok(0)
        }
        _ => bail!("Invalid startup reader role or argument count"),
    }
}

fn decimal(raw: &str) -> Result<u64> {
    let value = raw.parse::<u64>()?;
    ensure!(raw == value.to_string(), "Noncanonical startup reader number");
    Ok(value)
}

fn parse_member(pid: &str, birth: &str) -> Result<Member> {
    let number = parse_pid(pid)?;
    ensure!(number <= libc::pid_t::MAX as u32 && pid == number.to_string(), "Noncanonical startup reader PID");
    Ok(Member { pid: number, birth: decimal(birth)? })
}

fn member(identity: &ProcessIdentity) -> Member {
    Member { pid: identity.pid, birth: identity.birth }
}

fn operation(writer: &Writer, scope: Scope) -> String {
    format!("{TAG}:{}:{}:{}:{}:{}", writer.header(), scope.sequence,
        scope.master.pid, scope.master.birth, scope.begin_cs)
}

fn boot_start(scope: Scope) -> Result<u64> {
    scope.begin_cs.checked_mul(CENTISECOND).context("Invalid startup begin time")
}

fn capture_end(scope: Scope) -> Result<u64> {
    // Include attachment and authorization in one fixed two-second window.
    boot_start(scope)?.checked_add(2 * SECOND).context("Startup capture deadline overflow")
}

fn fresh(scope: Scope) -> Result<()> {
    let start = boot_start(scope)?;
    ensure!(now()?.checked_sub(start).is_some_and(|ns| ns < 3 * SECOND), "Startup receipt expired");
    Ok(())
}

fn same_image(pid: u32) -> Result<bool> {
    Ok(same_identity(&fs::metadata("/proc/self/exe")?, &fs::metadata(format!("/proc/{pid}/exe"))?))
}

fn owner_identity(writer: &Writer, scope: Scope, pid: u32) -> Result<ProcessIdentity> {
    let master = check_startup_base(writer, scope)?;
    let child = writer.process(pid)?;
    ensure!(child.parent == master.pid && child.group == master.group && child.session == master.session
        && same_image(pid)?, "Invalid startup owner process");
    Ok(child)
}

fn leader_identity(writer: &Writer, owner: Member, pid: u32) -> Result<ProcessIdentity> {
    let leader = writer.process(pid)?;
    ensure!(leader.parent == owner.pid && leader.group == pid && leader.session == pid && same_image(pid)?,
        "Invalid startup reader process");
    Ok(leader)
}

struct Channels {
    control: File,
    result: File,
}

impl Channels {
    fn open() -> Result<Self> {
        let (input, peer) = pipe()?;
        drop(peer);
        let (peer, output) = pipe()?;
        drop(peer);
        // Publish only kernel pipes through the live exact child's proc fd
        // directory. No named FIFO, duplicate shell peer, or result pathname.
        let control = at_descriptor(input, CONTROL)?;
        let result = at_descriptor(output, RESULT)?;
        Ok(Self { control, result })
    }

    fn start(&self, writer: &Writer, scope: Scope, until: u64) -> Result<bool> {
        let expected = format!("{}\n", operation(writer, scope));
        let mut frame = Vec::with_capacity(expected.len());
        loop {
            remaining(until)?;
            if writer.process(scope.master.pid).map(|p| member(&p)).ok() != Some(scope.master) {
                return Ok(false);
            }
            // M installs its writer before its result reader. Check attachment
            // first: EOF before the first peer opens is not cancellation.
            if peer_pipe(&self.result, scope.master.pid, SHELL_RESULT, libc::O_RDONLY)? {
                if drain(&self.control, &mut frame, expected.len())? {
                    return Ok(false);
                }
                if frame.len() == expected.len() {
                    ensure!(frame == expected.as_bytes(), "Invalid startup authorization frame");
                    remaining(until)?;
                    return self.active(scope);
                }
            }
            pause(until)?;
        }
    }

    fn active(&self, scope: Scope) -> Result<bool> {
        // After start only silence or EOF is valid. The exact launcher must
        // still hold its original writer, even if another reader leaked a copy.
        Ok(!drain(&self.control, &mut Vec::new(), 0)?
            && peer_pipe(&self.control, scope.master.pid, SHELL_CONTROL, libc::O_WRONLY)?)
    }
}

fn pipe_metadata(file: &File) -> Result<Metadata> {
    let info = file.metadata()?;
    // SAFETY: geteuid has no pointer arguments.
    ensure!(info.file_type().is_fifo() && info.uid() == unsafe { libc::geteuid() }
        && info.mode() & 0o077 == 0, "Startup channel is not an owned private pipe");
    Ok(info)
}

fn peer_pipe(pipe: &File, pid: u32, fd: RawFd, access: libc::c_int) -> Result<bool> {
    let path = format!("/proc/{pid}/fd/{fd}");
    let matches = |other: Metadata| -> Result<bool> { Ok(same_identity(&pipe_metadata(pipe)?, &other)) };
    match fs::metadata(&path) {
        Ok(meta) if matches(meta.clone())? => {},
        _ => return Ok(false),
    }
    let mut flags = String::new();
    match File::open(format!("/proc/{pid}/fdinfo/{fd}")) {
        Ok(info) => { info.take(513).read_to_string(&mut flags)?; }
        Err(_) => return Ok(false),
    }
    let actual = flags.lines().find_map(|line| line.strip_prefix("flags:").and_then(|n| i32::from_str_radix(n.trim(), 8).ok()));
    let Some(actual) = actual else { return Ok(false); };
    Ok(actual & libc::O_ACCMODE == access && fs::metadata(path).ok().is_some_and(|meta|
        pipe.metadata().is_ok_and(|held| same_identity(&held, &meta))))
}

fn child_command(role: &str, profile: DeviceProfile, scope: Scope, owner: Member, until: u64) -> Command {
    // This is the same running program, not a new lookup of the USB path.
    let mut child = Command::new("/proc/self/exe");
    child.args([FLAG, role, profile.id()]).args([
        scope.master.pid.to_string(), scope.master.birth.to_string(), scope.sequence.to_string(),
        scope.begin_cs.to_string(), owner.pid.to_string(), owner.birth.to_string(), until.to_string(),
    ]);
    child
}

fn leader(writer: &Writer, profile: DeviceProfile, scope: Scope, owner: Member, until: u64) -> Result<()> {
    remaining(until)?;
    ensure!(member(&owner_identity(writer, scope, owner.pid)?) == owner, "Startup owner changed");
    let mine = member(&leader_identity(writer, owner, writer.helper)?);
    arm_watchdog(mine.pid as libc::pid_t, writer.records.directory.as_raw_fd(), until)?;
    write_frame(libc::STDOUT_FILENO, b"A")?;
    let gate = duplicate(libc::STDIN_FILENO, 10)?;
    nonblocking(gate.as_raw_fd())?;
    let mut wire = Vec::new();
    loop {
        remaining(until)?;
        if drain(&gate, &mut wire, 128)? { break; }
        pause(until)?;
    }
    let text = std::str::from_utf8(&wire)?.strip_suffix('\n').context("Incomplete startup grant")?;
    let fields: Vec<_> = text.split(':').collect();
    let ["G", pid, birth, directory_dev, directory_ino, dev, ino] = fields.as_slice() else { bail!("Invalid startup grant channel"); };
    ensure!(parse_member(pid, birth)? == mine, "Startup grant is for another reader");
    let directory = &writer.records.identity;
    ensure!((decimal(directory_dev)?, decimal(directory_ino)?) == (directory.dev(), directory.ino()),
        "Startup grant names a different lock directory");
    writer.records.ensure_current()?;
    remaining(until)?;
    // A successful L exit is a faithful witness of the actual sampler's zero
    // exit. No code path maps a signaled or failed sampler to success.
    let status = child_command("sample", profile, scope, owner, until)
        .args([pid, birth, directory_dev, directory_ino, dev, ino])
        .stdin(Stdio::null()).stdout(Stdio::inherit()).stderr(Stdio::null()).status()?;
    ensure!(status.success(), "Startup sampler failed");
    Ok(())
}

struct Grant {
    file: File,
    complete: Option<(String, Metadata)>,
}

impl Grant {
    fn create(writer: &Writer) -> Result<Self> {
        writer.records.ensure_current()?;
        // SAFETY: the directory is pinned and c-string names a single constant leaf.
        let fd = syscall(unsafe { libc::openat(writer.records.directory.as_raw_fd(), c"native-startup-grant".as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK, 0o600) })?;
        let file = unsafe { File::from_raw_fd(fd) };
        Ok(Self { file, complete: None })
    }

    fn publish(&mut self, writer: &Writer, binding: StartupReadBinding) -> Result<()> {
        let body = binding.record(writer);
        self.file.write_all(body.as_bytes())?;
        self.complete = Some((body, self.named(writer)?));
        self.check(writer)
    }

    fn named(&self, writer: &Writer) -> Result<Metadata> {
        writer.records.ensure_current()?;
        let owned = self.file.metadata()?;
        let named = writer.records.open_record(GRANT)?.metadata()?;
        ensure!(private_regular(&owned) && private_regular(&named) && stable_file(&owned, &named), "Startup grant identity changed");
        Ok(named)
    }

    fn check(&self, writer: &Writer) -> Result<()> {
        let (body, published) = self.complete.as_ref().context("Startup grant was not published")?;
        ensure!(stable_file(published, &self.named(writer)?) && writer.records.read(GRANT)? == *body
            && stable_file(published, &self.named(writer)?), "Startup grant changed after publication");
        Ok(())
    }

    fn revoke(&self, writer: &Writer) -> Result<()> {
        let was_named = if self.complete.is_some() { self.check(writer) } else { self.named(writer).map(drop) };
        // Invalidate only the pinned inode, even if its pathname was replaced.
        // A replacement still prevents a clean terminal result and is preserved.
        self.file.set_len(0)?;
        was_named?;
        self.revoked(writer)
    }

    fn revoked(&self, writer: &Writer) -> Result<()> {
        ensure!(self.named(writer)?.len() == 0 && writer.records.read(GRANT)?.is_empty()
            && self.named(writer)?.len() == 0, "Startup grant revocation was not proven");
        Ok(())
    }
}

#[derive(Default)]
struct Transaction {
    leader: Option<Child>,
    bound_group: bool,
    grant: Option<Grant>,
}

struct Snapshot {
    bytes: Vec<u8>,
    sealed_ns: u64,
}

impl Transaction {
    fn capture(&mut self, writer: &Writer, channels: &Channels, profile: DeviceProfile,
        scope: Scope, owner: Member, until: u64) -> Result<Snapshot> {
        ensure!(writer.records.read_optional(GRANT)?.is_none(), "Startup grant slot is not empty");
        remaining(until)?;
        ensure!(channels.active(scope)?, "Launcher cancelled startup");
        let mut command = child_command("leader", profile, scope, owner, until);
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        // SAFETY: setsid is called before executing the private group leader,
        // using only async-signal-safe calls in the pre-exec function.
        unsafe { command.pre_exec(|| syscall(libc::setsid()).map(|_| ())); }
        let child = self.leader.insert(command.spawn()?);
        let pid = child.id();
        // Pin the group before waiting for readiness, so cancellation at the
        // gate can still retire the original group without guessing its identity.
        let pin = leader_identity(writer, owner, pid)?;
        self.bound_group = true;
        let output = child.stdout.take().context("Startup reader output is unavailable")?;
        let output = File::from(std::os::fd::OwnedFd::from(output));
        nonblocking(output.as_raw_fd())?;
        let mut armed = Vec::new();
        loop {
            remaining(until)?;
            ensure!(channels.active(scope)?, "Launcher cancelled startup");
            ensure!(!drain(&output, &mut armed, 1)?, "Startup reader closed before its grant");
            if armed == b"A" { break; }
            ensure!(armed.is_empty(), "Invalid startup watchdog acknowledgement");
            pause(until)?;
        }
        ensure!(leader_identity(writer, owner, pid)? == pin
            && member(&owner_identity(writer, scope, owner.pid)?) == owner, "Startup grant lineage changed");
        ensure!(channels.active(scope)?, "Launcher cancelled startup");
        remaining(until)?;
        let grant = self.grant.insert(Grant::create(writer)?);
        let meta = grant.file.metadata()?;
        let directory = &writer.records.identity;
        let binding = StartupReadBinding {
            scope, owner, leader: member(&pin), cutoff_ns: until,
            directory_dev: directory.dev(), directory_ino: directory.ino(),
            dev: meta.dev(), ino: meta.ino(),
        };
        grant.publish(writer, binding)?;
        let gate = child.stdin.take().context("Startup reader gate is unavailable")?;
        nonblocking(gate.as_raw_fd())?;
        remaining(until)?;
        ensure!(channels.active(scope)?, "Launcher cancelled startup");
        let token = format!("G:{}:{}:{}:{}:{}:{}\n", pin.pid, pin.birth,
            binding.directory_dev, binding.directory_ino, binding.dev, binding.ino);
        write_frame(gate.as_raw_fd(), token.as_bytes())?;
        drop(gate);

        let mut bytes = Vec::with_capacity(BODY_LIMIT);
        let mut eof = false;
        loop {
            remaining(until)?;
            ensure!(channels.active(scope)?, "Launcher cancelled startup");
            if !eof { eof = drain(&output, &mut bytes, BODY_LIMIT)?; }
            match observe_child(pid as libc::pid_t)? {
                Some(false) => bail!("Startup reader did not exit successfully"),
                Some(true) if eof => {
                    ensure!(!bytes.is_empty(), "Startup reader returned no body");
                    grant.check(writer)?;
                    ensure!(member(&owner_identity(writer, scope, owner.pid)?) == owner && channels.active(scope)?,
                        "Startup authorization changed before sealing");
                    // Move the complete bytes into the one-shot result before
                    // checking the clock. A later observation cannot seal them.
                    let mut result = Snapshot { bytes, sealed_ns: 0 };
                    result.sealed_ns = now()?;
                    ensure!(result.sealed_ns < until, "Startup snapshot sealed too late");
                    return Ok(result);
                }
                _ => pause(until)?,
            }
        }
    }

    fn close(&self, writer: &Writer, until: u64) -> Result<()> {
        let end = now()?.checked_add(SECOND).context("Startup retirement time overflow")?.min(until);
        let revoked = match &self.grant {
            Some(grant) => grant.revoke(writer),
            None => writer.records.read_optional(GRANT).and_then(|value| {
                ensure!(value.is_none(), "An unowned startup grant appeared");
                Ok(())
            }),
        };
        let pid = self.leader.as_ref().map(|child| child.id() as libc::pid_t);
        // Authorization could fail before confirming the group's identity. In
        // that case stop only the direct unreaped child, not an assumed group.
        // An elapsed deadline disallows further waiting or a terminal, but not
        // one final safe signal to a still-waitable child or its proven group.
        let signal = pid.map_or(Ok(()), |pid| stop_owned_child(pid, self.bound_group));
        // Attempt bounded retirement even when the name was replaced or a
        // signal failed; neither failure can be turned into a success.
        retire(pid, end)?;
        signal?;
        revoked?;
        if let Some(grant) = &self.grant { grant.revoked(writer)?; }
        remaining(end)?;
        Ok(())
    }
}

fn owner(writer: &Writer, profile: DeviceProfile, scope: Scope) -> Result<u8> {
    fresh(scope)?;
    let until = capture_end(scope)?;
    remaining(until)?;
    let terminal_until = until.checked_add(SECOND).context("Startup terminal time overflow")?;
    let mine = member(&owner_identity(writer, scope, writer.helper)?);
    become_subreaper()?;
    let channels = Channels::open()?;
    let mut transaction = Transaction::default();
    let started = channels.start(writer, scope, until);
    let captured = if matches!(started, Ok(true)) {
        transaction.capture(writer, &channels, profile, scope, mine, until).ok()
    } else { None };
    transaction.close(writer, terminal_until)?;
    // Do not swallow a protocol error after freezing the result. In
    // particular, bytes consumed by a failed check cannot become silence.
    let cancelled = matches!(started, Ok(false)) || !channels.active(scope)?;
    let (status, sealed_ns, body) = if cancelled {
        (CLEAN_CANCEL, 0, "-".to_owned())
    } else if let Some(snapshot) = captured {
        // Revalidate shell authority after retirement as well. A valid seal
        // alone cannot admit startup after a cancellation or changed context.
        ensure!(channels.active(scope)? && member(&owner_identity(writer, scope, mine.pid)?) == mine,
            "Startup launcher authority was lost after sealing");
        (0, snapshot.sealed_ns, hex(&snapshot.bytes))
    } else {
        (CLEAN_REFUSAL, 0, "-".to_owned())
    };
    fresh(scope)?;
    remaining(terminal_until)?;
    let grant = match &transaction.grant {
        Some(grant) => {
            let metadata = grant.named(writer)?;
            ensure!(metadata.len() == 0, "Startup grant was restored after retirement");
            (metadata.dev(), metadata.ino())
        }
        None => (0, 0),
    };
    let directory = &writer.records.identity;
    let terminal = format!("{}:{}:{}:{status}:{until}:{sealed_ns}:{}:{}:{}:{}:{body}\n",
        operation(writer, scope), mine.pid, mine.birth, directory.dev(), directory.ino(), grant.0, grant.1);
    writer.records.ensure_current()?;
    ensure!(status != 0 || channels.active(scope)?, "Startup authority closed before receipt publication");
    remaining(terminal_until)?;
    write_frame(channels.result.as_raw_fd(), terminal.as_bytes())?;
    Ok(status)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut result = String::with_capacity(2 * bytes.len());
    for byte in bytes {
        result.push(DIGITS[(byte >> 4) as usize] as char);
        result.push(DIGITS[(byte & 15) as usize] as char);
    }
    result
}

fn exited_owner(master: &ProcessIdentity, owner: Member) -> Result<()> {
    let pinned = PathBuf::from(format!("/proc/self/fd/{OWNER_PROC}"));
    let expected = format!("/proc/{}", owner.pid);
    let link = fs::read_link(&pinned)?;
    ensure!(pinned.metadata()?.is_dir() && (link.as_os_str() == expected.as_str()
        || link.as_os_str() == format!("{expected} (deleted)").as_str()), "Wrong startup owner proc directory");
    owner_exit_status(master, owner, || process_absent(owner.pid as libc::pid_t), || {
        let file = fs::OpenOptions::new().read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(pinned.join("stat"))?;
        let mut text = String::new();
        file.take((MAX_STATUS_BYTES + 1) as u64).read_to_string(&mut text)?;
        Ok(text)
    })
}

fn owner_exit_status(master: &ProcessIdentity, owner: Member,
    mut absent: impl FnMut() -> Result<bool>, read_pinned_stat: impl FnOnce() -> io::Result<String>,
) -> Result<()> {
    if absent()? { return Ok(()); }
    let text = match read_pinned_stat() {
        Ok(text) => text,
        // The parent can reap its zombie between kill(pid, 0) and either
        // opening or reading its pinned stat. Confirm ESRCH once more; a
        // missing record alone still proves nothing about a live or reused ID.
        Err(error) if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ESRCH)) => {
            if absent()? { return Ok(()); }
            return Err(error).context("Startup owner is still present after its stat disappeared");
        }
        Err(error) => return Err(error).context("Cannot read the pinned startup owner stat"),
    };
    let (raw_pid, _) = text.split_once(" (").context("Invalid startup owner stat")?;
    let (_, tail) = text.rsplit_once(") ").context("Invalid startup owner stat")?;
    let fields: Vec<_> = tail.split_ascii_whitespace().collect();
    ensure!(text.len() <= MAX_STATUS_BYTES && raw_pid == owner.pid.to_string() && fields.len() >= 20
        && matches!(fields[0], "Z" | "X" | "x") && parse_pid(fields[1])? == master.pid
        && parse_pid(fields[2])? == master.group && parse_pid(fields[3])? == master.session
        && decimal(fields[19])? == owner.birth, "Startup owner exit is not proven");
    Ok(())
}

fn receive(writer: &Writer, scope: Scope, owner: Member) -> Result<()> {
    fresh(scope)?;
    let master = check_startup_base(writer, scope)?;
    // SAFETY: no arguments; this is the receiver's actual direct parent.
    ensure!(unsafe { libc::getppid() } as u32 == master.pid, "Startup receiver is not owned by the launcher");
    exited_owner(&master, owner)?;
    let input = duplicate(libc::STDIN_FILENO, 10)?;
    ensure!(peer_pipe(&input, master.pid, SHELL_RESULT, libc::O_RDONLY)?, "Wrong startup result channel");
    nonblocking(input.as_raw_fd())?;
    let mut result = Vec::new();
    // Nonblocking, one shot, and only after actual owner exit readiness.
    ensure!(drain(&input, &mut result, 640)?, "Startup terminal writer is still open");
    let expected = format!("{}:{}:{}:", operation(writer, scope), owner.pid, owner.birth);
    let wire = std::str::from_utf8(&result)?.strip_prefix(&expected).and_then(|s| s.strip_suffix('\n'))
        .context("Startup terminal reply is not bound to this visit")?;
    let fields: Vec<_> = wire.split(':').collect();
    let [status, until, sealed, directory_dev, directory_ino, dev, ino, body] = fields.as_slice() else { bail!("Invalid startup terminal framing"); };
    let until = decimal(until)?;
    let sealed = decimal(sealed)?;
    let (dev, ino) = (decimal(dev)?, decimal(ino)?);
    let directory = &writer.records.identity;
    ensure!((decimal(directory_dev)?, decimal(directory_ino)?) == (directory.dev(), directory.ino()),
        "Startup terminal belongs to a different lock directory");
    ensure!(until == capture_end(scope)?, "Startup receipt has a mismatched deadline");
    remaining(until.checked_add(SECOND).context("Startup terminal time overflow")?)?;
    if *status == "0" {
        ensure!(sealed >= boot_start(scope)? && sealed < until && sealed <= now()? && ino > 0 && !body.is_empty()
            && body.len() <= BODY_LIMIT * 2 && body.len() % 2 == 0
            && body.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)), "Invalid sealed startup body");
    } else {
        ensure!(matches!(*status, "21" | "22") && sealed == 0 && *body == "-", "Invalid startup terminal status");
    }
    if (dev, ino) == (0, 0) {
        ensure!(writer.records.read_optional(GRANT)?.is_none(), "An unowned startup grant appeared after retirement");
    } else {
        let read_grant = || -> Result<Metadata> {
            let meta = writer.records.open_record(GRANT)?.metadata()?;
            ensure!(private_regular(&meta) && meta.len() == 0 && (meta.dev(), meta.ino()) == (dev, ino),
                "Retired startup grant no longer matches the owner's receipt");
            Ok(meta)
        };
        let before = read_grant()?;
        ensure!(writer.records.read(GRANT)?.is_empty() && stable_file(&before, &read_grant()?),
            "Retired startup grant changed during receipt validation");
    }
    check_startup_base(writer, scope)?;
    fresh(scope)?;
    write_frame(libc::STDOUT_FILENO, format!("{status} {body}\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_wire_numbers_and_raw_hex_are_exact() {
        for invalid in ["", "-1", "+1", "01", "1\n", " 1", "18446744073709551616"] {
            assert!(decimal(invalid).is_err());
        }
        assert_eq!(hex(&[0, b'\n', 128, 255]), "000a80ff");
    }

    #[test]
    fn startup_reader_witnesses_the_extra_byte_and_refuses_held_eof() -> Result<()> {
        let (input, output) = pipe()?;
        write_frame(output.as_raw_fd(), &[1; BODY_LIMIT])?;
        let mut body = Vec::new();
        assert!(!drain(&input, &mut body, BODY_LIMIT)?);
        assert_eq!(body.len(), BODY_LIMIT);
        write_frame(output.as_raw_fd(), &[2])?;
        assert!(drain(&input, &mut body, BODY_LIMIT).is_err());
        drop(output);
        assert!(drain(&input, &mut body, BODY_LIMIT)?);
        Ok(())
    }

    fn exit_identity() -> (ProcessIdentity, Member) {
        let master = ProcessIdentity {
            pid: 101, parent: 1, group: 101, session: 101, birth: 50,
            executable: PathBuf::from("/bin/sh"),
        };
        (master, Member { pid: 102, birth: 60 })
    }

    #[test]
    fn owner_reaped_between_existence_check_and_stat_requires_a_fresh_esrch() -> Result<()> {
        let (master, owner) = exit_identity();
        for errno in [libc::ENOENT, libc::ESRCH] {
            let mut checks = 0;
            owner_exit_status(&master, owner, || {
                checks += 1;
                Ok(checks == 2)
            }, || Err(io::Error::from_raw_os_error(errno)))?;
            assert_eq!(checks, 2);
            assert!(owner_exit_status(&master, owner, || Ok(false), || {
                Err(io::Error::from_raw_os_error(errno))
            }).is_err());
        }
        Ok(())
    }

    #[test]
    fn owner_exit_keeps_the_pinned_zombie_and_io_error_gates() -> Result<()> {
        let (master, owner) = exit_identity();
        let stat = |state: &str, birth: u64| format!(
            "{} (snapshot (owner)) {state} {} {} {} {}{birth}\n",
            owner.pid, master.pid, master.group, master.session, "0 ".repeat(15),
        );
        owner_exit_status(&master, owner, || Ok(false), || Ok(stat("Z", owner.birth)))?;
        assert!(owner_exit_status(&master, owner, || Ok(false), || Ok(stat("S", owner.birth))).is_err());
        assert!(owner_exit_status(&master, owner, || Ok(false), || Ok(stat("Z", owner.birth + 1))).is_err());
        assert!(owner_exit_status(&master, owner, || Ok(false), || Ok(String::new())).is_err());
        let mut checks = 0;
        assert!(owner_exit_status(&master, owner, || {
            checks += 1;
            Ok(checks == 2)
        }, || Err(io::Error::from_raw_os_error(libc::EACCES))).is_err());
        assert_eq!(checks, 1);
        Ok(())
    }

    #[test]
    fn exact_child_wait_retains_zero_and_nonzero_status() -> Result<()> {
        for (code, success) in [("0", true), ("39", false)] {
            let mut child = Command::new("/bin/sh")
                .args(["-c", "exit \"$1\"", "sh", code])
                .spawn()?;
            let observed = (|| -> Result<bool> {
                let until = now()? + SECOND;
                loop {
                    if let Some(value) = observe_child(child.id() as libc::pid_t)? {
                        return Ok(value);
                    }
                    pause(until)?;
                }
            })();
            let status = child.wait()?;
            assert_eq!(observed?, success);
            assert_eq!(status.code(), Some(code.parse()?));
            assert!(observe_child(child.id() as libc::pid_t).is_err());
        }
        Ok(())
    }
}

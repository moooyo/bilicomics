//! Read-only permission for the startup display sampler, not for legacy capture or publication.

use super::{
    MAX_CHAIN_MEMBERS, PrivateDirectory, ProcessIdentity, Writer, private_regular, stable_file,
};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::CStr,
    fs::{self, File, Metadata},
    io,
    mem::MaybeUninit,
    os::{
        fd::AsRawFd,
        unix::fs::{FileExt, MetadataExt},
    },
    path::Path,
    time::{Duration, Instant},
};

use crate::platform::NATIVE_CAPTURE_TIMEOUT;

const STARTUP_GRANT: &str = "native-startup-grant";
const NANOS_PER_SECOND: u64 = 1_000_000_000;
const NANOS_PER_CENTISECOND: u64 = NANOS_PER_SECOND / 100;
const INCOMPATIBLE_MARKERS: &[&CStr] = &[
    c"application-pid",
    c"application-pid.pending",
    c"application-exit",
    c"application-exit.pending",
    c"native-output-capability",
    c"native-output-capability.pending",
    c"lifecycle-ready",
    c"lifecycle-ready.pending",
    c"lifecycle-request",
    c"lifecycle-request.pending",
    c"lifecycle-ack",
    c"lifecycle-ack.pending",
    c"sleep-cover-request",
    c"sleep-cover-request.pending",
    c"sleep-cover-ready",
    c"sleep-cover-ready.pending",
    c"native-read-go",
    c"native-read-go.pending",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Member {
    pub(super) pid: u32,
    pub(super) birth: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Scope {
    pub(super) master: Member,
    pub(super) sequence: u64,
    pub(super) begin_cs: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StartupReadBinding {
    pub(super) scope: Scope,
    pub(super) owner: Member,
    pub(super) leader: Member,
    pub(super) cutoff_ns: u64,
    pub(super) directory_dev: u64,
    pub(super) directory_ino: u64,
    pub(super) dev: u64,
    pub(super) ino: u64,
}

impl StartupReadBinding {
    pub(super) fn record(self, writer: &Writer) -> String {
        let Scope { master, sequence, begin_cs } = self.scope;
        format!(
            "startup-display-v1:{}:{sequence}:{}:{}:{begin_cs}:{}:{}:{}:{}:{}\n",
            writer.header(),
            master.pid,
            master.birth,
            self.owner.pid,
            self.owner.birth,
            self.leader.pid,
            self.leader.birth,
            self.cutoff_ns,
        )
    }
}

pub(super) fn check_startup_base(writer: &Writer, scope: Scope) -> Result<ProcessIdentity> {
    ensure!(
        scope.sequence > 0
            && scope.master.pid > 0
            && writer.generation > 0
            && writer.generation_text == writer.generation.to_string(),
        "Invalid startup native-read scope"
    );
    ensure!(
        writer.nonce.len() == 32
            && writer.nonce.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "Invalid startup native-read token"
    );
    writer.records.ensure_current()?;
    ensure!(
        writer.records.read("generation")? == format!("{}\n", writer.header())
            && writer.records.read("nonce")? == format!("{}\n", writer.nonce)
            && writer.records.read("pid")? == format!("{}\n", scope.master.pid),
        "The startup native reader belongs to another launch"
    );
    let master = writer.process(scope.master.pid)?;
    ensure!(master.birth == scope.master.birth, "The startup launcher birth changed");
    for name in INCOMPATIBLE_MARKERS {
        require_absent(&writer.records, name)?;
    }
    writer.records.ensure_current()?;
    Ok(master)
}

fn require_absent(records: &PrivateDirectory, name: &CStr) -> Result<()> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: the private directory and terminated leaf are live, and stat is
    // writable. A dangling symlink or any other existing leaf is not absence.
    let status = unsafe {
        libc::fstatat(
            records.directory.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    ensure!(status != 0, "Incompatible startup native-read marker: {name:?}");
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::NotFound { Ok(()) } else { Err(error.into()) }
}

struct Grant {
    file: File,
    identity: Metadata,
    expected: String,
}

impl Grant {
    fn open(writer: &Writer, binding: StartupReadBinding) -> Result<Self> {
        writer.records.ensure_current()?;
        let file = writer.records.open_record(STARTUP_GRANT)?;
        let identity = file.metadata()?;
        let expected = binding.record(writer);
        ensure!(
            private_regular(&identity)
                && identity.dev() == binding.dev
                && identity.ino() == binding.ino
                && identity.len() == expected.len() as u64,
            "The startup native-read grant does not match its private file identity"
        );
        Ok(Self { file, identity, expected })
    }

    fn recheck(&self, records: &PrivateDirectory) -> Result<()> {
        let matches = |metadata: &Metadata| {
            private_regular(metadata) && stable_file(&self.identity, metadata)
        };
        ensure!(matches(&self.file.metadata()?), "The startup native-read grant changed");
        let mut bytes = vec![0; self.expected.len()];
        self.file.read_exact_at(&mut bytes, 0)?;
        ensure!(bytes.as_slice() == self.expected.as_bytes(), "Invalid startup native-read grant record");
        // Hold the original inode across reads. Never accept another file with
        // the same bytes, or an in-place revocation or rewrite of the grant.
        ensure!(
            matches(&self.file.metadata()?) && matches(&records.open_record(STARTUP_GRANT)?.metadata()?),
            "The startup native-read grant was changed or replaced"
        );
        Ok(())
    }
}

type ExecutableIdentity = (u64, u64);

fn executable_identity(path: &Path) -> Result<ExecutableIdentity> {
    // Stat the kernel executable link itself with dereferencing. Using its
    // printed path as authority would instead attest whatever now occupies it.
    let executable = fs::metadata(path)?;
    ensure!(executable.is_file(), "Invalid startup native-read executable");
    Ok((executable.dev(), executable.ino()))
}

struct Lineage {
    master: ProcessIdentity,
    descendants: Vec<ProcessIdentity>,
}

impl Lineage {
    fn prove(writer: &Writer, binding: StartupReadBinding) -> Result<Self> {
        let master = check_startup_base(writer, binding.scope)?;
        let owner_pid = binding.owner.pid;
        let leader_pid = binding.leader.pid;
        ensure!(
            owner_pid > 0 && leader_pid > 0 && owner_pid != master.pid && leader_pid != master.pid
                && owner_pid != leader_pid && ![master.pid, owner_pid, leader_pid].contains(&writer.helper),
            "The startup native-read roles must be distinct"
        );
        let owner = writer.process(owner_pid)?;
        let leader = writer.process(leader_pid)?;
        ensure!(
            owner.birth == binding.owner.birth && owner.parent == master.pid
                && owner.group == master.group && owner.session == master.session,
            "The startup native-read owner is not the expected launcher child"
        );
        ensure!(
            leader.birth == binding.leader.birth && leader.parent == owner.pid
                && leader.group == leader_pid && leader.session == leader_pid,
            "The startup native-read leader does not own the expected session"
        );
        let mut descendants = vec![owner, leader];
        let mut pid = writer.helper;
        for _ in 0..MAX_CHAIN_MEMBERS {
            ensure!(
                pid != master.pid && !descendants.iter().any(|member| member.pid == pid),
                "Invalid startup sampler ancestry"
            );
            let member = writer.process(pid)?;
            ensure!(
                member.group == leader_pid && member.session == leader_pid,
                "The startup sampler escaped the permitted group or session"
            );
            pid = member.parent;
            descendants.push(member);
            if pid == leader_pid {
                return Ok(Self { master, descendants });
            }
        }
        anyhow::bail!("The startup sampler is not a descendant of the permitted leader")
    }

    fn recheck(
        &self,
        writer: &Writer,
        binding: StartupReadBinding,
        executable: ExecutableIdentity,
    ) -> Result<()> {
        for member in &self.descendants {
            ensure!(writer.process(member.pid)? == *member, "The startup native-read lineage changed");
            if [binding.owner.pid, binding.leader.pid, writer.helper].contains(&member.pid) {
                let exe = writer.proc_root.join(member.pid.to_string()).join("exe");
                ensure!(
                    executable_identity(&exe)? == executable,
                    "A startup native-read participant is not running the current executable"
                );
            }
        }
        ensure!(
            check_startup_base(writer, binding.scope)? == self.master,
            "The startup launcher identity changed"
        );
        Ok(())
    }
}

/// An opaque permit, constructed only for the authenticated startup sampler.
/// It holds no authority to signal processes, publish files, or change device state.
pub(crate) struct StartupReadPermit {
    writer: Writer,
    binding: StartupReadBinding,
    lineage: Lineage,
    executable: ExecutableIdentity,
    grant: Grant,
    deadline: Instant,
}

impl StartupReadPermit {
    #[cfg(feature = "kindle")]
    fn acquire(binding: StartupReadBinding, end: Instant) -> Result<Self> {
        Self::from_writer(
            Writer::from_environment()?,
            binding,
            end,
            executable_identity(Path::new("/proc/self/exe"))?,
        )
    }

    fn from_writer(
        writer: Writer,
        binding: StartupReadBinding,
        end: Instant,
        executable: ExecutableIdentity,
    ) -> Result<Self> {
        let deadline = deadline_for(end, binding)?;
        let directory = &writer.records.identity;
        ensure!((directory.dev(), directory.ino()) == (binding.directory_dev, binding.directory_ino),
            "The startup native-read grant belongs to a different directory");
        let lineage = Lineage::prove(&writer, binding)?;
        let grant = Grant::open(&writer, binding)?;
        let permit = Self { writer, binding, lineage, executable, grant, deadline };
        permit.recheck()?;
        Ok(permit)
    }

    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }

    pub(crate) fn recheck(&self) -> Result<()> {
        ensure!(Instant::now() < self.deadline(), "Startup native-read deadline elapsed");
        self.lineage.recheck(&self.writer, self.binding, self.executable)?;
        self.grant.recheck(&self.writer.records)?;
        self.writer.records.ensure_current()?;
        // The launch window also counts time spent in system suspend.
        startup_window_remaining(self.binding.scope.begin_cs, clock_ns(libc::CLOCK_BOOTTIME)?)?;
        ensure!(Instant::now() < self.deadline(), "Startup native-read deadline elapsed");
        Ok(())
    }
}

fn clock_ns(clock: libc::clockid_t) -> Result<u64> {
    let mut stamp = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime writes only to the live timespec.
    if unsafe { libc::clock_gettime(clock, &mut stamp) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    ensure!((0..1_000_000_000).contains(&stamp.tv_nsec), "Invalid startup clock");
    u64::try_from(stamp.tv_sec)?
        .checked_mul(NANOS_PER_SECOND)
        .and_then(|seconds| seconds.checked_add(stamp.tv_nsec as u64))
        .context("Startup clock overflow")
}

fn startup_window_remaining(begin_cs: u64, boot_ns: u64) -> Result<Duration> {
    let begin = begin_cs.checked_mul(NANOS_PER_CENTISECOND).context("Invalid startup begin time")?;
    let elapsed = boot_ns.checked_sub(begin).context("Future startup begin time")?;
    NATIVE_CAPTURE_TIMEOUT
        .checked_sub(Duration::from_nanos(elapsed))
        .filter(|remaining| !remaining.is_zero())
        .context("Startup native-read window elapsed")
}

fn deadline_for(end: Instant, binding: StartupReadBinding) -> Result<Instant> {
    // Use the earlier Instant observation so clock conversion cannot extend
    // the absolute cutoff or the original startup window.
    let now = Instant::now();
    let boot = clock_ns(libc::CLOCK_BOOTTIME)?;
    let left = binding.cutoff_ns.checked_sub(boot).context("Startup native-read cutoff elapsed")?;
    let remaining = end.saturating_duration_since(now)
        .min(Duration::from_nanos(left))
        .min(startup_window_remaining(binding.scope.begin_cs, boot)?);
    ensure!(!remaining.is_zero(), "Startup native-read deadline elapsed");
    now.checked_add(remaining).context("Startup native-read deadline overflow")
}

#[cfg(feature = "kindle")]
pub(super) fn sample(
    profile: crate::platform::DeviceProfile,
    binding: StartupReadBinding,
    end: std::time::Instant,
) -> Result<String> {
    let permit = StartupReadPermit::acquire(binding, end)?;
    crate::platform::capture_native_startup_display_record(profile, permit).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        path::PathBuf,
    };

    const NONCE: &str = "0123456789abcdef0123456789abcdef";

    struct Fixture {
        temporary: tempfile::TempDir,
        writer: Writer,
        image: PathBuf,
        binding: StartupReadBinding,
    }

    impl Fixture {
        fn new() -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let lock = temporary.path().join("lock");
            fs::create_dir(&lock).unwrap();
            fs::set_permissions(&lock, fs::Permissions::from_mode(0o700)).unwrap();
            let image = temporary.path().join("bilicomics");
            fs::write(&image, b"executable identity fixture").unwrap();
            let writer = Writer {
                records: PrivateDirectory::open(&lock).unwrap(),
                generation: 7,
                generation_text: "7".into(),
                nonce: NONCE.into(),
                helper: 105,
                proc_root: temporary.path().join("proc"),
            };
            let (directory_dev, directory_ino) = (writer.records.identity.dev(), writer.records.identity.ino());
            let mut fixture = Self {
                temporary,
                writer,
                image,
                binding: StartupReadBinding {
                    scope: Scope { master: Member { pid: 101, birth: 401 }, sequence: 9, begin_cs: 0 },
                    owner: Member { pid: 102, birth: 402 },
                    leader: Member { pid: 103, birth: 403 },
                    cutoff_ns: 0,
                    directory_dev,
                    directory_ino,
                    dev: 0,
                    ino: 0,
                },
            };
            for (pid, parent, group, session, birth) in [
                (101, 1, 55, 54, 401),
                (102, 101, 55, 54, 402),
                (103, 102, 103, 103, 403),
                (104, 103, 103, 103, 404),
                (105, 104, 103, 103, 405),
            ] {
                fixture.process(pid, parent, group, session, birth);
            }
            fixture.write("generation", &format!("7:{NONCE}\n"));
            fixture.write("nonce", &format!("{NONCE}\n"));
            fixture.write("pid", "101\n");
            fixture.binding.scope.begin_cs = clock_ns(libc::CLOCK_BOOTTIME).unwrap() / NANOS_PER_CENTISECOND;
            fixture.binding.cutoff_ns = clock_ns(libc::CLOCK_BOOTTIME).unwrap() + NANOS_PER_SECOND;
            fixture.write(STARTUP_GRANT, &fixture.binding.record(&fixture.writer));
            let grant = fs::metadata(fixture.lock().join(STARTUP_GRANT)).unwrap();
            fixture.binding.dev = grant.dev();
            fixture.binding.ino = grant.ino();
            fixture
        }

        fn process(&self, pid: u32, parent: u32, group: u32, session: u32, birth: u64) {
            let directory = self.writer.proc_root.join(pid.to_string());
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join("stat"),
                format!("{pid} (fixture) S {parent} {group} {session} {}{birth}\n", "0 ".repeat(15)),
            ).unwrap();
            let exe = directory.join("exe");
            if !exe.exists() {
                symlink(&self.image, exe).unwrap();
            }
        }

        fn lock(&self) -> &Path {
            &self.writer.records.path
        }

        fn write(&self, name: &str, bytes: &str) {
            let path = self.lock().join(name);
            fs::write(&path, bytes).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }

        fn permit(&self, binding: StartupReadBinding, end: Instant) -> Result<StartupReadPermit> {
            let writer = Writer {
                records: PrivateDirectory::open(self.lock())?,
                generation: self.writer.generation,
                generation_text: self.writer.generation_text.clone(),
                nonce: self.writer.nonce.clone(),
                helper: self.writer.helper,
                proc_root: self.writer.proc_root.clone(),
            };
            StartupReadPermit::from_writer(writer, binding, end, executable_identity(&self.image)?)
        }

        fn acquire(&self) -> Result<StartupReadPermit> {
            self.permit(self.binding, Instant::now() + NATIVE_CAPTURE_TIMEOUT)
        }
    }

    #[test]
    fn startup_grant_requires_the_exact_purpose_and_bytes() {
        let fixture = Fixture::new();
        fixture.acquire().unwrap().recheck().unwrap();
        let mut binding = fixture.binding;
        binding.scope.begin_cs = 51;
        binding.cutoff_ns = 902;
        assert_eq!(binding.record(&fixture.writer), format!(
            "startup-display-v1:7:{NONCE}:9:101:401:51:102:402:103:403:902\n"
        ));
        let record = fixture.binding.record(&fixture.writer);
        for invalid in [
            record.trim_end().to_owned(),
            record.replace('\n', "\r\n"),
            record.replace(":9:", ":09:"),
            record.replace("startup-display-v1", "capture"),
        ] {
            fixture.write(STARTUP_GRANT, &invalid);
            assert!(fixture.acquire().is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn startup_rechecks_the_canonical_base_and_rejects_any_conflicting_leaf() {
        let mut fixture = Fixture::new();
        let permit = fixture.acquire().unwrap();
        for &name in INCOMPATIBLE_MARKERS {
            let leaf = fixture.lock().join(name.to_str().unwrap());
            symlink(fixture.temporary.path().join("missing"), &leaf).unwrap();
            assert!(permit.recheck().is_err(), "{name:?}");
            fs::remove_file(leaf).unwrap();
        }
        permit.recheck().unwrap();
        fixture.write("pid", "0101\n");
        assert!(permit.recheck().is_err());
        fixture.write("pid", "101\n");
        fixture.writer.generation_text = "+7".into();
        assert!(fixture.acquire().is_err());
        fixture.writer.generation_text = "7".into();
        fixture.write("generation", &format!("07:{NONCE}\n"));
        assert!(permit.recheck().is_err());
    }

    #[test]
    fn startup_requires_the_exact_births_direct_edges_and_group_session() {
        for (pid, parent, group, session, birth) in [
            (101, 1, 55, 54, 409),
            (102, 100, 55, 54, 402),
            (102, 101, 56, 54, 402),
            (102, 101, 55, 55, 402),
            (102, 101, 55, 54, 409),
            (103, 101, 103, 103, 403),
            (103, 102, 55, 103, 403),
            (103, 102, 103, 54, 403),
            (103, 102, 103, 103, 409),
            (104, 101, 103, 103, 404),
            (105, 104, 103, 54, 405),
        ] {
            let fixture = Fixture::new();
            let permit = fixture.acquire().unwrap();
            fixture.process(pid, parent, group, session, birth);
            assert!(fixture.acquire().is_err(), "{pid}:{parent}:{group}:{session}:{birth}");
            assert!(permit.recheck().is_err(), "{pid}:{parent}:{group}:{session}:{birth}");
        }
    }

    #[test]
    fn startup_rejects_a_different_executable_inode_even_at_the_same_path() {
        let fixture = Fixture::new();
        let permit = fixture.acquire().unwrap();
        fs::rename(&fixture.image, fixture.temporary.path().join("original-image")).unwrap();
        fs::write(&fixture.image, b"replacement image").unwrap();
        let error = permit.recheck().err().unwrap();
        assert!(error.to_string().contains("current executable"));
    }

    #[test]
    fn startup_rejects_revocation_and_identical_replacements() {
        let fixture = Fixture::new();
        let grant = fixture.lock().join(STARTUP_GRANT);
        let record = fixture.binding.record(&fixture.writer);
        let permit = fixture.acquire().unwrap();
        fs::rename(&grant, fixture.temporary.path().join("original-grant")).unwrap();
        fixture.write(STARTUP_GRANT, &record);
        assert!(fixture.acquire().is_err());
        assert!(permit.recheck().is_err());

        let fixture = Fixture::new();
        let permit = fixture.acquire().unwrap();
        fixture.write(STARTUP_GRANT, "");
        assert!(permit.recheck().is_err());
    }

    #[test]
    fn startup_rejects_a_replacement_directory_even_with_the_original_grant() {
        let fixture = Fixture::new();
        let permit = fixture.acquire().unwrap();
        let relocated = fixture.temporary.path().join("original-lock");
        fs::rename(fixture.lock(), &relocated).unwrap();
        fs::create_dir(fixture.lock()).unwrap();
        fs::set_permissions(fixture.lock(), fs::Permissions::from_mode(0o700)).unwrap();
        for record in ["pid", "nonce", "generation", STARTUP_GRANT] {
            fs::rename(relocated.join(record), fixture.lock().join(record)).unwrap();
        }
        assert!(fixture.acquire().is_err());
        assert!(permit.recheck().is_err());
    }

    #[test]
    fn startup_deadline_never_extends_the_caller_cutoff_or_original_window() {
        let fixture = Fixture::new();
        let mut binding = fixture.binding;
        assert!(fixture.permit(binding, Instant::now()).is_err());
        binding.cutoff_ns = 0;
        assert!(fixture.permit(binding, Instant::now() + NATIVE_CAPTURE_TIMEOUT).is_err());
        assert_eq!(
            startup_window_remaining(100, 2 * NANOS_PER_SECOND).unwrap(),
            Duration::from_secs(1),
        );
        assert!(startup_window_remaining(100, 3 * NANOS_PER_SECOND).is_err());
        assert!(startup_window_remaining(101, NANOS_PER_SECOND).is_err());
    }
}

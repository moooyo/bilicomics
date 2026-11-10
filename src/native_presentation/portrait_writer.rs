//! Read permission and exclusive sidecars for a bounded native-read descendant.

use super::*;

const STARTUP_PENDING: &str = "native-portrait-orientation.pending";
const RESUME_PENDING: &str = "resume-native-portrait-orientation.pending";
const MAX_CHAIN_MEMBERS: usize = 12;

#[derive(Clone, Copy)]
enum Publication {
    Startup(NativePortraitCalibration),
    Resume(u64),
}

#[derive(Clone, Copy)]
enum OwnershipPurpose {
    #[cfg(any(test, feature = "kindle"))]
    Capture,
    Publish(Publication),
}

impl Publication {
    fn parse(value: &str) -> Result<Self> {
        if let Some(sequence) = value.strip_prefix("CALIBRATED:") {
            let parsed = sequence
                .parse::<u64>()
                .context("Invalid portrait resume sequence")?;
            ensure!(
                parsed > 1 && sequence == parsed.to_string(),
                "Invalid portrait resume sequence"
            );
            return Ok(Self::Resume(parsed));
        }
        Ok(Self::Startup(NativePortraitCalibration::parse(value)?))
    }

    fn names(self) -> (&'static str, &'static str) {
        match self {
            Self::Startup(_) => (PORTRAIT_RECORD, STARTUP_PENDING),
            Self::Resume(_) => (RESUME_PORTRAIT_RECORD, RESUME_PENDING),
        }
    }

    fn content(self, header: &str) -> String {
        match self {
            Self::Startup(calibration) => {
                let direction = if calibration.fixed {
                    "FIXED"
                } else {
                    match calibration.direction {
                        NativePortraitDirection::Up => "U",
                        NativePortraitDirection::Down => "D",
                    }
                };
                format!(
                    "{header}:{direction}:{}\n",
                    calibration.framebuffer_rotation
                )
            }
            Self::Resume(sequence) => format!("{header}:{sequence}:CALIBRATED\n"),
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
struct ProcessIdentity {
    pid: u32,
    parent: u32,
    group: u32,
    session: u32,
    birth: u64,
    executable: PathBuf,
}

#[derive(Eq, PartialEq)]
struct Ownership {
    sources: Vec<(&'static str, Option<String>)>,
    processes: Vec<ProcessIdentity>,
}

struct Writer {
    records: PrivateDirectory,
    generation: u64,
    nonce: String,
    helper: u32,
    proc_root: PathBuf,
}

/// A read-only proof of the existing launcher's bounded native-read context.
/// Keeping this proof alive does not create or publish any marker.
#[cfg(any(test, feature = "kindle"))]
pub(crate) struct NativeReadPermit {
    writer: Writer,
    ownership: Ownership,
}

#[cfg(any(test, feature = "kindle"))]
impl NativeReadPermit {
    #[cfg(feature = "kindle")]
    pub(crate) fn acquire() -> Result<Self> {
        Self::from_writer(Writer::from_environment()?)
    }

    fn from_writer(writer: Writer) -> Result<Self> {
        let ownership = writer.ownership_for(OwnershipPurpose::Capture)?;
        Ok(Self { writer, ownership })
    }

    pub(crate) fn recheck(&self) -> Result<()> {
        ensure!(
            self.writer.ownership_for(OwnershipPurpose::Capture)? == self.ownership,
            "Native-read ownership changed during capture"
        );
        Ok(())
    }
}

pub(super) fn publish(value: &str) -> Result<()> {
    let publication = Publication::parse(value)?;
    Writer::from_environment()?.publish(publication)
}

impl Writer {
    fn from_environment() -> Result<Self> {
        let directory = std::env::var_os("BILICOMICS_LIFECYCLE_DIR")
            .context("Missing native-read directory")?;
        let nonce = std::env::var("BILICOMICS_LAUNCH_TOKEN")
            .context("Missing native-read token")?;
        ensure!(
            nonce.len() == 32 && nonce.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Invalid native-read token"
        );
        let generation = std::env::var("BILICOMICS_LAUNCH_GENERATION")
            .context("Missing native-read generation")?
            .parse::<u64>()?;
        ensure!(generation > 0, "Invalid native-read generation");
        Ok(Self {
            records: PrivateDirectory::open(Path::new(&directory))?,
            generation,
            nonce,
            helper: std::process::id(),
            proc_root: PathBuf::from("/proc"),
        })
    }

    fn header(&self) -> String {
        format!("{}:{}", self.generation, self.nonce)
    }

    fn publish(&self, publication: Publication) -> Result<()> {
        let ownership = self.ownership(publication)?;
        let (target, pending) = publication.names();
        let content = publication.content(&self.header());
        self.records
            .publish_private_marker(target, pending, &content, true, || {
                ensure!(
                    self.ownership(publication)? == ownership,
                    "Portrait publication ownership changed before commit"
                );
                Ok(())
            })?;
        ensure!(
            self.records.read(target)? == content,
            "The portrait sidecar changed after publication"
        );
        ensure!(
            self.ownership(publication)? == ownership,
            "Portrait publication ownership changed after commit"
        );
        self.records.ensure_current()?;
        Ok(())
    }

    fn ownership(&self, publication: Publication) -> Result<Ownership> {
        self.ownership_for(OwnershipPurpose::Publish(publication))
    }

    fn ownership_for(&self, purpose: OwnershipPurpose) -> Result<Ownership> {
        self.records.ensure_current()?;
        let header = self.header();
        let generation = self.records.read("generation")?;
        let nonce = self.records.read("nonce")?;
        ensure!(
            generation == format!("{header}\n") && nonce == format!("{}\n", self.nonce),
            "The portrait publication belongs to another launch"
        );
        let master_record = self.records.read("pid")?;
        let master = canonical_pid_record(&master_record)?;
        ensure!(
            master != self.helper,
            "The portrait helper cannot be its own launcher"
        );
        let permission = self.records.read("native-read-go")?;
        let line = permission
            .strip_suffix('\n')
            .context("Invalid native-read permission framing")?;
        let mut fields = line.split(':');
        let leader = parse_pid(fields.next().context("Missing native-read leader")?)?;
        let sequence_text = fields.next().context("Missing native-read sequence")?;
        let sequence = sequence_text.parse::<u64>()?;
        ensure!(
            sequence > 0 && sequence_text == sequence.to_string(),
            "Invalid native-read sequence"
        );
        ensure!(
            fields.next() == Some(self.nonce.as_str())
                && fields.next().is_none()
                && permission == format!("{leader}:{sequence}:{}\n", self.nonce),
            "The native-read permission belongs to another launch"
        );
        ensure!(
            leader != master && leader != self.helper,
            "Invalid native-read process relationship"
        );

        let master_identity = self.process(master)?;
        let leader_identity = self.process(leader)?;
        ensure!(
            leader_identity.parent == master
                && leader_identity.group == leader
                && leader_identity.session == leader,
            "The native reader is not owned by this launcher"
        );
        let mut processes = vec![master_identity, leader_identity.clone()];
        let mut member = self.helper;
        let mut reached_leader = false;
        for _ in 0..MAX_CHAIN_MEMBERS {
            let current = self.process(member)?;
            ensure!(
                current.group == leader && current.session == leader,
                "The portrait helper escaped its bounded native-read group"
            );
            if member == leader {
                ensure!(
                    current == leader_identity,
                    "The native-read leader changed during ancestry checks"
                );
                reached_leader = true;
                break;
            }
            ensure!(
                !processes.iter().any(|identity| identity.pid == member),
                "Cyclic portrait helper ancestry"
            );
            member = current.parent;
            processes.push(current);
        }
        ensure!(
            reached_leader,
            "The portrait helper does not descend from the permitted native reader"
        );

        let application = self.records.read_optional("application-pid")?;
        let request = self.records.read_optional("lifecycle-request")?;
        let mut sources = vec![
            ("generation", Some(generation)),
            ("nonce", Some(nonce)),
            ("pid", Some(master_record)),
            ("native-read-go", Some(permission)),
            ("application-pid", application.clone()),
            ("lifecycle-request", request.clone()),
        ];
        let suspend_sequence = match purpose {
            #[cfg(any(test, feature = "kindle"))]
            OwnershipPurpose::Capture => match (&application, &request) {
                (None, None) => None,
                (Some(_), Some(request)) => {
                    let sequence = request
                        .strip_prefix(&format!("{header}:"))
                        .and_then(|value| value.strip_suffix(":suspend\n"))
                        .context("Native capture does not follow a current suspend request")?;
                    let parsed = sequence.parse::<u64>()?;
                    ensure!(
                        parsed > 0 && sequence == parsed.to_string(),
                        "Invalid native capture suspend sequence"
                    );
                    Some(parsed)
                }
                _ => bail!("The native capture lifecycle context is incomplete"),
            },
            OwnershipPurpose::Publish(Publication::Startup(_)) => {
                ensure!(
                    application.is_none() && request.is_none(),
                    "Startup portrait publication follows application initialization"
                );
                let mode = self.records.read("native-epdc-mode")?;
                ensure!(
                    mode == format!("{header}:Y8\n") || mode == format!("{header}:Y8INV\n"),
                    "Startup portrait publication lacks its native mode snapshot"
                );
                sources.push(("native-epdc-mode", Some(mode)));
                None
            }
            OwnershipPurpose::Publish(Publication::Resume(sequence)) => Some(sequence - 1),
        };
        if let Some(sequence) = suspend_sequence {
            let application = canonical_pid_record(
                application
                    .as_deref()
                    .context("Resume native read has no application identity")?,
            )?;
            ensure!(
                application != self.helper && application != master && application != leader,
                "Invalid resumed portrait application identity"
            );
            let application_identity = self.process(application)?;
            let runner = self.process(application_identity.parent)?;
            ensure!(
                runner.parent == master
                    && runner.pid != master
                    && runner.pid != leader
                    && runner.pid != self.helper,
                "The resumed application is not owned by this launcher"
            );
            processes.push(application_identity);
            processes.push(runner);
            let previous = format!("{header}:{sequence}:suspend");
            ensure!(
                request.as_deref() == Some(format!("{previous}\n").as_str()),
                "Native resume read does not follow the current suspend request"
            );
            let ready = self.records.read("lifecycle-ready")?;
            ensure!(
                ready == format!("{header}\n"),
                "The resumed application lifecycle bridge is not ready"
            );
            let acknowledged = self.records.read("lifecycle-ack")?;
            ensure!(
                acknowledged == format!("{previous}:ok\n")
                    || acknowledged == format!("{previous}:checkpoint-error\n")
                    || acknowledged == format!("{previous}\n"),
                "The current suspend has not released native ownership"
            );
            sources.extend([
                ("lifecycle-ready", Some(ready)),
                ("lifecycle-ack", Some(acknowledged)),
            ]);
            if let OwnershipPurpose::Publish(Publication::Resume(sequence)) = purpose {
                let mode = self.records.read("resume-native-epdc-mode")?;
                let prefix = format!("{header}:{sequence}:");
                ensure!(
                    mode == format!("{prefix}Y8\n")
                        || mode == format!("{prefix}Y8INV\n")
                        || mode == format!("{prefix}FRAMEBUFFER\n"),
                    "Portrait resume publication lacks its current native mode request"
                );
                if mode == format!("{prefix}FRAMEBUFFER\n") {
                    let capability = self.records.read(CAPABILITY_RECORD)?;
                    ensure!(
                        capability == format!("{header}:{application}:framebuffer8\n"),
                        "The current application did not authorize framebuffer resume"
                    );
                    sources.push((CAPABILITY_RECORD, Some(capability)));
                }
                sources.push(("resume-native-epdc-mode", Some(mode)));
            }
        }
        // Verify the same live births, ancestry, executables, and source records
        // after enumeration; no unproven process is signaled by this helper.
        for identity in &processes {
            ensure!(
                self.process(identity.pid)? == *identity,
                "Portrait publication process identity changed"
            );
        }
        for (name, expected) in &sources {
            ensure!(
                self.records.read_optional(name)? == *expected,
                "Portrait publication source record changed"
            );
        }
        self.records.ensure_current()?;
        Ok(Ownership { sources, processes })
    }

    fn process(&self, pid: u32) -> Result<ProcessIdentity> {
        use std::os::unix::fs::OpenOptionsExt;
        let path = self.proc_root.join(pid.to_string());
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path.join("stat"))?;
        ensure!(
            file.metadata()?.is_file(),
            "Invalid native-read process record"
        );
        let mut bytes = Vec::new();
        file.take((MAX_STATUS_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= MAX_STATUS_BYTES,
            "The native-read process record is too large"
        );
        let stat = std::str::from_utf8(&bytes)?;
        let (identifier, _) = stat
            .split_once(" (")
            .context("Invalid native-read process framing")?;
        ensure!(
            identifier == pid.to_string(),
            "The native-read process identifier changed"
        );
        let (_, tail) = stat
            .rsplit_once(") ")
            .context("Invalid native-read process framing")?;
        let fields = tail.split_ascii_whitespace().collect::<Vec<_>>();
        ensure!(
            fields.len() >= 20 && !matches!(fields[0], "Z" | "X" | "x"),
            "The native-read process is not alive"
        );
        let parent = parse_pid(fields[1])?;
        let group = parse_pid(fields[2])?;
        let session = parse_pid(fields[3])?;
        let birth = fields[19].parse::<u64>()?;
        let executable = fs::read_link(path.join("exe"))?;
        ensure!(
            executable.is_absolute()
                && !executable
                    .as_os_str()
                    .to_string_lossy()
                    .ends_with(" (deleted)"),
            "The native-read executable changed"
        );
        Ok(ProcessIdentity {
            pid,
            parent,
            group,
            session,
            birth,
            executable,
        })
    }
}

fn canonical_pid_record(raw: &str) -> Result<u32> {
    let value = raw
        .strip_suffix('\n')
        .context("Invalid portrait process marker framing")?;
    let pid = parse_pid(value)?;
    ensure!(
        raw == format!("{pid}\n"),
        "Invalid portrait process marker framing"
    );
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    const NONCE: &str = "0123456789abcdef0123456789abcdef";

    struct Fixture {
        temporary: tempfile::TempDir,
        writer: Writer,
    }

    impl Fixture {
        fn new() -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let lock = temporary.path().join("lock");
            fs::create_dir(&lock).unwrap();
            fs::set_permissions(&lock, fs::Permissions::from_mode(0o700)).unwrap();
            let writer = Writer {
                records: PrivateDirectory::open(&lock).unwrap(),
                generation: 7,
                nonce: NONCE.into(),
                helper: 708,
                proc_root: temporary.path().join("proc"),
            };
            let fixture = Self { temporary, writer };
            for (pid, parent, group) in [
                (701, 700, 701),
                (704, 701, 704),
                (705, 704, 704),
                (706, 705, 704),
                (707, 706, 704),
                (708, 707, 704),
            ] {
                fixture.process(pid, parent, group, pid as u64 * 100);
            }
            for (name, value) in [
                ("pid", "701".into()),
                ("nonce", NONCE.into()),
                ("generation", format!("7:{NONCE}")),
                ("native-read-go", format!("704:9:{NONCE}")),
                ("native-epdc-mode", format!("7:{NONCE}:Y8INV")),
            ] {
                fixture.record(name, &value);
            }
            fixture
        }

        fn process(&self, pid: u32, parent: u32, group: u32, birth: u64) {
            let directory = self.writer.proc_root.join(pid.to_string());
            fs::create_dir_all(&directory).unwrap();
            let mut fields = vec!["0".to_owned(); 20];
            fields[0] = "S".into();
            fields[1] = parent.to_string();
            fields[2] = group.to_string();
            fields[3] = group.to_string();
            fields[19] = birth.to_string();
            fs::write(
                directory.join("stat"),
                format!("{pid} (fixture) {}\n", fields.join(" ")),
            )
            .unwrap();
            let executable = directory.join("exe");
            if fs::symlink_metadata(&executable).is_ok() {
                fs::remove_file(&executable).unwrap();
            }
            symlink(
                if pid == self.writer.helper {
                    "/fixture/bilicomics"
                } else {
                    "/fixture/sh"
                },
                executable,
            )
            .unwrap();
        }

        fn record(&self, name: &str, value: &str) {
            let path = self.writer.records.path.join(name);
            fs::write(&path, format!("{value}\n")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }

        fn resume(&self, status: &str) {
            self.process(702, 701, 701, 70200);
            self.process(703, 702, 701, 70300);
            for (name, value) in [
                ("application-pid", "703".into()),
                ("lifecycle-request", format!("7:{NONCE}:1:suspend")),
                ("lifecycle-ready", format!("7:{NONCE}")),
                ("lifecycle-ack", format!("7:{NONCE}:1:suspend{status}")),
                (
                    "resume-native-epdc-mode",
                    format!("7:{NONCE}:2:FRAMEBUFFER"),
                ),
                (CAPABILITY_RECORD, format!("7:{NONCE}:703:framebuffer8")),
            ] {
                self.record(name, &value);
            }
        }

        fn assert_no_publication(&self, publication: Publication) {
            let (target, pending) = publication.names();
            assert!(fs::symlink_metadata(self.writer.records.path.join(target)).is_err());
            assert!(fs::symlink_metadata(self.writer.records.path.join(pending)).is_err());
        }

        fn read_permit(&self) -> Result<NativeReadPermit> {
            NativeReadPermit::from_writer(Writer {
                records: PrivateDirectory::open(&self.writer.records.path)?,
                generation: self.writer.generation,
                nonce: self.writer.nonce.clone(),
                helper: self.writer.helper,
                proc_root: self.writer.proc_root.clone(),
            })
        }
    }

    #[test]
    fn read_permission_precedes_mode_publication_and_creates_no_sidecars() {
        let fixture = Fixture::new();
        fs::remove_file(fixture.writer.records.path.join("native-epdc-mode")).unwrap();
        let before = fs::read_dir(&fixture.writer.records.path).unwrap().count();
        let permit = fixture.read_permit().unwrap();
        permit.recheck().unwrap();
        assert_eq!(fs::read_dir(&fixture.writer.records.path).unwrap().count(), before);
        fixture.assert_no_publication(Publication::parse("U:3").unwrap());
        fixture.assert_no_publication(Publication::Resume(2));
        assert!(fixture.writer.publish(Publication::parse("U:3").unwrap()).is_err());
    }

    #[test]
    fn resumed_read_permission_requires_suspend_release_but_not_the_future_mode_record() {
        for status in [":ok", ":checkpoint-error", ""] {
            let fixture = Fixture::new();
            fixture.resume(status);
            fs::remove_file(fixture.writer.records.path.join("resume-native-epdc-mode")).unwrap();
            let before = fs::read_dir(&fixture.writer.records.path).unwrap().count();
            fixture.read_permit().unwrap().recheck().unwrap();
            assert_eq!(fs::read_dir(&fixture.writer.records.path).unwrap().count(), before);
            fixture.assert_no_publication(Publication::Resume(2));
            assert!(fixture.writer.publish(Publication::Resume(2)).is_err());
        }
        for (name, value) in [
            ("application-pid", "999".into()),
            ("lifecycle-request", format!("7:{NONCE}:1:resume")),
            ("lifecycle-request", format!("7:{NONCE}:01:suspend")),
            ("lifecycle-request", format!("7:{NONCE}:0:suspend")),
            ("lifecycle-request", format!("8:{NONCE}:1:suspend")),
            ("lifecycle-ready", format!("8:{NONCE}")),
            ("lifecycle-ack", format!("7:{NONCE}:1:suspend:pending")),
            ("lifecycle-ack", format!("7:{NONCE}:2:suspend:ok")),
        ] {
            let fixture = Fixture::new();
            fixture.resume(":ok");
            fixture.record(name, &value);
            assert!(fixture.read_permit().is_err(), "{name}");
        }
    }

    #[test]
    fn read_permission_rechecks_launch_sources_ancestry_birth_and_executable() {
        for (name, value) in [
            ("generation", format!("8:{NONCE}")),
            ("nonce", "ffffffffffffffffffffffffffffffff".into()),
            ("pid", "702".into()),
            ("native-read-go", format!("704:10:{NONCE}")),
            ("application-pid", "703".into()),
            ("lifecycle-request", format!("7:{NONCE}:1:suspend")),
        ] {
            let fixture = Fixture::new();
            let permit = fixture.read_permit().unwrap();
            fixture.record(name, &value);
            assert!(permit.recheck().is_err(), "{name}");
            fixture.assert_no_publication(Publication::parse("U:3").unwrap());
        }
        for (pid, parent, group, birth) in [
            (704, 700, 704, 70400),
            (704, 701, 701, 70400),
            (708, 701, 704, 70800),
            (708, 707, 701, 70800),
            (708, 707, 704, 70801),
            (707, 706, 704, 70701),
            (701, 700, 701, 70101),
        ] {
            let fixture = Fixture::new();
            let permit = fixture.read_permit().unwrap();
            fixture.process(pid, parent, group, birth);
            assert!(permit.recheck().is_err(), "{pid}:{parent}:{group}:{birth}");
        }
        let fixture = Fixture::new();
        let permit = fixture.read_permit().unwrap();
        let executable = fixture.writer.proc_root.join("708/exe");
        fs::remove_file(&executable).unwrap();
        symlink("/fixture/replaced-bilicomics", executable).unwrap();
        assert!(permit.recheck().is_err());
    }

    #[test]
    fn read_permission_rejects_revocation_unsafe_markers_and_replaced_directory() {
        let fixture = Fixture::new();
        let permit = fixture.read_permit().unwrap();
        fs::remove_file(fixture.writer.records.path.join("native-read-go")).unwrap();
        assert!(permit.recheck().is_err());
        for kind in ["symlink", "hardlink", "public"] {
            let fixture = Fixture::new();
            let permission = fixture.writer.records.path.join("native-read-go");
            let value = fs::read_to_string(&permission).unwrap();
            fs::remove_file(&permission).unwrap();
            let outside = fixture.temporary.path().join("permission");
            fs::write(&outside, &value).unwrap();
            fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
            match kind {
                "symlink" => symlink(&outside, &permission).unwrap(),
                "hardlink" => fs::hard_link(&outside, &permission).unwrap(),
                _ => {
                    fs::write(&permission, &value).unwrap();
                    fs::set_permissions(&permission, fs::Permissions::from_mode(0o644)).unwrap();
                }
            }
            assert!(fixture.read_permit().is_err(), "{kind}");
            assert_eq!(fs::read_to_string(outside).unwrap(), value);
        }
        let fixture = Fixture::new();
        let permit = fixture.read_permit().unwrap();
        fs::rename(&fixture.writer.records.path, fixture.temporary.path().join("old-lock")).unwrap();
        fs::create_dir(&fixture.writer.records.path).unwrap();
        fs::set_permissions(&fixture.writer.records.path, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(permit.recheck().is_err());
        assert_eq!(fs::read_dir(&fixture.writer.records.path).unwrap().count(), 0);
    }

    #[test]
    fn startup_portrait_publication_is_private_exclusive_and_does_not_change_epdc() {
        let fixture = Fixture::new();
        let publication = Publication::parse("D:1").unwrap();
        fixture.writer.publish(publication).unwrap();
        let target = fixture.writer.records.path.join(PORTRAIT_RECORD);
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            format!("7:{NONCE}:D:1\n")
        );
        let metadata = fs::metadata(&target).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o600);
        assert_eq!(metadata.nlink(), 1);
        assert!(!fixture.writer.records.path.join(STARTUP_PENDING).exists());
        assert_eq!(
            fixture.writer.records.read("native-epdc-mode").unwrap(),
            format!("7:{NONCE}:Y8INV\n")
        );
        assert!(
            fixture
                .writer
                .publish(Publication::parse("U:3").unwrap())
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(target).unwrap(),
            format!("7:{NONCE}:D:1\n")
        );
    }

    #[test]
    fn resume_portrait_publication_preserves_checkpoint_error_ram_continuation() {
        for status in [":ok", ":checkpoint-error", ""] {
            let fixture = Fixture::new();
            fixture.resume(status);
            fixture
                .writer
                .publish(Publication::parse("CALIBRATED:2").unwrap())
                .unwrap();
            assert_eq!(
                fixture.writer.records.read(RESUME_PORTRAIT_RECORD).unwrap(),
                format!("7:{NONCE}:2:CALIBRATED\n")
            );
        }
    }

    #[test]
    fn portrait_publication_rejects_wrong_launch_reader_permission_and_process_ancestry() {
        for (name, value) in [
            ("nonce", "ffffffffffffffffffffffffffffffff"),
            ("generation", "8:0123456789abcdef0123456789abcdef"),
            ("native-read-go", "704:9:ffffffffffffffffffffffffffffffff"),
            ("native-read-go", "704:09:0123456789abcdef0123456789abcdef"),
        ] {
            let fixture = Fixture::new();
            fixture.record(name, value);
            let publication = Publication::parse("U:3").unwrap();
            assert!(fixture.writer.publish(publication).is_err(), "{name}");
            fixture.assert_no_publication(publication);
        }
        for (parent, group) in [(701, 704), (707, 701)] {
            let fixture = Fixture::new();
            fixture.process(708, parent, group, 70800);
            let publication = Publication::parse("U:3").unwrap();
            assert!(fixture.writer.publish(publication).is_err());
            fixture.assert_no_publication(publication);
        }
    }

    #[test]
    fn resume_portrait_publication_requires_current_suspend_bridge_and_capability() {
        for (name, value) in [
            ("lifecycle-request", format!("7:{NONCE}:2:resume")),
            ("lifecycle-ack", format!("7:{NONCE}:1:suspend:unknown")),
            ("lifecycle-ready", format!("8:{NONCE}")),
            (CAPABILITY_RECORD, format!("7:{NONCE}:999:framebuffer8")),
        ] {
            let fixture = Fixture::new();
            fixture.resume(":ok");
            fixture.record(name, &value);
            let publication = Publication::parse("CALIBRATED:2").unwrap();
            assert!(fixture.writer.publish(publication).is_err(), "{name}");
            fixture.assert_no_publication(publication);
        }
    }

    #[test]
    fn portrait_publication_never_opens_or_replaces_unowned_pending_or_target_leaves() {
        for name in [PORTRAIT_RECORD, STARTUP_PENDING] {
            for kind in [
                "regular",
                "symlink",
                "dangling",
                "hardlink",
                "fifo",
                "directory",
                "directory-link",
            ] {
                let fixture = Fixture::new();
                let leaf = fixture.writer.records.path.join(name);
                let outside = fixture.temporary.path().join("outside");
                fs::write(&outside, "untouched\n").unwrap();
                fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
                match kind {
                    "regular" => fixture.record(name, "unowned"),
                    "symlink" => symlink(&outside, &leaf).unwrap(),
                    "dangling" => {
                        symlink(fixture.temporary.path().join("uncreated"), &leaf).unwrap()
                    }
                    "hardlink" => fs::hard_link(&outside, &leaf).unwrap(),
                    "fifo" => {
                        use std::{ffi::CString, os::unix::ffi::OsStrExt};
                        let pathname = CString::new(leaf.as_os_str().as_bytes()).unwrap();
                        // SAFETY: this leaf belongs to the isolated test directory.
                        assert_eq!(unsafe { libc::mkfifo(pathname.as_ptr(), 0o600) }, 0);
                    }
                    "directory" => fs::create_dir(&leaf).unwrap(),
                    _ => symlink(fixture.temporary.path(), &leaf).unwrap(),
                }
                assert!(
                    fixture
                        .writer
                        .publish(Publication::parse("U:3").unwrap())
                        .is_err(),
                    "{name}:{kind}"
                );
                assert!(fs::symlink_metadata(&leaf).is_ok());
                assert_eq!(fs::read_to_string(&outside).unwrap(), "untouched\n");
                assert!(!fixture.temporary.path().join("uncreated").exists());
                assert!(!fixture.temporary.path().join(STARTUP_PENDING).exists());
            }
        }
    }

    #[test]
    fn failed_portrait_publication_retains_pending_evidence_without_a_named_unlink() {
        let fixture = Fixture::new();
        let publication = Publication::parse("U:3").unwrap();
        let ownership = fixture.writer.ownership(publication).unwrap();
        let calls = std::cell::Cell::new(0);
        let (target, pending) = publication.names();
        assert!(
            fixture
                .writer
                .records
                .publish_private_marker(
                    target,
                    pending,
                    &publication.content(&fixture.writer.header()),
                    true,
                    || {
                        calls.set(calls.get() + 1);
                        if calls.get() == 2 {
                            fixture.process(708, 707, 704, 70801);
                        }
                        ensure!(
                            fixture.writer.ownership(publication)? == ownership,
                            "Injected changed helper birth"
                        );
                        Ok(())
                    }
                )
                .is_err()
        );
        assert_eq!(calls.get(), 2);
        assert!(!fixture.writer.records.path.join(target).exists());
        assert_eq!(
            fs::read_to_string(fixture.writer.records.path.join(pending)).unwrap(),
            publication.content(&fixture.writer.header())
        );
    }

    #[test]
    fn portrait_publication_rejects_directory_replacement_and_invalid_typed_payloads() {
        for value in [
            "U:03",
            "L:3",
            "D:4",
            "FIXED:3\n",
            "CALIBRATED:0",
            "CALIBRATED:1",
            "CALIBRATED:02",
            "CALIBRATED:2:extra",
            "../outside",
        ] {
            assert!(Publication::parse(value).is_err(), "{value}");
        }
        let fixture = Fixture::new();
        let old = fixture.temporary.path().join("old-lock");
        fs::rename(&fixture.writer.records.path, &old).unwrap();
        fs::create_dir(&fixture.writer.records.path).unwrap();
        fs::set_permissions(
            &fixture.writer.records.path,
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        assert!(
            fixture
                .writer
                .publish(Publication::parse("U:3").unwrap())
                .is_err()
        );
        assert_eq!(
            fs::read_dir(&fixture.writer.records.path).unwrap().count(),
            0
        );
        assert!(!old.join(PORTRAIT_RECORD).exists());
    }
}

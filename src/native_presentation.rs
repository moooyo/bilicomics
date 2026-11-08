//! Consume native display snapshots from the owning launcher only.
//!
//! The snapshot is output compensation, not a request to change the system theme.

use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result, bail, ensure};

const MAX_MARKER_BYTES: usize = 1024;
const MAX_STATUS_BYTES: usize = 64 * 1024;
const CAPABILITY_RECORD: &str = "native-output-capability";
const PORTRAIT_RECORD: &str = "native-portrait-orientation";
const RESUME_PORTRAIT_RECORD: &str = "resume-native-portrait-orientation";
#[cfg(unix)]
const CAPABILITY_PENDING: &str = "native-output-capability.pending";

#[cfg(target_os = "linux")]
mod portrait_writer;

/// Publish one typed portrait sidecar from the owner's bounded native-read group.
/// This internal helper performs no display, input, or firmware operation.
pub fn publish_native_portrait_orientation(value: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        portrait_writer::publish(value)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = value;
        bail!("Portrait publication requires the Linux native launcher")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeOutputMode {
    Y8,
    Y8Inv,
}

impl NativeOutputMode {
    pub fn inverted(self) -> bool {
        self == Self::Y8Inv
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "Y8" => Ok(Self::Y8),
            "Y8INV" => Ok(Self::Y8Inv),
            _ => bail!("The native display mode is unknown"),
        }
    }
}

/// A current resume request, never a cached output mode from startup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeResumePresentation {
    Observed(NativeOutputMode),
    FramebufferProbe,
}

/// Native portrait direction, independent of output polarity and app rotation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativePortraitDirection {
    Up,
    Down,
}

/// A same-launch direction/GETVAR pair captured before native display takeover.
///
/// Absolute kernel rotation codes never identify Up or Down across devices.
/// This baseline can resolve a wake only alongside a current CALIBRATED request
/// and a fresh read-only kernel observation from that exact callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativePortraitCalibration {
    direction: NativePortraitDirection,
    framebuffer_rotation: u32,
    fixed: bool,
}

impl NativePortraitCalibration {
    fn parse(value: &str) -> Result<Self> {
        let (direction, rotation) = value
            .split_once(':')
            .context("The native portrait calibration is incomplete")?;
        let (direction, fixed) = match direction {
            "U" => (NativePortraitDirection::Up, false),
            "D" => (NativePortraitDirection::Down, false),
            "FIXED" => (NativePortraitDirection::Up, true),
            _ => bail!("The native portrait direction is unknown"),
        };
        let framebuffer_rotation = match rotation {
            "0" => 0,
            "1" => 1,
            "2" => 2,
            "3" => 3,
            _ => bail!("The native framebuffer rotation is unknown"),
        };
        Ok(Self {
            direction,
            framebuffer_rotation,
            fixed,
        })
    }

    fn resolve(self, observed_rotation: u32, resuming: bool) -> Result<NativePortraitDirection> {
        ensure!(
            observed_rotation <= 3,
            "The observed framebuffer rotation is unknown"
        );
        let delta = (observed_rotation + 4 - self.framebuffer_rotation) % 4;
        if !resuming || self.fixed {
            ensure!(
                delta == 0,
                "Native rotation changed outside the calibrated portrait transition"
            );
            return Ok(self.direction);
        }
        match delta {
            0 => Ok(self.direction),
            2 => Ok(match self.direction {
                NativePortraitDirection::Up => NativePortraitDirection::Down,
                NativePortraitDirection::Down => NativePortraitDirection::Up,
            }),
            _ => bail!("A native landscape transition is not approved"),
        }
    }
}

/// One stable presentation record paired with a fresh read-only kernel sample.
pub struct NativePortraitObservation<T> {
    pub direction: NativePortraitDirection,
    pub presentation: NativeResumePresentation,
    pub framebuffer: T,
}

/// A generation-bound view of the launcher's private presentation records.
#[derive(Clone)]
pub struct NativePresentationContext {
    records: Arc<PrivateDirectory>,
    generation: u64,
    nonce: String,
    application: u32,
    proc_root: PathBuf,
    resume_sequence: Arc<AtomicU64>,
}

impl NativePresentationContext {
    pub(crate) fn new(records: Arc<PrivateDirectory>, generation: u64, nonce: String) -> Self {
        Self {
            records,
            generation,
            nonce,
            application: std::process::id(),
            proc_root: PathBuf::from("/proc"),
            resume_sequence: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Read before the first framebuffer clear or input acquisition.
    pub fn startup_mode(&self) -> Result<NativeOutputMode> {
        match self.read_presentation(None)? {
            NativeResumePresentation::Observed(mode) => Ok(mode),
            NativeResumePresentation::FramebufferProbe => {
                bail!("A framebuffer probe cannot replace the startup display mode")
            }
        }
    }

    /// Require a new protected calibration before the first native I/O.
    ///
    /// Older launchers publish only EPDC records. Their polarity records remain
    /// readable, but cannot silently substitute for this orientation contract.
    pub fn startup_portrait_calibration(&self) -> Result<NativePortraitCalibration> {
        let before = self.binding(false)?;
        let raw = self.records.read(PORTRAIT_RECORD)?;
        let calibration = self.parse_portrait_record(&raw)?;
        ensure!(
            self.records.read(PORTRAIT_RECORD)? == raw,
            "The native portrait calibration changed while reading"
        );
        ensure!(
            before.agrees_with(&self.binding(false)?),
            "The launcher identity changed while reading its portrait calibration"
        );
        self.records.ensure_current()?;
        Ok(calibration)
    }

    /// Match the pre-takeover baseline to fresh metadata before initial I/O.
    pub fn observe_startup_portrait<T>(
        &self,
        observe: impl FnOnce() -> Result<(u32, T)>,
    ) -> Result<NativePortraitObservation<T>> {
        self.observe_portrait(None, observe)
    }

    /// Resolve the active wake from fresh GETVAR, without querying the paused WM.
    ///
    /// CALIBRATED grants only this callback permission to use its same-launch
    /// baseline; it carries no current direction. The kernel sample must be
    /// read-only and layout-checked before any input or display is reacquired.
    /// Recheck ownership, records, and sequence after the supplied observation.
    pub fn observe_resume_portrait<T>(
        &self,
        observe: impl FnOnce() -> Result<(u32, T)>,
    ) -> Result<NativePortraitObservation<T>> {
        let sequence = self.resume_sequence.load(Ordering::Acquire);
        ensure!(
            sequence > 0,
            "No matching portrait resume transition is being dispatched"
        );
        let result = self.observe_portrait(Some(sequence), observe)?;
        ensure!(
            self.resume_sequence.load(Ordering::Acquire) == sequence,
            "The resume transition changed while observing its portrait direction"
        );
        Ok(result)
    }

    /// Advertise permission to attempt a fresh framebuffer probe on each wake.
    ///
    /// This record carries no polarity and cannot authorize input acquisition.
    /// Startup may precede the application's PID marker, but its actual process
    /// ancestry must already identify this launcher's application and runner.
    /// A failed post-publication check does not imply the record was never visible.
    pub fn publish_framebuffer_capability(&self, supported: bool) -> Result<()> {
        #[cfg(unix)]
        {
            let before = self.binding(false)?;
            let legacy = self.capability_record(false);
            let framebuffer = self.capability_record(true);
            let content = if supported { &framebuffer } else { &legacy };
            self.records.publish_capability(content, || {
                ensure!(before.agrees_with(&self.binding(false)?), "The launcher identity changed while publishing its output capability");
                if let Some(existing) = self.records.read_optional(CAPABILITY_RECORD)? {
                    ensure!(existing == legacy || existing == framebuffer, "The existing output capability belongs to another application or is invalid");
                }
                Ok(())
            })?;
            ensure!(
                self.records.read(CAPABILITY_RECORD)? == *content,
                "The output capability changed after publication"
            );
            ensure!(
                before.agrees_with(&self.binding(false)?),
                "The launcher identity changed after publishing its output capability"
            );
            self.records.ensure_current()?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = supported;
            bail!("Output capability publication requires descriptor-relative private files")
        }
    }

    /// Read during the currently dispatched resume callback, before resuming I/O.
    pub fn resume_mode(&self) -> Result<NativeOutputMode> {
        match self.resume_presentation_request()? {
            NativeResumePresentation::Observed(mode) => Ok(mode),
            NativeResumePresentation::FramebufferProbe => {
                bail!("The current resume requires a fresh framebuffer probe")
            }
        }
    }

    /// Resolve only the active resume request; a probe must be performed afresh.
    pub fn resume_presentation_request(&self) -> Result<NativeResumePresentation> {
        let sequence = self.resume_sequence.load(Ordering::Acquire);
        ensure!(
            sequence > 0,
            "No matching resume transition is being dispatched"
        );
        let presentation = self.read_presentation(Some(sequence))?;
        ensure!(
            self.resume_sequence.load(Ordering::Acquire) == sequence,
            "The resume transition changed while reading its display mode"
        );
        Ok(presentation)
    }

    pub(crate) fn set_resume_sequence(&self, sequence: u64) {
        self.resume_sequence.store(sequence, Ordering::Release);
    }

    fn header(&self) -> String {
        format!("{}:{}", self.generation, self.nonce)
    }

    fn capability_record(&self, supported: bool) -> String {
        format!(
            "{}:{}:{}\n",
            self.header(),
            self.application,
            if supported { "framebuffer8" } else { "legacy" }
        )
    }

    fn parse_portrait_record(&self, raw: &str) -> Result<NativePortraitCalibration> {
        let record = raw
            .strip_suffix('\n')
            .context("The native portrait calibration requires exact record framing")?;
        let prefix = format!("{}:", self.header());
        let value = record
            .strip_prefix(&prefix)
            .context("The native portrait calibration belongs to another launch")?;
        NativePortraitCalibration::parse(value)
    }

    fn observe_portrait<T>(
        &self,
        sequence: Option<u64>,
        observe: impl FnOnce() -> Result<(u32, T)>,
    ) -> Result<NativePortraitObservation<T>> {
        let before = self.binding(sequence.is_some())?;
        let request = sequence.map(|value| format!("{}:{value}:resume", self.header()));
        let resume_marker = sequence.map(|value| format!("{}:{value}:CALIBRATED\n", self.header()));
        if let Some(expected) = &request {
            ensure!(
                self.record("lifecycle-request")? == *expected,
                "The portrait observation does not match the current resume request"
            );
        }
        if let Some(expected) = &resume_marker {
            ensure!(
                self.records.read(RESUME_PORTRAIT_RECORD)? == *expected,
                "The portrait observation is not authorized by the current resume request"
            );
        }
        let raw = self.records.read(PORTRAIT_RECORD)?;
        let calibration = self.parse_portrait_record(&raw)?;
        // Include the existing polarity/capability contract in this stable
        // observation so it cannot be mixed with another resume transition.
        let presentation = self.read_presentation(sequence)?;
        let (rotation, framebuffer) = observe()?;
        let direction = calibration.resolve(rotation, sequence.is_some())?;
        ensure!(
            self.records.read(PORTRAIT_RECORD)? == raw,
            "The native portrait calibration changed during kernel observation"
        );
        if let Some(expected) = &resume_marker {
            ensure!(
                self.records.read(RESUME_PORTRAIT_RECORD)? == *expected,
                "The portrait resume permission changed during kernel observation"
            );
        }
        if let Some(expected) = &request {
            ensure!(
                self.record("lifecycle-request")? == *expected,
                "The resume request changed during portrait observation"
            );
        }
        ensure!(
            self.read_presentation(sequence)? == presentation,
            "The native presentation changed during portrait observation"
        );
        ensure!(
            before.agrees_with(&self.binding(sequence.is_some())?),
            "The launcher identity changed during portrait observation"
        );
        self.records.ensure_current()?;
        Ok(NativePortraitObservation {
            direction,
            presentation,
            framebuffer,
        })
    }

    fn read_presentation(&self, sequence: Option<u64>) -> Result<NativeResumePresentation> {
        let before = self.binding(sequence.is_some())?;
        let request = sequence.map(|value| format!("{}:{value}:resume", self.header()));
        if let Some(expected) = &request {
            ensure!(
                self.record("lifecycle-request")? == *expected,
                "The native display snapshot does not match the current resume request"
            );
        }
        let name = if sequence.is_some() {
            "resume-native-epdc-mode"
        } else {
            "native-epdc-mode"
        };
        let raw_record = self.records.read(name)?;
        let record = single_record(&raw_record)?;
        let prefix = match sequence {
            Some(value) => format!("{}:{value}:", self.header()),
            None => format!("{}:", self.header()),
        };
        let value = record
            .strip_prefix(&prefix)
            .context("The native display snapshot belongs to another launch or transition")?;
        let presentation = if value == "FRAMEBUFFER" && sequence.is_some() {
            ensure!(
                raw_record == format!("{prefix}FRAMEBUFFER\n"),
                "Invalid framebuffer probe request framing"
            );
            ensure!(
                self.records.read(CAPABILITY_RECORD)? == self.capability_record(true),
                "The current application did not advertise framebuffer probe capability"
            );
            NativeResumePresentation::FramebufferProbe
        } else {
            NativeResumePresentation::Observed(NativeOutputMode::parse(value)?)
        };
        ensure!(
            self.records.read(name)? == raw_record,
            "The native display snapshot changed while reading"
        );
        if let Some(expected) = &request {
            ensure!(
                self.record("lifecycle-request")? == *expected,
                "The resume request changed while reading its display mode"
            );
        }
        let after = self.binding(sequence.is_some())?;
        ensure!(
            before.agrees_with(&after),
            "The launcher identity changed while reading its display mode"
        );
        if presentation == NativeResumePresentation::FramebufferProbe {
            ensure!(
                self.records.read(CAPABILITY_RECORD)? == self.capability_record(true),
                "The framebuffer probe capability changed while reading"
            );
        }
        self.records.ensure_current()?;
        Ok(presentation)
    }

    fn record(&self, name: &str) -> Result<String> {
        Ok(single_record(&self.records.read(name)?)?.to_owned())
    }

    fn binding(&self, require_application: bool) -> Result<BindingIdentity> {
        self.records.ensure_current()?;
        ensure!(
            self.record("generation")? == self.header(),
            "The native display generation no longer matches"
        );
        ensure!(
            self.record("nonce")? == self.nonce,
            "The native display token no longer matches"
        );
        let master = parse_pid(&self.record("pid")?)?;
        let application_marker = self
            .records
            .read_optional("application-pid")?
            .map(|value| single_record(&value).and_then(parse_pid))
            .transpose()?;
        ensure!(
            !require_application || application_marker.is_some(),
            "The resumed application identity is missing"
        );
        ensure!(
            application_marker.is_none_or(|value| value == self.application),
            "The native display records belong to another application"
        );
        #[cfg(target_os = "linux")]
        let runner = {
            let runner = process_parent(&self.proc_root, self.application)?;
            ensure!(
                self.application != runner && self.application != master && runner != master,
                "Invalid launcher process ancestry"
            );
            ensure!(
                process_parent(&self.proc_root, runner)? == master
                    && process_parent(&self.proc_root, self.application)? == runner,
                "The application is not owned by the native display supervisor"
            );
            runner
        };
        #[cfg(not(target_os = "linux"))]
        let runner = {
            ensure!(
                application_marker == Some(self.application),
                "The application identity is unavailable on this platform"
            );
            0
        };
        Ok(BindingIdentity {
            master,
            runner,
            application_marker,
        })
    }
}

#[derive(Eq, PartialEq)]
struct BindingIdentity {
    master: u32,
    runner: u32,
    application_marker: Option<u32>,
}

impl BindingIdentity {
    fn agrees_with(&self, after: &Self) -> bool {
        self.master == after.master
            && self.runner == after.runner
            && self
                .application_marker
                .is_none_or(|value| after.application_marker == Some(value))
    }
}

fn parse_pid(value: &str) -> Result<u32> {
    ensure!(
        !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()),
        "Invalid launcher process identifier"
    );
    let value = value.parse::<u32>()?;
    ensure!(value > 0, "Invalid launcher process identifier");
    Ok(value)
}

pub(crate) fn single_record(value: &str) -> Result<&str> {
    let value = value.strip_suffix('\n').unwrap_or(value);
    let value = value.strip_suffix('\r').unwrap_or(value);
    ensure!(
        !value.is_empty()
            && !value
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace()),
        "Invalid lifecycle record framing"
    );
    Ok(value)
}

/// Keep the opened directory pinned so a path replacement cannot mix records.
pub(crate) struct PrivateDirectory {
    path: PathBuf,
    directory: File,
    identity: Metadata,
}

impl PrivateDirectory {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        ensure!(
            path.is_absolute(),
            "The lifecycle directory must be absolute"
        );
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Directory handles require backup semantics; inspect the reparse
            // point itself instead of following a replaced directory link.
            options.custom_flags(0x0200_0000 | 0x0020_0000);
        }
        let directory = options
            .open(path)
            .context("Open private lifecycle directory")?;
        let identity = directory.metadata()?;
        ensure!(
            private_directory(&identity),
            "The lifecycle directory must be owned and private"
        );
        let result = Self {
            path: path.to_owned(),
            directory,
            identity,
        };
        result.ensure_current()?;
        Ok(result)
    }

    pub(crate) fn ensure_current(&self) -> Result<()> {
        let current = fs::symlink_metadata(&self.path)?;
        ensure!(
            private_directory(&current) && same_identity(&self.identity, &current),
            "The private lifecycle directory was replaced"
        );
        Ok(())
    }

    pub(crate) fn read_optional(&self, name: &str) -> Result<Option<String>> {
        match self.read(name) {
            Ok(value) => Ok(Some(value)),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn read(&self, name: &str) -> Result<String> {
        ensure!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
            "Invalid lifecycle marker name"
        );
        self.ensure_current()?;
        let mut file = self.open_record(name)?;
        let before = file.metadata()?;
        ensure!(
            private_regular(&before) && before.len() <= MAX_MARKER_BYTES as u64,
            "Unsafe lifecycle marker"
        );
        let mut bytes = Vec::new();
        file.by_ref()
            .take((MAX_MARKER_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        let after = file.metadata()?;
        ensure!(
            bytes.len() <= MAX_MARKER_BYTES
                && bytes.len() as u64 == before.len()
                && private_regular(&after)
                && stable_file(&before, &after),
            "The lifecycle marker changed while reading"
        );
        let named = self.open_record(name)?.metadata()?;
        ensure!(
            private_regular(&named) && stable_file(&after, &named),
            "The lifecycle marker was replaced while reading"
        );
        self.ensure_current()?;
        Ok(String::from_utf8(bytes)?)
    }

    #[cfg(unix)]
    fn publish_capability(&self, content: &str, validate: impl Fn() -> Result<()>) -> Result<()> {
        self.publish_private_marker(
            CAPABILITY_RECORD,
            CAPABILITY_PENDING,
            content,
            false,
            validate,
        )
    }

    #[cfg(unix)]
    fn publish_private_marker(
        &self,
        target_name: &str,
        pending_name: &str,
        content: &str,
        require_absent: bool,
        validate: impl Fn() -> Result<()>,
    ) -> Result<()> {
        use std::{
            ffi::CString,
            io::Write,
            os::fd::{AsRawFd, FromRawFd},
        };
        ensure!(
            content.len() <= MAX_MARKER_BYTES,
            "The private marker is too large"
        );
        self.ensure_current()?;
        validate()?;
        if require_absent {
            ensure!(
                self.read_optional(target_name)?.is_none(),
                "The portrait publication target already exists"
            );
        }
        let pending = CString::new(pending_name)?;
        let target = CString::new(target_name)?;
        // SAFETY: the pinned directory fd and both terminated leaf names remain
        // live. O_EXCL does not follow or truncate an existing pending marker.
        let descriptor = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                pending.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: this new descriptor is transferred into exactly one File owner.
        let mut file = unsafe { File::from_raw_fd(descriptor) };
        let result = (|| -> Result<()> {
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            let created = file.metadata()?;
            ensure!(
                private_regular(&created) && created.len() == content.len() as u64,
                "Unsafe pending private marker"
            );
            validate()?;
            self.ensure_current()?;
            let named = self.open_record(pending_name)?.metadata()?;
            ensure!(
                private_regular(&named) && stable_file(&created, &named),
                "The pending private marker changed before publication"
            );
            if require_absent {
                ensure!(
                    self.read_optional(target_name)?.is_none(),
                    "The portrait publication target appeared before commit"
                );
                #[cfg(target_os = "linux")]
                {
                    // SAFETY: both leaf names and the pinned directory fd remain
                    // valid. RENAME_NOREPLACE never follows or replaces a target
                    // created between the absence check and this atomic commit.
                    if unsafe {
                        libc::syscall(
                            libc::SYS_renameat2,
                            self.directory.as_raw_fd(),
                            pending.as_ptr(),
                            self.directory.as_raw_fd(),
                            target.as_ptr(),
                            libc::RENAME_NOREPLACE,
                        )
                    } == -1
                    {
                        return Err(std::io::Error::last_os_error().into());
                    }
                }
                #[cfg(not(target_os = "linux"))]
                bail!("Exclusive private marker publication requires Linux renameat2");
            } else {
                // SAFETY: both paths are relative to the same pinned directory.
                ensure!(
                    unsafe {
                        libc::renameat(
                            self.directory.as_raw_fd(),
                            pending.as_ptr(),
                            self.directory.as_raw_fd(),
                            target.as_ptr(),
                        )
                    } == 0,
                    "Publish the private output capability"
                );
            }
            let published = self.open_record(target_name)?.metadata()?;
            // A rename may change ctime, so establish a fresh stable-read baseline
            // after committing instead of comparing pre-rename timestamps.
            ensure!(
                private_regular(&published)
                    && same_identity(&created, &published)
                    && created.len() == published.len(),
                "The published private marker was replaced"
            );
            self.ensure_current()?;
            Ok(())
        })();
        // Exclusive portrait failures retain their pending evidence for the
        // owner after this bounded reader has stopped. A named unlink after an
        // inode check cannot atomically exclude a same-UID replacement race.
        // Preserve the existing capability writer's separate cleanup behavior.
        if result.is_err() && !require_absent {
            self.remove_owned_marker_pending(pending_name, &file);
        }
        result
    }

    #[cfg(unix)]
    fn remove_owned_marker_pending(&self, pending_name: &str, file: &File) {
        use std::{ffi::CString, os::fd::AsRawFd};
        let Ok(owned) = file.metadata() else {
            return;
        };
        let Ok(named) = self
            .open_record(pending_name)
            .and_then(|value| Ok(value.metadata()?))
        else {
            return;
        };
        if !private_regular(&owned) || !private_regular(&named) || !same_identity(&owned, &named) {
            return;
        }
        let Ok(pending) = CString::new(pending_name) else {
            return;
        };
        // SAFETY: only the still-matching pending marker in the pinned directory
        // is removed. An unowned pre-existing pending marker is never cleaned up.
        let _ = unsafe { libc::unlinkat(self.directory.as_raw_fd(), pending.as_ptr(), 0) };
    }

    #[cfg(unix)]
    fn open_record(&self, name: &str) -> Result<File> {
        use std::{
            ffi::CString,
            os::fd::{AsRawFd, FromRawFd},
        };
        let name = CString::new(name)?;
        // SAFETY: the directory fd and terminated leaf name remain valid for openat.
        // O_NONBLOCK prevents a FIFO from blocking before its type is checked.
        let descriptor = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: this new descriptor is transferred into exactly one File owner.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    #[cfg(not(unix))]
    fn open_record(&self, name: &str) -> Result<File> {
        let path = self.path.join(name);
        let before = fs::symlink_metadata(&path)?;
        ensure!(
            before.is_file() && !before.file_type().is_symlink(),
            "Unsafe lifecycle marker"
        );
        Ok(File::open(path)?)
    }
}

fn private_directory(metadata: &Metadata) -> bool {
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no pointer arguments and only reads process identity.
        metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn private_regular(metadata: &Metadata) -> bool {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no pointer arguments and only reads process identity.
        metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o077 == 0
            && metadata.nlink() == 1
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn same_identity(left: &Metadata, right: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(not(unix))]
    {
        left.created().ok() == right.created().ok()
    }
}

fn stable_file(left: &Metadata, right: &Metadata) -> bool {
    if !same_identity(left, right) || left.len() != right.len() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.mtime() == right.mtime()
            && left.mtime_nsec() == right.mtime_nsec()
            && left.ctime() == right.ctime()
            && left.ctime_nsec() == right.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        left.modified().ok() == right.modified().ok()
    }
}

#[cfg(target_os = "linux")]
fn process_parent(proc_root: &Path, process: u32) -> Result<u32> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(proc_root.join(process.to_string()).join("status"))?;
    ensure!(
        file.metadata()?.is_file(),
        "Invalid process ancestry record"
    );
    let mut bytes = Vec::new();
    file.take((MAX_STATUS_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_STATUS_BYTES,
        "The process ancestry record is too large"
    );
    let status = std::str::from_utf8(&bytes)?;
    let mut parents = status.lines().filter_map(|line| line.strip_prefix("PPid:"));
    let parent = parse_pid(parents.next().context("Missing process ancestry")?.trim())?;
    ensure!(parents.next().is_none(), "Ambiguous process ancestry");
    Ok(parent)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    const NONCE: &str = "0123456789abcdef0123456789abcdef";

    struct Fixture {
        _temporary: tempfile::TempDir,
        lock: PathBuf,
        context: NativePresentationContext,
    }

    impl Fixture {
        fn new() -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let lock = temporary.path().join("lock");
            fs::create_dir(&lock).unwrap();
            fs::set_permissions(&lock, fs::Permissions::from_mode(0o700)).unwrap();
            let proc_root = temporary.path().join("proc");
            for (process, parent) in [(303, 302), (302, 301)] {
                let directory = proc_root.join(process.to_string());
                fs::create_dir_all(&directory).unwrap();
                fs::write(
                    directory.join("status"),
                    format!("Name:\tfixture\nPPid:\t{parent}\n"),
                )
                .unwrap();
            }
            let mut context = NativePresentationContext::new(
                Arc::new(PrivateDirectory::open(&lock).unwrap()),
                7,
                NONCE.into(),
            );
            context.application = 303;
            context.proc_root = proc_root;
            let result = Self {
                _temporary: temporary,
                lock,
                context,
            };
            for (name, value) in [
                ("nonce", NONCE.into()),
                ("generation", format!("7:{NONCE}")),
                ("pid", "301".into()),
                ("native-epdc-mode", format!("7:{NONCE}:Y8INV")),
            ] {
                result.record(name, &value);
            }
            result
        }

        fn record(&self, name: &str, value: &str) {
            let path = self.lock.join(name);
            fs::write(&path, format!("{value}\n")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }

        fn resume(&self, sequence: u64, mode: &str) {
            self.record("application-pid", "303");
            self.record("lifecycle-request", &format!("7:{NONCE}:{sequence}:resume"));
            self.record(
                "resume-native-epdc-mode",
                &format!("7:{NONCE}:{sequence}:{mode}"),
            );
            self.context.set_resume_sequence(sequence);
        }

        fn portrait(&self, calibration: &str) {
            self.record(PORTRAIT_RECORD, &format!("7:{NONCE}:{calibration}"));
        }

        fn calibrated_resume(&self, sequence: u64, mode: &str) {
            self.resume(sequence, mode);
            self.record(
                RESUME_PORTRAIT_RECORD,
                &format!("7:{NONCE}:{sequence}:CALIBRATED"),
            );
        }
    }

    #[test]
    fn startup_accepts_known_modes_and_proves_early_application_ancestry() {
        let fixture = Fixture::new();
        assert_eq!(
            fixture.context.startup_mode().unwrap(),
            NativeOutputMode::Y8Inv
        );
        fixture.record("native-epdc-mode", &format!("7:{NONCE}:Y8"));
        assert!(!fixture.context.startup_mode().unwrap().inverted());
        fixture.record("application-pid", "999");
        assert!(fixture.context.startup_mode().is_err());
    }

    #[test]
    fn startup_portrait_matches_the_pre_takeover_pair_before_any_native_io() {
        for (calibration, rotation, expected) in [
            ("U:3", 3, NativePortraitDirection::Up),
            ("D:1", 1, NativePortraitDirection::Down),
            ("FIXED:0", 0, NativePortraitDirection::Up),
        ] {
            let fixture = Fixture::new();
            fixture.portrait(calibration);
            let observed = fixture
                .context
                .observe_startup_portrait(|| Ok((rotation, "fresh")))
                .unwrap();
            assert_eq!(observed.direction, expected);
            assert_eq!(observed.framebuffer, "fresh");
            assert_eq!(
                observed.presentation,
                NativeResumePresentation::Observed(NativeOutputMode::Y8Inv)
            );
            assert!(
                fixture
                    .context
                    .observe_startup_portrait(|| Ok(((rotation + 2) % 4, ())))
                    .is_err()
            );
        }
    }

    #[test]
    fn portrait_calibration_never_assumes_an_absolute_device_rotation_mapping() {
        for baseline in 0..4 {
            for (direction, expected, flipped) in [
                (
                    "U",
                    NativePortraitDirection::Up,
                    NativePortraitDirection::Down,
                ),
                (
                    "D",
                    NativePortraitDirection::Down,
                    NativePortraitDirection::Up,
                ),
            ] {
                let fixture = Fixture::new();
                fixture.portrait(&format!("{direction}:{baseline}"));
                fixture.calibrated_resume(8, "Y8");
                assert_eq!(
                    fixture
                        .context
                        .observe_resume_portrait(|| Ok((baseline, ())))
                        .unwrap()
                        .direction,
                    expected
                );
                assert_eq!(
                    fixture
                        .context
                        .observe_resume_portrait(|| Ok(((baseline + 2) % 4, ())))
                        .unwrap()
                        .direction,
                    flipped
                );
                // Resolve each wake against the same trusted anchor, never by
                // repeatedly adding a delta to the last resumed direction.
                fixture.calibrated_resume(10, "Y8INV");
                assert_eq!(
                    fixture
                        .context
                        .observe_resume_portrait(|| Ok((baseline, ())))
                        .unwrap()
                        .direction,
                    expected
                );
            }
        }
    }

    #[test]
    fn portrait_resume_rejects_landscape_unknown_and_fixed_direction_changes() {
        let fixture = Fixture::new();
        fixture.portrait("U:3");
        fixture.calibrated_resume(8, "Y8");
        for rotation in [0, 2, 4, u32::MAX] {
            assert!(
                fixture
                    .context
                    .observe_resume_portrait(|| Ok((rotation, ())))
                    .is_err(),
                "{rotation}"
            );
        }
        fixture.portrait("FIXED:3");
        assert!(
            fixture
                .context
                .observe_resume_portrait(|| Ok((1, ())))
                .is_err()
        );
        assert_eq!(
            fixture
                .context
                .observe_resume_portrait(|| Ok((3, ())))
                .unwrap()
                .direction,
            NativePortraitDirection::Up
        );
    }

    #[test]
    fn legacy_epdc_records_cannot_substitute_for_a_protected_portrait_calibration() {
        let fixture = Fixture::new();
        assert_eq!(
            fixture.context.startup_mode().unwrap(),
            NativeOutputMode::Y8Inv
        );
        assert!(fixture.context.startup_portrait_calibration().is_err());
        assert!(
            fixture
                .context
                .observe_startup_portrait::<()>(|| panic!(
                    "missing calibration must not open the framebuffer"
                ))
                .is_err()
        );
        fixture.portrait("D:1");
        fixture.resume(8, "Y8");
        assert!(
            fixture
                .context
                .observe_resume_portrait::<()>(|| panic!(
                    "missing current permission must not sample the framebuffer"
                ))
                .is_err()
        );
        assert_eq!(fixture.context.resume_mode().unwrap(), NativeOutputMode::Y8);
    }

    #[test]
    fn portrait_records_require_exact_new_framing_and_canonical_values() {
        let fixture = Fixture::new();
        for raw in [
            format!("7:{NONCE}:U:3"),
            format!("7:{NONCE}:U:3\r\n"),
            format!("7:{NONCE}:U:3\n\n"),
            format!("7:{NONCE}:u:3\n"),
            format!("7:{NONCE}:U:03\n"),
            format!("7:{NONCE}:D:4\n"),
            format!("7:{NONCE}:U:3:extra\n"),
            format!("7:{NONCE}: U:3\n"),
            format!("8:{NONCE}:U:3\n"),
            "7:ffffffffffffffffffffffffffffffff:U:3\n".into(),
        ] {
            fs::write(fixture.lock.join(PORTRAIT_RECORD), &raw).unwrap();
            fs::set_permissions(
                fixture.lock.join(PORTRAIT_RECORD),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            assert!(
                fixture
                    .context
                    .observe_startup_portrait::<()>(|| panic!(
                        "invalid calibration must not sample devices"
                    ))
                    .is_err(),
                "{raw:?}"
            );
        }
        fixture.portrait("U:3");
        fixture.calibrated_resume(8, "Y8");
        for raw in [
            format!("7:{NONCE}:8:CALIBRATED"),
            format!("7:{NONCE}:8:CALIBRATED\r\n"),
            format!("7:{NONCE}:8:CALIBRATED\n\n"),
            format!("7:{NONCE}:8:U:3\n"),
            format!("7:{NONCE}:7:CALIBRATED\n"),
            format!("7:{NONCE}:08:CALIBRATED\n"),
            format!("8:{NONCE}:8:CALIBRATED\n"),
        ] {
            fs::write(fixture.lock.join(RESUME_PORTRAIT_RECORD), &raw).unwrap();
            assert!(
                fixture
                    .context
                    .observe_resume_portrait::<()>(|| panic!(
                        "invalid permission must not sample devices"
                    ))
                    .is_err(),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn portrait_resume_requires_active_sequence_current_application_and_output_capability() {
        let fixture = Fixture::new();
        fixture.portrait("D:1");
        fixture.calibrated_resume(8, "FRAMEBUFFER");
        assert!(
            fixture
                .context
                .observe_resume_portrait::<()>(|| panic!(
                    "missing output capability must reject the kernel sample"
                ))
                .is_err()
        );
        fixture
            .context
            .publish_framebuffer_capability(true)
            .unwrap();
        let observed = fixture
            .context
            .observe_resume_portrait(|| Ok((3, Some(false))))
            .unwrap();
        assert_eq!(observed.direction, NativePortraitDirection::Up);
        assert_eq!(
            observed.presentation,
            NativeResumePresentation::FramebufferProbe
        );
        assert_eq!(observed.framebuffer, Some(false));
        fixture.context.set_resume_sequence(0);
        assert!(
            fixture
                .context
                .observe_resume_portrait::<()>(|| panic!(
                    "a detached callback cannot observe a wake"
                ))
                .is_err()
        );
        fixture.context.set_resume_sequence(9);
        assert!(
            fixture
                .context
                .observe_resume_portrait::<()>(|| panic!("a stale request cannot observe a wake"))
                .is_err()
        );
        fixture.context.set_resume_sequence(8);
        fixture.record("application-pid", "999");
        assert!(
            fixture
                .context
                .observe_resume_portrait::<()>(|| panic!(
                    "another application cannot observe a wake"
                ))
                .is_err()
        );
    }

    #[test]
    fn portrait_kernel_observation_rechecks_ownership_records_and_request_before_returning() {
        for changed in [
            PORTRAIT_RECORD,
            RESUME_PORTRAIT_RECORD,
            "lifecycle-request",
            "nonce",
            "application-pid",
            "resume-native-epdc-mode",
        ] {
            let fixture = Fixture::new();
            fixture.portrait("U:3");
            fixture.calibrated_resume(8, "Y8");
            assert!(
                fixture
                    .context
                    .observe_resume_portrait(|| {
                        fixture.record(changed, "changed");
                        Ok((1, ()))
                    })
                    .is_err(),
                "{changed}"
            );
        }
        let fixture = Fixture::new();
        fixture.portrait("U:3");
        fixture.calibrated_resume(8, "Y8");
        assert!(
            fixture
                .context
                .observe_resume_portrait(|| {
                    fixture.context.set_resume_sequence(0);
                    Ok((1, ()))
                })
                .is_err()
        );
        let fixture = Fixture::new();
        fixture.portrait("U:3");
        assert!(
            fixture
                .context
                .observe_startup_portrait(|| {
                    fixture.portrait("D:1");
                    Ok((3, ()))
                })
                .is_err()
        );
    }

    #[test]
    fn portrait_sidecars_use_the_same_private_no_follow_record_boundary() {
        for name in [PORTRAIT_RECORD, RESUME_PORTRAIT_RECORD] {
            for kind in ["symlink", "hardlink", "fifo", "public", "oversized"] {
                let fixture = Fixture::new();
                fixture.portrait("U:3");
                fixture.calibrated_resume(8, "Y8");
                let target = fixture.lock.join(name);
                let outside = fixture._temporary.path().join("outside");
                fs::write(&outside, fs::read(&target).unwrap()).unwrap();
                fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
                fs::remove_file(&target).unwrap();
                match kind {
                    "symlink" => symlink(&outside, &target).unwrap(),
                    "hardlink" => fs::hard_link(&outside, &target).unwrap(),
                    "fifo" => {
                        use std::{ffi::CString, os::unix::ffi::OsStrExt};
                        let path = CString::new(target.as_os_str().as_bytes()).unwrap();
                        // SAFETY: this pathname belongs to the isolated fixture.
                        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
                    }
                    "public" => {
                        fs::copy(&outside, &target).unwrap();
                        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
                    }
                    _ => fixture.record(name, &"x".repeat(MAX_MARKER_BYTES + 1)),
                }
                assert!(
                    fixture
                        .context
                        .observe_resume_portrait::<()>(|| panic!(
                            "unsafe sidecar must not sample devices"
                        ))
                        .is_err(),
                    "{name}:{kind}"
                );
            }
        }
    }

    #[test]
    fn resume_requires_the_current_dispatch_sequence_and_current_application() {
        let fixture = Fixture::new();
        fixture.resume(8, "Y8");
        assert_eq!(fixture.context.resume_mode().unwrap(), NativeOutputMode::Y8);
        fixture.context.set_resume_sequence(0);
        assert!(fixture.context.resume_mode().is_err());
        fixture.context.set_resume_sequence(9);
        assert!(fixture.context.resume_mode().is_err());
        fixture.resume(9, "Y8INV");
        assert!(fixture.context.resume_mode().unwrap().inverted());
        fixture.record("resume-native-epdc-mode", &format!("7:{NONCE}:8:Y8"));
        assert!(fixture.context.resume_mode().is_err());
        fixture.resume(9, "Y8");
        fs::remove_file(fixture.lock.join("application-pid")).unwrap();
        assert!(fixture.context.resume_mode().is_err());
    }

    #[test]
    fn capability_publication_is_private_atomic_and_bound_before_pid_publication() {
        use std::os::unix::fs::MetadataExt;
        let fixture = Fixture::new();
        let capability = fixture.lock.join(CAPABILITY_RECORD);
        assert!(!fixture.lock.join("application-pid").exists());
        fixture
            .context
            .publish_framebuffer_capability(true)
            .unwrap();
        assert_eq!(
            fs::read_to_string(&capability).unwrap(),
            format!("7:{NONCE}:303:framebuffer8\n")
        );
        let first = File::open(&capability).unwrap();
        let first_identity = first.metadata().unwrap();
        assert_eq!(first_identity.mode() & 0o777, 0o600);
        assert_eq!(first_identity.nlink(), 1);
        assert!(!fixture.lock.join(CAPABILITY_PENDING).exists());
        fixture.record("application-pid", "303");
        fixture
            .context
            .publish_framebuffer_capability(false)
            .unwrap();
        assert_eq!(
            fs::read_to_string(&capability).unwrap(),
            format!("7:{NONCE}:303:legacy\n")
        );
        assert!(!same_identity(
            &first_identity,
            &fs::metadata(&capability).unwrap()
        ));
        assert!(!fixture.lock.join(CAPABILITY_PENDING).exists());
        assert_eq!(
            fixture.context.startup_mode().unwrap(),
            NativeOutputMode::Y8Inv
        );
    }

    #[test]
    fn capability_publication_rejects_changed_ownership_and_foreign_existing_records() {
        for (name, value) in [
            ("generation", format!("8:{NONCE}")),
            ("nonce", "ffffffffffffffffffffffffffffffff".into()),
            ("application-pid", "999".into()),
            (CAPABILITY_RECORD, format!("7:{NONCE}:999:framebuffer8")),
            (CAPABILITY_RECORD, format!("8:{NONCE}:303:legacy")),
            (CAPABILITY_RECORD, format!("7:{NONCE}:303:unknown")),
        ] {
            let fixture = Fixture::new();
            fixture.record(name, &value);
            assert!(
                fixture
                    .context
                    .publish_framebuffer_capability(true)
                    .is_err(),
                "{name}"
            );
            assert!(!fixture.lock.join(CAPABILITY_PENDING).exists());
            assert_eq!(
                fs::read_to_string(fixture.lock.join(name)).unwrap(),
                format!("{value}\n")
            );
        }
        let fixture = Fixture::new();
        fs::write(fixture.context.proc_root.join("302/status"), "PPid:\t999\n").unwrap();
        assert!(
            fixture
                .context
                .publish_framebuffer_capability(true)
                .is_err()
        );
        assert!(!fixture.lock.join(CAPABILITY_RECORD).exists());
    }

    #[test]
    fn capability_publication_never_replaces_or_removes_an_existing_pending_marker() {
        for kind in ["regular", "symlink", "hardlink", "fifo"] {
            let fixture = Fixture::new();
            let pending = fixture.lock.join(CAPABILITY_PENDING);
            let outside = fixture._temporary.path().join("outside");
            fs::write(&outside, "untouched\n").unwrap();
            fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
            match kind {
                "regular" => fixture.record(CAPABILITY_PENDING, "unowned"),
                "symlink" => symlink(&outside, &pending).unwrap(),
                "hardlink" => fs::hard_link(&outside, &pending).unwrap(),
                _ => {
                    use std::{ffi::CString, os::unix::ffi::OsStrExt};
                    let path = CString::new(pending.as_os_str().as_bytes()).unwrap();
                    // SAFETY: this pathname belongs to the temporary fixture.
                    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
                }
            }
            assert!(
                fixture
                    .context
                    .publish_framebuffer_capability(true)
                    .is_err(),
                "{kind}"
            );
            assert!(fs::symlink_metadata(&pending).is_ok(), "{kind}");
            assert!(!fixture.lock.join(CAPABILITY_RECORD).exists());
            assert_eq!(fs::read_to_string(&outside).unwrap(), "untouched\n");
        }
    }

    #[test]
    fn capability_publication_rejects_unsafe_targets_and_cleans_only_its_failed_pending_write() {
        for kind in ["symlink", "hardlink", "fifo", "public"] {
            let fixture = Fixture::new();
            let capability = fixture.lock.join(CAPABILITY_RECORD);
            let outside = fixture._temporary.path().join("outside");
            fs::write(&outside, format!("7:{NONCE}:303:framebuffer8\n")).unwrap();
            fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
            match kind {
                "symlink" => symlink(&outside, &capability).unwrap(),
                "hardlink" => fs::hard_link(&outside, &capability).unwrap(),
                "fifo" => {
                    use std::{ffi::CString, os::unix::ffi::OsStrExt};
                    let path = CString::new(capability.as_os_str().as_bytes()).unwrap();
                    // SAFETY: this pathname belongs to the temporary fixture.
                    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
                }
                _ => {
                    fixture.record(CAPABILITY_RECORD, &format!("7:{NONCE}:303:framebuffer8"));
                    fs::set_permissions(&capability, fs::Permissions::from_mode(0o644)).unwrap();
                }
            }
            assert!(
                fixture
                    .context
                    .publish_framebuffer_capability(true)
                    .is_err(),
                "{kind}"
            );
            assert!(fs::symlink_metadata(&capability).is_ok());
            assert!(!fixture.lock.join(CAPABILITY_PENDING).exists());
        }
        let fixture = Fixture::new();
        let validations = std::cell::Cell::new(0);
        assert!(
            fixture
                .context
                .records
                .publish_capability(&fixture.context.capability_record(true), || {
                    validations.set(validations.get() + 1);
                    ensure!(
                        validations.get() == 1,
                        "The fixture changed ownership before publication"
                    );
                    Ok(())
                })
                .is_err()
        );
        assert_eq!(validations.get(), 2);
        assert!(!fixture.lock.join(CAPABILITY_PENDING).exists());
        assert!(!fixture.lock.join(CAPABILITY_RECORD).exists());
    }

    #[test]
    fn framebuffer_resume_requires_current_capability_and_never_uses_the_startup_mode() {
        let fixture = Fixture::new();
        fixture.resume(8, "FRAMEBUFFER");
        assert!(fixture.context.resume_presentation_request().is_err());
        fixture
            .context
            .publish_framebuffer_capability(false)
            .unwrap();
        assert!(fixture.context.resume_presentation_request().is_err());
        fixture
            .context
            .publish_framebuffer_capability(true)
            .unwrap();
        assert_eq!(
            fixture.context.resume_presentation_request().unwrap(),
            NativeResumePresentation::FramebufferProbe
        );
        assert!(fixture.context.resume_mode().is_err());
        assert_eq!(
            fixture.context.startup_mode().unwrap(),
            NativeOutputMode::Y8Inv
        );
        fixture.context.set_resume_sequence(0);
        assert!(fixture.context.resume_presentation_request().is_err());
        fixture.context.set_resume_sequence(9);
        assert!(fixture.context.resume_presentation_request().is_err());
        fixture.resume(9, "Y8");
        assert_eq!(
            fixture.context.resume_presentation_request().unwrap(),
            NativeResumePresentation::Observed(NativeOutputMode::Y8)
        );
        fixture.resume(10, "FRAMEBUFFER");
        fs::remove_file(fixture.lock.join("application-pid")).unwrap();
        assert!(fixture.context.resume_presentation_request().is_err());
    }

    #[test]
    fn framebuffer_requests_reject_stale_capabilities_and_require_exact_new_record_framing() {
        let fixture = Fixture::new();
        fixture.resume(8, "FRAMEBUFFER");
        for capability in [
            format!("8:{NONCE}:303:framebuffer8\n"),
            "7:ffffffffffffffffffffffffffffffff:303:framebuffer8\n".into(),
            format!("7:{NONCE}:999:framebuffer8\n"),
            format!("7:{NONCE}:303:FRAMEBUFFER\n"),
            format!("7:{NONCE}:303:framebuffer8"),
            format!("7:{NONCE}:303:framebuffer8\r\n"),
            format!("7:{NONCE}:303:framebuffer8\n\n"),
        ] {
            fs::write(fixture.lock.join(CAPABILITY_RECORD), &capability).unwrap();
            fs::set_permissions(
                fixture.lock.join(CAPABILITY_RECORD),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            assert!(
                fixture.context.resume_presentation_request().is_err(),
                "{capability:?}"
            );
        }
        fixture.record(CAPABILITY_RECORD, &format!("7:{NONCE}:303:framebuffer8"));
        for record in [
            format!("7:{NONCE}:8:FRAMEBUFFER"),
            format!("7:{NONCE}:8:FRAMEBUFFER\r\n"),
            format!("7:{NONCE}:8:FRAMEBUFFER\n\n"),
            format!("7:{NONCE}:8:framebuffer\n"),
            format!("7:{NONCE}:8:FRAMEBUFFER:extra\n"),
        ] {
            fs::write(fixture.lock.join("resume-native-epdc-mode"), &record).unwrap();
            assert!(
                fixture.context.resume_presentation_request().is_err(),
                "{record:?}"
            );
        }
        fixture.record("native-epdc-mode", &format!("7:{NONCE}:FRAMEBUFFER"));
        assert!(fixture.context.startup_mode().is_err());
    }

    #[test]
    fn framebuffer_resume_rechecks_current_request_and_private_capability() {
        let fixture = Fixture::new();
        fixture.resume(8, "FRAMEBUFFER");
        fixture
            .context
            .publish_framebuffer_capability(true)
            .unwrap();
        fixture.record("lifecycle-request", &format!("7:{NONCE}:9:resume"));
        assert!(fixture.context.resume_presentation_request().is_err());
        fixture.resume(8, "FRAMEBUFFER");
        fs::set_permissions(
            fixture.lock.join(CAPABILITY_RECORD),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(fixture.context.resume_presentation_request().is_err());
        fs::remove_file(fixture.lock.join(CAPABILITY_RECORD)).unwrap();
        symlink(
            fixture.lock.join("native-epdc-mode"),
            fixture.lock.join(CAPABILITY_RECORD),
        )
        .unwrap();
        assert!(fixture.context.resume_presentation_request().is_err());
    }

    #[test]
    fn missing_unknown_stale_and_ambiguous_records_fail_closed() {
        let fixture = Fixture::new();
        for value in [
            format!("8:{NONCE}:Y8"),
            "7:ffffffffffffffffffffffffffffffff:Y8".into(),
            format!("7:{NONCE}:y8inv"),
            format!("7:{NONCE}:Y8INV:extra"),
            format!("7:{NONCE}:Y8INV\n"),
            format!("7:{NONCE}: Y8INV"),
        ] {
            fixture.record("native-epdc-mode", &value);
            assert!(fixture.context.startup_mode().is_err());
        }
        fs::remove_file(fixture.lock.join("native-epdc-mode")).unwrap();
        assert!(fixture.context.startup_mode().is_err());
    }

    #[test]
    fn mode_reads_reject_changed_context_and_broken_ancestry() {
        for name in ["generation", "nonce", "pid"] {
            let fixture = Fixture::new();
            fixture.record(name, "wrong");
            assert!(fixture.context.startup_mode().is_err());
        }
        let fixture = Fixture::new();
        fs::write(fixture.context.proc_root.join("302/status"), "PPid:\t999\n").unwrap();
        assert!(fixture.context.startup_mode().is_err());
    }

    #[test]
    fn private_records_reject_symlinks_hardlinks_fifos_and_public_permissions() {
        for kind in ["symlink", "hardlink", "fifo", "public", "oversized"] {
            let fixture = Fixture::new();
            let mode = fixture.lock.join("native-epdc-mode");
            let outside = fixture._temporary.path().join("outside");
            fs::write(&outside, format!("7:{NONCE}:Y8INV\n")).unwrap();
            fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
            fs::remove_file(&mode).unwrap();
            match kind {
                "symlink" => symlink(&outside, &mode).unwrap(),
                "hardlink" => fs::hard_link(&outside, &mode).unwrap(),
                "fifo" => {
                    use std::{ffi::CString, os::unix::ffi::OsStrExt};
                    let path = CString::new(mode.as_os_str().as_bytes()).unwrap();
                    // SAFETY: path is a valid, terminated temporary test pathname.
                    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
                }
                "public" => {
                    fixture.record("native-epdc-mode", &format!("7:{NONCE}:Y8INV"));
                    fs::set_permissions(&mode, fs::Permissions::from_mode(0o644)).unwrap();
                }
                _ => fixture.record("native-epdc-mode", &"x".repeat(MAX_MARKER_BYTES + 1)),
            }
            assert!(fixture.context.startup_mode().is_err(), "{kind}");
        }
    }

    #[test]
    fn pinned_directory_rejects_replacement_and_public_directory_permissions() {
        let fixture = Fixture::new();
        let old = fixture._temporary.path().join("old-lock");
        fs::rename(&fixture.lock, old).unwrap();
        fs::create_dir(&fixture.lock).unwrap();
        fs::set_permissions(&fixture.lock, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(fixture.context.startup_mode().is_err());
        let fixture = Fixture::new();
        fs::set_permissions(&fixture.lock, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(fixture.context.startup_mode().is_err());
    }
}

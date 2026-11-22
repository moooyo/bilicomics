//! Opt-in startup display sampling for a private study build only.

use std::{
    cell::RefCell,
    ffi::{CStr, CString},
    fmt::Write as _,
    fs::{File, OpenOptions},
    io::{self, Read, Write as _},
    os::{fd::{AsRawFd, FromRawFd}, unix::fs::{MetadataExt, OpenOptionsExt}},
    path::Path,
};

#[derive(Clone, Copy)]
pub(crate) enum Bucket {
    LaunchApproval, PermitAcquire, PermitInitial, QueryOuterPre, QueryOuterPost,
    QueryInnerPre, QueryInnerPost, QueryInnerPeriodic, MetadataOuterPre, MetadataOuterPost,
    MetadataCheckpoint, Metadata1, Metadata2, CommandsRun, CommandSpawn, CommandRead,
    CommandTryWait, CommandPoll,
}

const BUCKETS: [&str; Bucket::CommandPoll as usize + 1] = [
    "launch_approval", "permit_acquire", "permit_initial", "query_outer_pre", "query_outer_post",
    "query_inner_pre", "query_inner_post", "query_inner_periodic", "metadata_outer_pre", "metadata_outer_post",
    "metadata_checkpoint", "metadata_1", "metadata_2", "commands_run", "command_spawn", "command_read",
    "command_try_wait", "command_poll",
];

#[derive(Clone, Copy, Default)]
struct Elapsed {
    elapsed_ns: u64,
    count: u32,
}

#[derive(Default)]
struct Totals {
    buckets: [Elapsed; BUCKETS.len()],
    process_stat_reads: u32,
    invalid_clock: bool,
}

impl Totals {
    fn add(&mut self, bucket: Bucket, started: Option<u64>, finished: Option<u64>) {
        let entry = &mut self.buckets[bucket as usize];
        let updated = finished.zip(started).and_then(|(finish, start)| finish.checked_sub(start))
            .and_then(|elapsed| entry.elapsed_ns.checked_add(elapsed)).zip(entry.count.checked_add(1));
        if let Some((elapsed_ns, count)) = updated {
            *entry = Elapsed { elapsed_ns, count };
        } else {
            self.invalid_clock = true;
        }
    }
}

thread_local! {
    static ACTIVE: RefCell<Option<Totals>> = const { RefCell::new(None) };
}

pub(crate) struct Identity {
    pub(crate) supervisor_pid: u32,
    pub(crate) supervisor_birth: u64,
    pub(crate) native_read_pid: u32,
    pub(crate) native_read_birth: u64,
    pub(crate) native_read_sequence: u64,
    pub(crate) helper_pid: u32,
    pub(crate) helper_birth: u64,
}

pub(crate) struct Sampling {
    run_id: String,
    started: u64,
}

impl Sampling {
    pub(crate) fn begin() -> Option<Self> {
        let run_id = std::env::var("BILICOMICS_DISPLAY_COST_RUN").ok()?;
        if !valid_run_id(&run_id) { return None; }
        let started = monotonic_ns()?;
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            if active.is_some() { return None; }
            *active = Some(Totals::default());
            Some(Self { run_id, started })
        })
    }

    pub(crate) fn run_id(&self) -> &str { &self.run_id }

    pub(crate) fn finish(self, identity: Identity) {
        let Some(finished) = monotonic_ns().filter(|finished| *finished >= self.started) else { return; };
        let Some(totals) = ACTIVE.with(|active| active.borrow_mut().take()) else { return; };
        if !totals.invalid_clock {
            let _ = publish(Path::new("/mnt/us"), &self.run_id, identity, self.started, finished, &totals);
        }
    }
}

impl Drop for Sampling {
    fn drop(&mut self) { ACTIVE.with(|active| { active.borrow_mut().take(); }); }
}

pub(crate) fn process_stat_read() {
    ACTIVE.with(|active| {
        if let Some(totals) = active.borrow_mut().as_mut() {
            if let Some(count) = totals.process_stat_reads.checked_add(1) {
                totals.process_stat_reads = count;
            } else {
                totals.invalid_clock = true;
            }
        }
    });
}

pub(crate) fn observe<T>(bucket: Bucket, work: impl FnOnce() -> T) -> T {
    observe_with(bucket, work, monotonic_ns)
}

fn observe_with<T>(bucket: Bucket, work: impl FnOnce() -> T, clock: impl Fn() -> Option<u64>) -> T {
    if !ACTIVE.with(|active| active.borrow().is_some()) { return work(); }
    let started = clock();
    let result = work();
    let finished = clock();
    ACTIVE.with(|active| {
        if let Some(totals) = active.borrow_mut().as_mut() {
            totals.add(bucket, started, finished);
        }
    });
    result
}

fn monotonic_ns() -> Option<u64> {
    let mut stamp = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    let result = unsafe {
        let errno = libc::__errno_location();
        let saved = *errno;
        let status = libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut stamp);
        *errno = saved;
        status
    };
    if result != 0 { return None; }
    u64::try_from(stamp.tv_sec).ok()?.checked_mul(1_000_000_000)?
        .checked_add(u64::try_from(stamp.tv_nsec).ok()?)
}

fn lower_hex(byte: u8) -> bool { byte.is_ascii_digit() || matches!(byte, b'a'..=b'f') }

fn valid_run_id(run_id: &str) -> bool {
    run_id.strip_prefix("bili-shelf-").is_some_and(|suffix| suffix.len() == 32 && suffix.bytes().all(lower_hex))
}

fn boot_id(value: &str) -> Option<&str> {
    let value = value.strip_suffix('\n').unwrap_or(value);
    (value.len() == 36 && value.bytes().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) { byte == b'-' } else { lower_hex(byte) }
    })).then_some(value)
}

fn publish(base: &Path, run_id: &str, identity: Identity, started: u64, finished: u64, totals: &Totals) -> io::Result<()> {
    let mut boot = String::new();
    File::open("/proc/sys/kernel/random/boot_id")?.take(38).read_to_string(&mut boot)?;
    let boot = boot_id(&boot).ok_or(io::ErrorKind::InvalidData)?;
    let line = json_line(run_id, boot, identity, started, finished, totals);
    write_new(base, run_id, line.as_bytes())
}

fn json_line(run_id: &str, boot: &str, identity: Identity, started: u64, finished: u64, totals: &Totals) -> String {
    let Identity { supervisor_pid, supervisor_birth, native_read_pid, native_read_birth,
        native_read_sequence, helper_pid, helper_birth } = identity;
    let mut line = format!(concat!(
        "{{\"schema\":\"kindle-startup-display-cost-v1\",\"clock\":\"CLOCK_MONOTONIC\",\"run_id\":\"{}\",\"boot_id\":\"{}\",",
        "\"supervisor_pid\":{},\"supervisor_birth\":{},\"native_read_pid\":{},\"native_read_birth\":{},\"native_read_sequence\":{},",
        "\"helper_pid\":{},\"helper_birth\":{},\"helper_started_monotonic_ns\":{},\"helper_finished_monotonic_ns\":{},",
        "\"process_stat_reads\":{},\"buckets\":{{"),
        run_id, boot, supervisor_pid, supervisor_birth, native_read_pid, native_read_birth, native_read_sequence,
        helper_pid, helper_birth, started, finished, totals.process_stat_reads);
    for (index, (name, elapsed)) in BUCKETS.iter().zip(&totals.buckets).enumerate() {
        if index != 0 { line.push(','); }
        let _ = write!(line, "\"{name}\":{{\"elapsed_ns\":{},\"count\":{}}}", elapsed.elapsed_ns, elapsed.count);
    }
    line.push_str("}}\n");
    line
}

const DIRECTORY_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;

fn write_new(base: &Path, run_id: &str, bytes: &[u8]) -> io::Result<()> {
    if !valid_run_id(run_id) { return Err(io::ErrorKind::InvalidInput.into()); }
    let base = OpenOptions::new().read(true).custom_flags(DIRECTORY_FLAGS).open(base)?;
    let run = open_at(&base, &CString::new(run_id)?, DIRECTORY_FLAGS)?;
    let data = open_at(&run, c"data", DIRECTORY_FLAGS)?;
    let owner = unsafe { libc::geteuid() };
    if run.metadata()?.uid() != owner || data.metadata()?.uid() != owner {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let leaf = CString::new(format!("{run_id}.display-cost.json"))?;
    open_at(&data, &leaf, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC)?
        .write_all(bytes)
}

fn open_at(directory: &File, leaf: &CStr, flags: libc::c_int) -> io::Result<File> {
    let descriptor = unsafe { libc::openat(directory.as_raw_fd(), leaf.as_ptr(), flags, 0o600 as libc::mode_t) };
    if descriptor < 0 { Err(io::Error::last_os_error()) } else { Ok(unsafe { File::from_raw_fd(descriptor) }) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, fs, os::unix::fs::{DirBuilderExt, PermissionsExt, symlink}};

    const RUN: &str = "bili-shelf-0123456789abcdef0123456789abcdef";

    #[test]
    fn capture_cost_preserves_nested_scopes_without_extra_work_or_clocks() {
        assert_eq!(observe_with(Bucket::Metadata2, || 42, || panic!("disabled sample read a clock")), 42);
        let clock = Cell::new(0);
        let step = || { clock.set(clock.get() + 1); Some(clock.get()) };
        ACTIVE.with(|active| *active.borrow_mut() = Some(Totals::default()));
        observe_with(Bucket::CommandsRun, || {
            observe_with(Bucket::QueryInnerPre, process_stat_read, step);
            observe_with(Bucket::CommandPoll, || {}, step);
        }, step);
        let totals = ACTIVE.with(|active| active.borrow_mut().take().unwrap());
        assert_eq!(totals.buckets[Bucket::CommandsRun as usize].elapsed_ns, 5);
        assert_eq!(totals.buckets[Bucket::QueryInnerPre as usize].elapsed_ns, 1);
        assert_eq!(totals.buckets[Bucket::CommandPoll as usize].elapsed_ns, 1);
        assert_eq!(totals.process_stat_reads, 1);
        assert!(!totals.invalid_clock);
    }

    #[test]
    fn capture_cost_refuses_an_incomplete_or_backwards_clock() {
        for (start, end) in [(None, Some(6)), (Some(5), None), (Some(7), Some(6))] {
            let mut totals = Totals::default();
            totals.add(Bucket::PermitAcquire, start, end);
            assert!(totals.invalid_clock);
        }
        assert!(!valid_run_id("bili-shelf-0123456789abcdef0123456789abcdeF"));
        assert!(!valid_run_id("../other"));
        assert_eq!(boot_id("12345678-9abc-4def-8123-456789abcdef\n"), Some("12345678-9abc-4def-8123-456789abcdef"));
        assert!(boot_id("invalid").is_none());
    }

    #[test]
    fn capture_cost_sidecar_is_exclusive_and_refuses_parent_symlinks() -> io::Result<()> {
        let base = std::env::temp_dir().join(format!("bilicomics-display-cost-test-{}", std::process::id()));
        let run = base.join(RUN);
        let data = run.join("data");
        let leaf = data.join(format!("{RUN}.display-cost.json"));
        fs::DirBuilder::new().mode(0o700).create(&base)?;
        let result = (|| {
            fs::create_dir(&run)?;
            symlink(&base, &data)?;
            let symlink_rejected = write_new(&base, RUN, b"sample\n").is_err();
            fs::remove_file(&data)?;
            fs::create_dir(&data)?;
            write_new(&base, RUN, b"sample\n")?;
            let duplicate = write_new(&base, RUN, b"replacement\n");
            let mode = fs::metadata(&leaf)?.permissions().mode();
            let original = fs::read(&leaf)?;
            Ok::<_, io::Error>((symlink_rejected, duplicate, mode, original))
        })();
        fs::remove_dir_all(&base)?;
        let (symlink_rejected, duplicate, mode, original) = result?;
        assert!(symlink_rejected);
        assert_eq!(duplicate.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(original, b"sample\n");
        Ok(())
    }
}

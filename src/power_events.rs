//! Signal-safe power-event delivery to the existing Slint event loop.

use std::{
    cell::RefCell,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering}},
};

use anyhow::{Context, Result, bail, ensure};

#[cfg(any(not(unix), test))]
use std::io::Read;

use crate::native_presentation::{NativePresentationContext, PrivateDirectory, single_record};

#[cfg(unix)]
mod native_relay;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerEvent { Suspend, Resume, Quit }

const MAX_NATIVE_POWER_RECORD_BYTES: usize = 256;

/// Relay native power records without initializing application state or I/O.
/// Every emitted record is one short write, so concurrent FIFO writers cannot
/// splice lifecycle notices into a partial native record.
pub fn relay_native_power_events() -> Result<()> {
    #[cfg(unix)] {
        use std::{fs::File, os::fd::FromRawFd};
        fn duplicate(descriptor: libc::c_int) -> Result<File> {
            // SAFETY: fcntl duplicates an existing standard stream without
            // transferring ownership of the original descriptor.
            let duplicated = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 4) };
            ensure!(duplicated >= 0, "Duplicate native power relay stream");
            // SAFETY: the successful duplication has exactly one File owner.
            Ok(unsafe { File::from_raw_fd(duplicated) })
        }
        native_relay::run(duplicate(libc::STDIN_FILENO)?, duplicate(3)?, duplicate(libc::STDOUT_FILENO)?)
    }
    #[cfg(not(unix))] {
        relay_native_power_records(std::io::stdin().lock(), std::io::stdout().lock())
    }
}

#[cfg(any(not(unix), test))]
fn relay_native_power_records(mut input: impl Read, mut output: impl Write) -> Result<()> {
    let mut pending = [0u8; MAX_NATIVE_POWER_RECORD_BYTES];
    let mut count = 0;
    let mut buffer = [0u8; MAX_NATIVE_POWER_RECORD_BYTES];
    loop {
        let received = match input.read(&mut buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result.context("Read native power event stream")?,
        };
        if received == 0 {
            ensure!(count == 0, "The native power event stream ended with an incomplete record");
            return Ok(());
        }
        for &byte in &buffer[..received] {
            ensure!(count < MAX_NATIVE_POWER_RECORD_BYTES - 1 || byte == b'\n', "The native power event record is too large");
            ensure!(byte == b'\n' || (b' '..=b'~').contains(&byte), "The native power event record contains unsupported bytes");
            pending[count] = byte;
            count += 1;
            if byte != b'\n' { continue; }
            validate_native_power_record(&pending[..count])?;
            let written = loop {
                match output.write(&pending[..count]) {
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    result => break result.context("Forward native power event record")?,
                }
            };
            ensure!(written == count, "The native power event record was not written atomically");
            output.flush().context("Flush native power event record")?;
            count = 0;
        }
    }
}

fn validate_native_power_record(record: &[u8]) -> Result<()> {
    let line = std::str::from_utf8(record.strip_suffix(b"\n").context("Incomplete native power event record")?)?;
    let event = line.split(' ').next().unwrap_or_default();
    ensure!(matches!(event, "goingToScreenSaver" | "outOfScreenSaver" | "exitingScreenSaver" | "wakeupFromSuspend" | "usbConfigured"),
        "The native power event stream contains an unknown event");
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerAck { pub checkpoint_saved: bool }

impl PowerAck {
    pub fn saved() -> Self { Self { checkpoint_saved: true } }
    pub fn checkpoint_error() -> Self { Self { checkpoint_saved: false } }
}

type PowerCallback = Box<dyn FnMut(PowerEvent) -> Result<PowerAck>>;

struct Binding {
    callback: PowerCallback,
    context: Option<LaunchContext>,
    last_ack: Option<(Request, PowerAck)>,
}

thread_local! { static BINDING: RefCell<Option<Binding>> = const { RefCell::new(None) }; }

static SIGNAL_WRITE: AtomicI32 = AtomicI32::new(-1);
static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);
static NEXT_FILE: AtomicU32 = AtomicU32::new(1);

#[derive(Clone)]
struct LaunchContext {
    directory: PathBuf,
    token: String,
    generation: u64,
    records: Arc<PrivateDirectory>,
    presentation: NativePresentationContext,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Request { generation: u64, token: String, sequence: u64, event: PowerEvent }

impl Request {
    fn parse(value: &str) -> Option<Self> {
        let fields = single_record(value).ok()?.split(':').collect::<Vec<_>>();
        if fields.len() != 4 { return None; }
        if !fields[0].bytes().all(|byte| byte.is_ascii_digit()) || !fields[2].bytes().all(|byte| byte.is_ascii_digit()) { return None; }
        let generation = fields[0].parse::<u64>().ok().filter(|value| *value > 0)?;
        let sequence = fields[2].parse::<u64>().ok().filter(|value| *value > 0)?;
        if fields[1].len() != 32 || !fields[1].bytes().all(|byte| byte.is_ascii_hexdigit()) { return None; }
        let event = match fields[3] { "suspend" => PowerEvent::Suspend, "resume" => PowerEvent::Resume, _ => return None };
        Some(Self { generation, token: fields[1].to_owned(), sequence, event })
    }

    fn line(&self) -> String {
        format!("{}:{}:{}:{}", self.generation, self.token, self.sequence,
            if self.event == PowerEvent::Suspend { "suspend" } else { "resume" })
    }
}

impl LaunchContext {
    fn new(directory: PathBuf, token: String, generation: u64) -> Result<Self> {
        let records = Arc::new(PrivateDirectory::open(&directory)?);
        let presentation = NativePresentationContext::new(records.clone(), generation, token.clone());
        Ok(Self { directory, token, generation, records, presentation })
    }

    fn from_environment() -> Result<Option<Self>> {
        let directory = std::env::var_os("BILICOMICS_LIFECYCLE_DIR");
        let token = std::env::var("BILICOMICS_LAUNCH_TOKEN").ok();
        let generation = std::env::var("BILICOMICS_LAUNCH_GENERATION").ok();
        if directory.is_none() && token.is_none() && generation.is_none() { return Ok(None); }
        let directory = PathBuf::from(directory.context("Missing lifecycle directory")?);
        ensure!(directory.is_absolute(), "The lifecycle directory must be absolute");
        let token = token.context("Missing lifecycle token")?;
        ensure!(token.len() == 32 && token.bytes().all(|byte| byte.is_ascii_hexdigit()), "Invalid lifecycle token");
        let generation = generation.context("Missing lifecycle generation")?.parse::<u64>()?;
        ensure!(generation > 0, "Invalid lifecycle generation");
        let context = Self::new(directory, token, generation)?;
        ensure!(single_record(&context.read("nonce")?)? == context.token, "The lifecycle token does not match its directory");
        ensure!(single_record(&context.read("generation")?)? == context.header(), "The lifecycle generation does not match its directory");
        if let Some(pid) = context.records.read_optional("application-pid")? {
            ensure!(single_record(&pid)? == std::process::id().to_string(), "The lifecycle directory belongs to another process");
        }
        Ok(Some(context))
    }

    fn header(&self) -> String { format!("{}:{}", self.generation, self.token) }

    fn read(&self, name: &str) -> Result<String> {
        self.records.read(name)
    }

    fn request(&self, event: PowerEvent) -> Option<Request> {
        let request = Request::parse(&self.read("lifecycle-request").ok()?)?;
        (request.event == event && request.generation == self.generation && request.token == self.token).then_some(request)
    }

    fn acknowledge(&self, request: &Request, ack: PowerAck) -> Result<()> {
        atomic_marker(&self.directory, "lifecycle-ack", &format!("{}:{}\n", request.line(),
            if ack.checkpoint_saved { "ok" } else { "checkpoint-error" }))
    }
}

fn atomic_marker(directory: &Path, name: &str, content: &str) -> Result<()> {
    let target = directory.join(name);
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        ensure!(metadata.is_file() && !metadata.file_type().is_symlink(), "Unsafe lifecycle output marker");
    }
    let temporary = directory.join(format!(".{name}.{}.{}", std::process::id(), NEXT_FILE.fetch_add(1, Ordering::Relaxed)));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, &target)?;
        Ok(())
    })();
    if result.is_err() { let _ = fs::remove_file(&temporary); }
    result
}

fn dispatch(event: PowerEvent) {
    BINDING.with(|slot| {
        let Some(mut binding) = slot.borrow_mut().take() else { return; };
        let request = binding.context.as_ref().and_then(|context| context.request(event));
        let permitted = event == PowerEvent::Quit || binding.context.is_none() || request.is_some();
        if permitted {
            let duplicate = request.as_ref().and_then(|request| binding.last_ack.as_ref().filter(|(last, _)| last == request).map(|(_, ack)| *ack));
            let stale = request.as_ref().zip(binding.last_ack.as_ref())
                .is_some_and(|(request, (last, _))| request.sequence <= last.sequence && request != last);
            if !stale {
                if let (Some(context), Some(request)) = (&binding.context, request.as_ref()) {
                    context.presentation.set_resume_sequence(if event == PowerEvent::Resume { request.sequence } else { 0 });
                }
                let result = duplicate.map(Ok).unwrap_or_else(|| (binding.callback)(event));
                if let Some(context) = &binding.context { context.presentation.set_resume_sequence(0); }
                match result {
                    Ok(ack) => {
                        if let (Some(context), Some(request)) = (&binding.context, request) {
                            if let Err(error) = context.acknowledge(&request, ack) {
                                eprintln!("bilicomics: lifecycle acknowledgement failed: {error:#}");
                            } else { binding.last_ack = Some((request, ack)); }
                        }
                    }
                    Err(error) => eprintln!("bilicomics: lifecycle transition failed: {error:#}"),
                }
            }
        }
        *slot.borrow_mut() = Some(binding);
    });
}

fn event_from_byte(value: u8) -> Option<PowerEvent> {
    match value { b'S' => Some(PowerEvent::Suspend), b'R' => Some(PowerEvent::Resume), b'T' => Some(PowerEvent::Quit), _ => None }
}

pub struct SignalBridge {
    context: Option<LaunchContext>,
    #[cfg(unix)] read: std::os::fd::OwnedFd,
    #[cfg(unix)] write: std::os::fd::OwnedFd,
    #[cfg(unix)] previous: Vec<(libc::c_int, libc::sighandler_t)>,
    #[cfg(unix)] watcher: Option<std::thread::JoinHandle<()>>,
    #[cfg(unix)] stopping: Arc<AtomicBool>,
}

impl SignalBridge {
    /// Clone presentation ownership before moving the bridge into its event loop.
    pub fn native_presentation_context(&self) -> Option<NativePresentationContext> {
        self.context.as_ref().map(|context| context.presentation.clone())
    }

    /// Install handlers and publish readiness without starting a thread.
    pub fn install() -> Result<Self> {
        QUIT_REQUESTED.store(false, Ordering::Release);
        let context = LaunchContext::from_environment()?;
        #[cfg(unix)] {
            use std::os::fd::{AsRawFd, FromRawFd};
            let mut descriptors = [-1; 2];
            // Both descriptors are owned immediately after a successful pipe call.
            ensure!(unsafe { libc::pipe(descriptors.as_mut_ptr()) } == 0, "Create lifecycle signal pipe");
            let read = unsafe { std::os::fd::OwnedFd::from_raw_fd(descriptors[0]) };
            let write = unsafe { std::os::fd::OwnedFd::from_raw_fd(descriptors[1]) };
            for descriptor in [read.as_raw_fd(), write.as_raw_fd()] {
                ensure!(unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) } >= 0, "Configure lifecycle pipe inheritance");
                let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
                ensure!(flags >= 0 && unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0, "Configure lifecycle pipe blocking");
            }
            if SIGNAL_WRITE.compare_exchange(-1, write.as_raw_fd(), Ordering::SeqCst, Ordering::SeqCst).is_err() {
                bail!("The lifecycle signal bridge is already installed");
            }
            let mut bridge = Self { context, read, write, previous: Vec::new(), watcher: None, stopping: Arc::new(AtomicBool::new(false)) };
            for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGUSR1, libc::SIGUSR2] {
                let previous = unsafe { libc::signal(signal, signal_handler as *const () as libc::sighandler_t) };
                ensure!(previous != libc::SIG_ERR, "Install lifecycle signal handler");
                bridge.previous.push((signal, previous));
            }
            bridge.publish_ready()?;
            Ok(bridge)
        }
        #[cfg(not(unix))] {
            let bridge = Self { context };
            bridge.publish_ready()?;
            Ok(bridge)
        }
    }

    fn publish_ready(&self) -> Result<()> {
        if let Some(context) = &self.context {
            atomic_marker(&context.directory, "lifecycle-ready", &format!("{}\n", context.header()))?;
        }
        Ok(())
    }

    /// Bind on the UI thread after backend installation, then start the pipe reader.
    pub fn start(&mut self, callback: impl FnMut(PowerEvent) -> Result<PowerAck> + 'static) -> Result<()> {
        BINDING.with(|slot| *slot.borrow_mut() = Some(Binding { callback: Box::new(callback), context: self.context.clone(), last_ack: None }));
        #[cfg(unix)] {
            use std::os::fd::AsRawFd;
            // Process early requests before the native event loop acquires input.
            drain_events(self.read.as_raw_fd(), false);
            let read = self.read.try_clone()?;
            let stopping = self.stopping.clone();
            self.watcher = Some(std::thread::Builder::new().name("bilicomics-power-events".into()).spawn(move || {
                let descriptor = read.as_raw_fd();
                while !stopping.load(Ordering::Acquire) {
                    let mut poll = libc::pollfd { fd: descriptor, events: libc::POLLIN, revents: 0 };
                    let result = unsafe { libc::poll(&mut poll, 1, -1) };
                    if result < 0 {
                        if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) { continue; }
                        break;
                    }
                    if poll.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 { break; }
                    if !drain_events(descriptor, true) { break; }
                }
            })?);
        }
        Ok(())
    }

    pub fn quit_requested(&self) -> bool { QUIT_REQUESTED.load(Ordering::Acquire) }

    /// Stop advertising signal readiness before the event loop tears down.
    pub fn retire(&self) {
        if let Some(context) = &self.context {
            if context.read("lifecycle-ready").ok().is_some_and(|value| value.trim() == context.header()) {
                let _ = fs::remove_file(context.directory.join("lifecycle-ready"));
            }
        }
    }
}

#[cfg(unix)]
extern "C" fn signal_handler(signal: libc::c_int) {
    #[cfg(target_os = "linux")]
    let previous_errno = unsafe { *libc::__errno_location() };
    let value = match signal {
        libc::SIGUSR1 => b'S', libc::SIGUSR2 => b'R',
        libc::SIGTERM | libc::SIGINT => { QUIT_REQUESTED.store(true, Ordering::Release); b'T' },
        _ => return,
    };
    let descriptor = SIGNAL_WRITE.load(Ordering::Acquire);
    if descriptor >= 0 {
        // A one-byte nonblocking write and lock-free atomics are signal-safe.
        let _ = unsafe { libc::write(descriptor, (&value as *const u8).cast(), 1) };
    }
    #[cfg(target_os = "linux")]
    unsafe { *libc::__errno_location() = previous_errno; }
}

#[cfg(unix)]
fn drain_events(descriptor: libc::c_int, asynchronous: bool) -> bool {
    if QUIT_REQUESTED.load(Ordering::Acquire) {
        if asynchronous { let _ = slint::invoke_from_event_loop(|| dispatch(PowerEvent::Quit)); }
        else { dispatch(PowerEvent::Quit); }
        return false;
    }
    let mut buffer = [0u8; 64];
    loop {
        let count = unsafe { libc::read(descriptor, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count <= 0 {
            if count < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) { continue; }
            return count != 0;
        }
        for value in &buffer[..count as usize] {
            if *value == b'Q' { return false; }
            if let Some(event) = event_from_byte(*value) {
                if asynchronous {
                    if slint::invoke_from_event_loop(move || dispatch(event)).is_err() { return false; }
                } else { dispatch(event); }
            }
        }
    }
}

impl Drop for SignalBridge {
    fn drop(&mut self) {
        self.retire();
        #[cfg(unix)] {
            use std::os::fd::AsRawFd;
            SIGNAL_WRITE.store(-1, Ordering::Release);
            for (signal, previous) in self.previous.iter().rev() { unsafe { libc::signal(*signal, *previous); } }
            if self.watcher.is_some() {
                self.stopping.store(true, Ordering::Release);
                let value = b'Q';
                let _ = unsafe { libc::write(self.write.as_raw_fd(), (&value as *const u8).cast(), 1) };
                if let Some(watcher) = self.watcher.take() { let _ = watcher.join(); }
            }
        }
        BINDING.with(|slot| *slot.borrow_mut() = None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct PowerPackets(Vec<Vec<u8>>);

    impl Write for PowerPackets {
        fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
            self.0.push(value.to_owned());
            Ok(value.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    struct PowerChunks { bytes: std::io::Cursor<Vec<u8>>, size: usize }

    impl Read for PowerChunks {
        fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
            let length = target.len().min(self.size);
            self.bytes.read(&mut target[..length])
        }
    }

    #[test]
    fn native_power_relay_frames_split_and_bulk_input_as_atomic_short_records() {
        let records = ["goingToScreenSaver 2\n", "outOfScreenSaver 1\n", "exitingScreenSaver 1\n",
            "wakeupFromSuspend 0\n", "usbConfigured 1\n"];
        let bytes = records.concat().into_bytes();
        for size in [1, 2, 17, 256, 4096] {
            let mut output = PowerPackets::default();
            relay_native_power_records(PowerChunks { bytes: std::io::Cursor::new(bytes.clone()), size }, &mut output).unwrap();
            assert_eq!(output.0, records.iter().map(|value| value.as_bytes().to_owned()).collect::<Vec<_>>());
        }
    }

    #[test]
    fn native_power_relay_rejects_unknown_binary_and_incomplete_records() {
        for record in [b"unknown 1\n".as_slice(), b"\n", b"goingToScreenSaver 1\r\n",
            b"goingToScreenSaver\x00 1\n", b"outOfScreenSaver 1"] {
            let mut output = PowerPackets::default();
            assert!(relay_native_power_records(record, &mut output).is_err());
            assert!(output.0.is_empty());
        }
        let mut output = PowerPackets::default();
        assert!(relay_native_power_records(b"wakeupFromSuspend 0\npartial".as_slice(), &mut output).is_err());
        assert_eq!(output.0, vec![b"wakeupFromSuspend 0\n".to_vec()]);
    }

    #[test]
    fn native_power_relay_bounds_unterminated_input_before_allocating_another_record() {
        let prefix = b"goingToScreenSaver ";
        let mut maximum = prefix.to_vec();
        maximum.resize(MAX_NATIVE_POWER_RECORD_BYTES - 1, b'1');
        maximum.push(b'\n');
        let mut output = PowerPackets::default();
        relay_native_power_records(maximum.as_slice(), &mut output).unwrap();
        assert_eq!(output.0, vec![maximum.clone()]);
        for record in [vec![b'x'; MAX_NATIVE_POWER_RECORD_BYTES], {
            let mut value = maximum;
            value.insert(value.len() - 1, b'1');
            value
        }] {
            let mut output = PowerPackets::default();
            assert!(relay_native_power_records(record.as_slice(), &mut output).is_err());
            assert!(output.0.is_empty());
        }
    }

    #[test]
    fn native_power_relay_never_retries_a_partial_output_write() {
        struct Short { writes: usize }
        impl Write for Short {
            fn write(&mut self, value: &[u8]) -> std::io::Result<usize> { self.writes += 1; Ok(value.len() - 1) }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let mut output = Short { writes: 0 };
        assert!(relay_native_power_records(b"wakeupFromSuspend 0\n".as_slice(), &mut output).is_err());
        assert_eq!(output.writes, 1);
    }

    #[test]
    fn native_power_relay_retries_only_interrupted_reads_and_writes() {
        struct InterruptedRead { first: bool, bytes: std::io::Cursor<Vec<u8>> }
        impl Read for InterruptedRead {
            fn read(&mut self, value: &mut [u8]) -> std::io::Result<usize> {
                if self.first { self.first = false; return Err(std::io::ErrorKind::Interrupted.into()); }
                self.bytes.read(value)
            }
        }
        struct InterruptedWrite { first: bool, packets: PowerPackets }
        impl Write for InterruptedWrite {
            fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
                if self.first { self.first = false; return Err(std::io::ErrorKind::Interrupted.into()); }
                self.packets.write(value)
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let bytes = b"goingToScreenSaver 2\n".to_vec();
        let mut output = InterruptedWrite { first: true, packets: PowerPackets::default() };
        relay_native_power_records(InterruptedRead { first: true, bytes: std::io::Cursor::new(bytes.clone()) }, &mut output).unwrap();
        assert_eq!(output.packets.0, vec![bytes]);
    }

    fn context(directory: &Path) -> LaunchContext {
        LaunchContext::new(directory.to_owned(), "0123456789abcdef0123456789abcdef".into(), 1).unwrap()
    }

    #[test]
    fn lifecycle_requests_reject_stale_contexts_and_unrecognized_actions() {
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path());
        atomic_marker(directory.path(), "lifecycle-request", "2:0123456789abcdef0123456789abcdef:1:suspend\n").unwrap();
        assert!(context.request(PowerEvent::Suspend).is_none());
        atomic_marker(directory.path(), "lifecycle-request", "1:ffffffffffffffffffffffffffffffff:1:suspend\n").unwrap();
        assert!(context.request(PowerEvent::Suspend).is_none());
        atomic_marker(directory.path(), "lifecycle-request", "1:0123456789abcdef0123456789abcdef:1:resume\n").unwrap();
        assert!(context.request(PowerEvent::Suspend).is_none());
        assert!(context.request(PowerEvent::Resume).is_some());
        assert!(Request::parse("1:0123456789abcdef0123456789abcdef:0:suspend").is_none());
        assert!(Request::parse("1:0123456789abcdef0123456789abcdef:1:execute").is_none());
    }

    #[test]
    fn duplicate_power_events_acknowledge_without_repeating_checkpoint_work() {
        use std::{cell::Cell, rc::Rc};
        let directory = tempfile::tempdir().unwrap();
        let context = context(directory.path());
        atomic_marker(directory.path(), "lifecycle-request", "1:0123456789abcdef0123456789abcdef:4:suspend\n").unwrap();
        let calls = Rc::new(Cell::new(0));
        let counter = calls.clone();
        BINDING.with(|slot| *slot.borrow_mut() = Some(Binding { context: Some(context.clone()), last_ack: None,
            callback: Box::new(move |_| { counter.set(counter.get() + 1); Ok(PowerAck::checkpoint_error()) }) }));
        dispatch(PowerEvent::Suspend);
        dispatch(PowerEvent::Suspend);
        assert_eq!(calls.get(), 1);
        assert_eq!(context.read("lifecycle-ack").unwrap().trim(), "1:0123456789abcdef0123456789abcdef:4:suspend:checkpoint-error");
        atomic_marker(directory.path(), "lifecycle-request", "1:0123456789abcdef0123456789abcdef:3:resume\n").unwrap();
        dispatch(PowerEvent::Resume);
        assert_eq!(calls.get(), 1);
        BINDING.with(|slot| *slot.borrow_mut() = None);
    }

    #[cfg(target_os = "linux")]
    fn presentation_context(directory: &Path) -> LaunchContext {
        let context = context(directory);
        // The fixture uses this test process and its existing parent chain;
        // no native services, display, input devices, or signals are involved.
        // SAFETY: getppid has no pointer arguments and only reads ancestry.
        let parent = unsafe { libc::getppid() };
        let status = fs::read_to_string(format!("/proc/{parent}/status")).unwrap();
        let master = status.lines().find_map(|line| line.strip_prefix("PPid:")).unwrap().trim();
        for (name, value) in [("nonce", context.token.clone()), ("generation", context.header()),
            ("pid", master.to_owned()), ("application-pid", std::process::id().to_string())] {
            atomic_marker(directory, name, &format!("{value}\n")).unwrap();
        }
        context
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn resume_snapshot_is_available_inside_the_detached_callback_and_not_reused_for_duplicates() {
        use std::{cell::Cell, rc::Rc};
        let directory = tempfile::tempdir().unwrap();
        let context = presentation_context(directory.path());
        let request = format!("{}:4:resume", context.header());
        atomic_marker(directory.path(), "lifecycle-request", &format!("{request}\n")).unwrap();
        atomic_marker(directory.path(), "resume-native-epdc-mode", &format!("{}:4:Y8INV\n", context.header())).unwrap();
        let presentation = context.presentation.clone();
        let callback_presentation = presentation.clone();
        let calls = Rc::new(Cell::new(0));
        let counter = calls.clone();
        BINDING.with(|slot| *slot.borrow_mut() = Some(Binding { context: Some(context.clone()), last_ack: None,
            callback: Box::new(move |event| {
                assert_eq!(event, PowerEvent::Resume);
                BINDING.with(|slot| assert!(slot.borrow().is_none()));
                assert!(callback_presentation.resume_mode()?.inverted());
                counter.set(counter.get() + 1);
                Ok(PowerAck::saved())
            }) }));
        dispatch(PowerEvent::Resume);
        assert!(presentation.resume_mode().is_err());
        dispatch(PowerEvent::Resume);
        assert_eq!(calls.get(), 1);
        assert_eq!(context.read("lifecycle-ack").unwrap().trim(), format!("{request}:ok"));
        BINDING.with(|slot| *slot.borrow_mut() = None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn missing_resume_snapshot_is_not_acknowledged_and_can_be_reconciled() {
        let directory = tempfile::tempdir().unwrap();
        let context = presentation_context(directory.path());
        atomic_marker(directory.path(), "lifecycle-request", &format!("{}:2:resume\n", context.header())).unwrap();
        let presentation = context.presentation.clone();
        BINDING.with(|slot| *slot.borrow_mut() = Some(Binding { context: Some(context.clone()), last_ack: None,
            callback: Box::new(move |_| { presentation.resume_mode()?; Ok(PowerAck::saved()) }) }));
        dispatch(PowerEvent::Resume);
        assert!(!directory.path().join("lifecycle-ack").exists());
        assert!(context.presentation.resume_mode().is_err());
        atomic_marker(directory.path(), "resume-native-epdc-mode", &format!("{}:2:Y8\n", context.header())).unwrap();
        dispatch(PowerEvent::Resume);
        assert!(directory.path().join("lifecycle-ack").exists());
        BINDING.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn lifecycle_markers_do_not_follow_symlinks() {
        #[cfg(unix)] {
            let directory = tempfile::tempdir().unwrap();
            let outside = directory.path().join("outside");
            fs::write(&outside, "keep").unwrap();
            std::os::unix::fs::symlink(&outside, directory.path().join("lifecycle-ack")).unwrap();
            assert!(atomic_marker(directory.path(), "lifecycle-ack", "replace").is_err());
            assert_eq!(fs::read_to_string(outside).unwrap(), "keep");
        }
    }
}

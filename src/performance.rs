//! Opt-in, bounded software timing records without application ownership.
//!
//! Callers use static operation/stage labels and numeric diagnostics only. Never
//! include account data, content identifiers, paths, URLs, credentials or pixels.
//! Records are serialized before the short append lock. Appending still has an
//! observer cost, so collect stage samples first and emit outside measured spans.

use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::{Mutex, MutexGuard, OnceLock, atomic::{AtomicBool, AtomicU64, Ordering}},
    thread::ThreadId,
    time::Instant,
};

use serde::Serialize;

pub const MAX_TRACE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 64 * 1024;
const TERMINAL_RESERVE_BYTES: u64 = 1024;
const SCHEMA_VERSION: u32 = 1;

static REGISTRY: Registry = Registry::new();

/// Configure this process once, creating a new private regular file.
///
/// Existing files are never overwritten. An unsuccessful attempt to open the
/// file leaves configuration available for a later explicit retry. Once a sink
/// is installed, closing or disabling it never permits a second configuration.
pub fn configure(path: &Path) -> io::Result<()> {
    REGISTRY.configure(path, MAX_TRACE_BYTES)
}

pub fn enabled() -> bool {
    REGISTRY.enabled()
}

/// Close the optional sink after a final best-effort record. No writer thread
/// is joined, but this serializes with an append that may already be in progress.
/// Diagnostic I/O errors are retained in status rather than returned to shutdown.
pub fn finish() {
    if let Some(sink) = REGISTRY.sink.get() {
        sink.stop(StopReason::Finished);
    }
}

/// Exposes diagnostic completeness even when a failed file cannot accept a
/// terminal record. The status contains no file path or other private input.
pub fn status() -> TraceStatus {
    REGISTRY.sink.get().map_or_else(TraceStatus::default, Sink::status)
}

#[derive(Clone, Copy, Debug)]
pub struct Operation {
    id: u64,
    kind: &'static str,
}

impl Operation {
    /// Disabled diagnostics neither allocate an identity nor sample a clock.
    pub fn new(kind: &'static str) -> Option<Self> {
        REGISTRY.operation(kind)
    }

    pub fn id(self) -> u64 { self.id }

    #[cfg(test)]
    pub(crate) fn fixture(id: u64, kind: &'static str) -> Self { Self { id, kind } }

    pub fn mark(self, stage: &'static str) {
        self.record(stage, &());
    }

    /// The caller should only construct data after obtaining an Operation.
    /// A disabled sink never invokes the supplied value's serializer.
    pub fn record<T: Serialize + ?Sized>(self, stage: &'static str, data: &T) {
        record_for_id(self.id, self.kind, stage, data);
    }
}

pub fn startup<T: Serialize + ?Sized>(stage: &'static str, data: &T) {
    record_for_id(0, "startup", stage, data);
}

/// Bridge a numeric operation identity to separately owned backend diagnostics.
pub fn record_for_id<T: Serialize + ?Sized>(
    operation_id: u64, kind: &'static str, stage: &'static str, data: &T,
) {
    REGISTRY.record(operation_id, kind, stage, data);
}

/// Absolute monotonic clocks permit correlation across UI and worker threads.
/// The private Instant also supplies portable elapsed wall time for spans.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Stamp {
    pub monotonic_ns: Option<u64>,
    pub thread_cpu_ns: Option<u64>,
    pub thread_id: Option<u64>,
    #[serde(skip)]
    instant: Instant,
    #[serde(skip)]
    owner: ThreadId,
}

impl Stamp {
    pub fn now() -> Self {
        Self {
            instant: Instant::now(),
            owner: std::thread::current().id(),
            monotonic_ns: monotonic_ns(),
            thread_cpu_ns: thread_cpu_ns(),
            thread_id: native_thread_id(),
        }
    }

    /// CPU time is meaningful only on the same thread with two valid readings.
    pub fn elapsed_since(&self, started: &Self) -> SpanSample {
        let same_thread = self.owner == started.owner
            && self.thread_id.zip(started.thread_id).is_some_and(|(end, begin)| end == begin);
        let cpu_us = if same_thread {
            self.thread_cpu_ns.zip(started.thread_cpu_ns)
                .and_then(|(end, begin)| end.checked_sub(begin)).map(|ns| ns / 1000)
        } else { None };
        SpanSample {
            wall_us: micros(self.instant.saturating_duration_since(started.instant)),
            cpu_us,
        }
    }
}

#[derive(Debug)]
pub struct Span { started: Stamp }

impl Span {
    /// Call only inside an explicitly enabled profiling branch.
    pub fn start() -> Self { Self { started: Stamp::now() } }
    pub fn finish(self) -> SpanSample { Stamp::now().elapsed_since(&self.started) }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SpanSample {
    pub wall_us: u64,
    pub cpu_us: Option<u64>,
}

/// Aggregate completed and partial attempts without treating missing CPU time
/// as zero. After one unknown attempt, aggregate CPU remains unknown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct StageTotals {
    pub wall_us: u64,
    pub cpu_us: Option<u64>,
    pub attempts: u32,
    pub completed: u32,
    pub partial: u32,
}

impl StageTotals {
    pub fn record(&mut self, sample: SpanSample, complete: bool) {
        self.cpu_us = if self.attempts == 0 { sample.cpu_us }
            else { self.cpu_us.zip(sample.cpu_us).and_then(|(total, value)| total.checked_add(value)) };
        self.wall_us = self.wall_us.saturating_add(sample.wall_us);
        self.attempts = self.attempts.saturating_add(1);
        if complete { self.completed = self.completed.saturating_add(1); }
        else { self.partial = self.partial.saturating_add(1); }
    }
}

pub type StageMap = BTreeMap<&'static str, StageTotals>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Finished,
    ByteLimit,
    RecordTooLarge,
    SerializationError,
    IoError,
    OperationIdsExhausted,
    Poisoned,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct TraceStatus {
    pub configured: bool,
    pub enabled: bool,
    pub complete_bytes: u64,
    pub complete_records: u64,
    pub stopped_reason: Option<StopReason>,
    pub terminal_written: bool,
    pub output_integrity_unknown: bool,
}

struct Registry {
    configuration: Mutex<()>,
    sink: OnceLock<Sink<File>>,
}

impl Registry {
    const fn new() -> Self {
        Self { configuration: Mutex::new(()), sink: OnceLock::new() }
    }

    fn configure(&self, path: &Path, max_bytes: u64) -> io::Result<()> {
        // Serialize configuration before creating a file, so a losing caller
        // cannot leave an unused file at a different candidate pathname.
        let _configuration = self.configuration.lock()
            .map_err(|_| io::Error::other("Performance trace configuration lock failed"))?;
        if self.sink.get().is_some() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "Performance trace was already configured"));
        }
        let mut options = OpenOptions::new();
        options.append(true).create_new(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Performance trace requires a regular file"));
        }
        let sink = Sink::new(file, max_bytes);
        #[derive(Serialize)]
        struct Header { max_trace_bytes: u64, max_record_bytes: usize }
        sink.emit(0, "diagnostic", "trace_started", &Header {
            max_trace_bytes: max_bytes, max_record_bytes: MAX_RECORD_BYTES,
        });
        if !sink.active.load(Ordering::Acquire) {
            return Err(io::Error::other("Performance trace could not write its initial record"));
        }
        self.sink.set(sink)
            .map_err(|_| io::Error::new(io::ErrorKind::AlreadyExists, "Performance trace was already configured"))
    }

    fn enabled(&self) -> bool {
        self.sink.get().is_some_and(|sink| sink.active.load(Ordering::Acquire))
    }

    fn operation(&self, kind: &'static str) -> Option<Operation> {
        let sink = self.sink.get()?;
        if !sink.active.load(Ordering::Acquire) { return None; }
        match sink.next_operation.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1)) {
            Ok(id) => Some(Operation { id, kind }),
            Err(_) => { sink.stop(StopReason::OperationIdsExhausted); None }
        }
    }

    fn record<T: Serialize + ?Sized>(&self, id: u64, kind: &'static str, stage: &'static str, data: &T) {
        if let Some(sink) = self.sink.get() { sink.emit(id, kind, stage, data); }
    }
}

trait TraceOutput: Write + Send {
    /// Production files use O_APPEND, so truncation does not require seeking
    /// the descriptor before a later terminal append.
    fn rollback(&mut self, complete_bytes: u64) -> io::Result<()>;
}

impl TraceOutput for File {
    fn rollback(&mut self, complete_bytes: u64) -> io::Result<()> { self.set_len(complete_bytes) }
}

struct Sink<W: TraceOutput> {
    active: AtomicBool,
    next_operation: AtomicU64,
    started: Instant,
    limit: u64,
    state: Mutex<SinkState<W>>,
}

struct SinkState<W> {
    output: Option<W>,
    bytes: u64,
    records: u64,
    stopped_reason: Option<StopReason>,
    terminal_attempted: bool,
    terminal_written: bool,
    output_integrity_unknown: bool,
}

#[derive(Serialize)]
struct Record<'a, T: ?Sized> {
    schema_version: u32,
    process_id: u32,
    operation_id: u64,
    kind: &'static str,
    stage: &'static str,
    monotonic_ns: Option<u64>,
    thread_cpu_ns: Option<u64>,
    thread_id: Option<u64>,
    trace_elapsed_us: u64,
    data: &'a T,
}

impl<W: TraceOutput> Sink<W> {
    fn new(output: W, limit: u64) -> Self {
        Self { active: AtomicBool::new(true), next_operation: AtomicU64::new(1),
            started: Instant::now(), limit,
            state: Mutex::new(SinkState { output: Some(output), bytes: 0, records: 0,
                stopped_reason: None, terminal_attempted: false, terminal_written: false,
                output_integrity_unknown: false }) }
    }

    fn lock(&self) -> MutexGuard<'_, SinkState<W>> {
        self.state.lock().unwrap_or_else(|error| {
            self.active.store(false, Ordering::Release);
            let mut state = error.into_inner();
            state.stopped_reason.get_or_insert(StopReason::Poisoned);
            state.output_integrity_unknown = true;
            state.output.take();
            state
        })
    }

    fn serialize<T: Serialize + ?Sized>(&self, id: u64, kind: &'static str,
        stage: &'static str, data: &T) -> Result<Vec<u8>, StopReason>
    {
        let stamp = Stamp::now();
        let record = Record { schema_version: SCHEMA_VERSION, process_id: std::process::id(),
            operation_id: id, kind, stage, monotonic_ns: stamp.monotonic_ns,
            thread_cpu_ns: stamp.thread_cpu_ns, thread_id: stamp.thread_id,
            trace_elapsed_us: micros(stamp.instant.saturating_duration_since(self.started)), data };
        let mut buffer = LineBuffer::new(MAX_RECORD_BYTES - 1);
        if serde_json::to_writer(&mut buffer, &record).is_err() {
            return Err(if buffer.exceeded { StopReason::RecordTooLarge } else { StopReason::SerializationError });
        }
        if buffer.bytes.len() == buffer.bytes.capacity() { buffer.bytes.reserve_exact(1); }
        buffer.bytes.push(b'\n');
        Ok(buffer.bytes)
    }

    fn emit<T: Serialize + ?Sized>(&self, id: u64, kind: &'static str, stage: &'static str, data: &T) {
        if !self.active.load(Ordering::Acquire) { return; }
        let line = match self.serialize(id, kind, stage, data) {
            Ok(line) => line,
            Err(reason) => { self.stop(reason); return; }
        };
        let reason = {
            let mut state = self.lock();
            if state.stopped_reason.is_some() { return; }
            let reason = if state.bytes.saturating_add(line.len() as u64)
                > self.limit.saturating_sub(TERMINAL_RESERVE_BYTES)
            {
                Some(StopReason::ByteLimit)
            } else if append_line(&mut state, &line).is_err() {
                Some(StopReason::IoError)
            } else { None };
            if let Some(reason) = reason {
                state.stopped_reason = Some(reason);
                self.active.store(false, Ordering::Release);
            }
            reason
        };
        if let Some(reason) = reason { self.write_terminal(reason); }
    }

    fn stop(&self, reason: StopReason) {
        let reason = {
            let mut state = self.lock();
            if state.terminal_attempted { return; }
            let reason = *state.stopped_reason.get_or_insert(reason);
            self.active.store(false, Ordering::Release);
            reason
        };
        // A concurrent emitter may have stopped admission but not yet written
        // its terminal record. Finish that same transition before returning.
        self.write_terminal(reason);
    }

    fn write_terminal(&self, reason: StopReason) {
        #[derive(Serialize)]
        struct Terminal { reason: StopReason, truncated: bool }
        // This fixed payload has no error string, path or caller-supplied data.
        // Its serialized record fits the reserved region at all integer widths.
        let line = self.serialize(0, "diagnostic", "trace_stopped", &Terminal {
            reason, truncated: reason != StopReason::Finished,
        }).ok();
        let output = {
            let mut state = self.lock();
            if state.terminal_attempted { return; }
            state.terminal_attempted = true;
            if !state.output_integrity_unknown {
                if let Some(line) = line.filter(|line| state.bytes.saturating_add(line.len() as u64) <= self.limit) {
                    state.terminal_written = append_line(&mut state, &line).is_ok();
                }
            }
            state.output.take()
        };
        drop(output);
    }

    fn status(&self) -> TraceStatus {
        let state = self.lock();
        TraceStatus { configured: true, enabled: self.active.load(Ordering::Acquire),
            complete_bytes: state.bytes, complete_records: state.records,
            stopped_reason: state.stopped_reason, terminal_written: state.terminal_written,
            output_integrity_unknown: state.output_integrity_unknown }
    }
}

fn append_line<W: TraceOutput>(state: &mut SinkState<W>, line: &[u8]) -> io::Result<()> {
    let output = state.output.as_mut().ok_or_else(|| io::Error::other("Performance trace is closed"))?;
    if let Err(error) = output.write_all(line) {
        if output.rollback(state.bytes).is_err() { state.output_integrity_unknown = true; }
        return Err(error);
    }
    state.bytes += line.len() as u64;
    state.records += 1;
    Ok(())
}

struct LineBuffer {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl LineBuffer {
    fn new(limit: usize) -> Self {
        Self { bytes: Vec::with_capacity(limit.min(1024)), limit, exceeded: false }
    }
}

impl Write for LineBuffer {
    fn write(&mut self, value: &[u8]) -> io::Result<usize> {
        if value.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::new(io::ErrorKind::FileTooLarge, "Performance record exceeds its limit"));
        }
        // Grow geometrically, but never request payload capacity beyond the
        // record budget. Allocator bookkeeping is outside the payload count.
        let needed = self.bytes.len() + value.len();
        if needed > self.bytes.capacity() {
            let capacity = self.bytes.capacity().saturating_mul(2).max(needed).min(self.limit);
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.bytes.extend_from_slice(value);
        Ok(value.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

fn micros(duration: std::time::Duration) -> u64 {
    duration.as_micros().try_into().unwrap_or(u64::MAX)
}

#[cfg(target_os = "linux")]
fn clock_ns(clock: libc::clockid_t) -> Option<u64> {
    let mut stamp = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // Both clock IDs are read-only observations of the calling process/thread.
    if unsafe { libc::clock_gettime(clock, &mut stamp) } != 0
        || !(0..1_000_000_000).contains(&stamp.tv_nsec)
    { return None; }
    u64::try_from(stamp.tv_sec).ok()?.checked_mul(1_000_000_000)?
        .checked_add(u64::try_from(stamp.tv_nsec).ok()?)
}

#[cfg(target_os = "linux")]
fn monotonic_ns() -> Option<u64> { clock_ns(libc::CLOCK_MONOTONIC) }
#[cfg(target_os = "linux")]
fn thread_cpu_ns() -> Option<u64> { clock_ns(libc::CLOCK_THREAD_CPUTIME_ID) }
#[cfg(target_os = "linux")]
fn native_thread_id() -> Option<u64> {
    // gettid is a read-only identity query for the calling Linux thread.
    let id = unsafe { libc::syscall(libc::SYS_gettid) };
    (id > 0).then(|| id as u64)
}

#[cfg(not(target_os = "linux"))]
fn monotonic_ns() -> Option<u64> { None }
#[cfg(not(target_os = "linux"))]
fn thread_cpu_ns() -> Option<u64> { None }
#[cfg(not(target_os = "linux"))]
fn native_thread_id() -> Option<u64> { None }

#[cfg(test)]
mod tests;

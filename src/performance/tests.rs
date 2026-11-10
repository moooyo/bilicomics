use super::*;
use serde::{Serializer, ser::{Error as _, SerializeSeq}};
use std::{sync::{Arc, atomic::{AtomicUsize, Ordering}}, time::Duration};

struct MustNotSerialize;

impl Serialize for MustNotSerialize {
    fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        panic!("Disabled diagnostics invoked serialization");
    }
}

fn records(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path).unwrap().lines()
        .map(|line| serde_json::from_str(line).unwrap()).collect()
}

#[test]
fn disabled_and_finished_sinks_skip_serialization_and_operation_ids() {
    let registry = Registry::new();
    assert!(!registry.enabled());
    assert!(registry.operation("reader").is_none());
    registry.record(0, "reader", "disabled", &MustNotSerialize);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("trace.jsonl");
    registry.configure(&path, MAX_TRACE_BYTES).unwrap();
    let sink = registry.sink.get().unwrap();
    assert_eq!(registry.operation("reader").unwrap().id(), 1);
    sink.stop(StopReason::Finished);
    let bytes = std::fs::read(&path).unwrap();
    assert!(registry.operation("reader").is_none());
    registry.record(1, "reader", "disabled", &MustNotSerialize);
    assert_eq!(sink.next_operation.load(Ordering::Relaxed), 2);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(!registry.enabled());
    assert_eq!(records(&path).last().unwrap()["data"]["truncated"], false);
}

#[test]
fn concurrent_configuration_creates_only_the_winning_file() {
    let directory = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new());
    let results = std::thread::scope(|scope| {
        let workers = (0..8).map(|index| {
            let registry = registry.clone();
            let path = directory.path().join(format!("trace-{index}.jsonl"));
            scope.spawn(move || registry.configure(&path, MAX_TRACE_BYTES))
        }).collect::<Vec<_>>();
        workers.into_iter().map(|worker| worker.join().unwrap()).collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(results.iter().filter_map(|result| result.as_ref().err())
        .all(|error| error.kind() == io::ErrorKind::AlreadyExists));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    registry.sink.get().unwrap().stop(StopReason::Finished);
    assert!(registry.configure(&directory.path().join("another.jsonl"), MAX_TRACE_BYTES).is_err());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn existing_outputs_are_preserved_and_successful_output_is_private() {
    let directory = tempfile::tempdir().unwrap();
    let existing = directory.path().join("existing.jsonl");
    std::fs::write(&existing, b"existing evidence\n").unwrap();
    let registry = Registry::new();
    assert!(registry.configure(&existing, MAX_TRACE_BYTES).is_err());
    assert_eq!(std::fs::read(&existing).unwrap(), b"existing evidence\n");
    assert!(registry.configure(directory.path(), MAX_TRACE_BYTES).is_err());
    #[cfg(unix)] {
        use std::os::unix::fs::symlink;
        let link = directory.path().join("symlink.jsonl");
        symlink(&existing, &link).unwrap();
        assert!(registry.configure(&link, MAX_TRACE_BYTES).is_err());
        assert_eq!(std::fs::read(&existing).unwrap(), b"existing evidence\n");
    }
    let output = directory.path().join("new.jsonl");
    registry.configure(&output, MAX_TRACE_BYTES).unwrap();
    let metadata = std::fs::metadata(&output).unwrap();
    assert!(metadata.is_file());
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(metadata.permissions().mode() & 0o077, 0);
    }
    registry.sink.get().unwrap().stop(StopReason::Finished);
}

#[test]
fn byte_limit_keeps_whole_json_lines_and_one_terminal_record() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bounded.jsonl");
    let registry = Registry::new();
    registry.configure(&path, 4096).unwrap();
    for index in 0..100 {
        registry.record(index, "reader", "sample", &index);
    }
    let sink = registry.sink.get().unwrap();
    let status = sink.status();
    assert!(!status.enabled);
    assert_eq!(status.stopped_reason, Some(StopReason::ByteLimit));
    assert!(status.terminal_written);
    assert!(!status.output_integrity_unknown);
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.len() <= 4096);
    assert_eq!(bytes.len() as u64, status.complete_bytes);
    assert_eq!(bytes.last(), Some(&b'\n'));
    let values = records(&path);
    assert_eq!(values.len() as u64, status.complete_records);
    assert_eq!(values.iter().filter(|value| value["stage"] == "trace_stopped").count(), 1);
    assert_eq!(values.last().unwrap()["data"]["reason"], "byte_limit");
    registry.record(1000, "reader", "ignored", &MustNotSerialize);
    sink.stop(StopReason::Finished);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn an_oversized_stream_stops_serializing_at_the_record_bound() {
    struct Stream<'a>(&'a AtomicUsize);
    impl Serialize for Stream<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut sequence = serializer.serialize_seq(None)?;
            for _ in 0..1_000_000 {
                self.0.fetch_add(1, Ordering::Relaxed);
                sequence.serialize_element("bounded payload")?;
            }
            sequence.end()
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("oversized.jsonl");
    let registry = Registry::new();
    registry.configure(&path, MAX_TRACE_BYTES).unwrap();
    let attempts = AtomicUsize::new(0);
    registry.record(1, "reader", "oversized", &Stream(&attempts));
    assert!(attempts.load(Ordering::Relaxed) < MAX_RECORD_BYTES);
    let values = records(&path);
    assert_eq!(values.len(), 2);
    assert_eq!(values[1]["data"]["reason"], "record_too_large");
    assert!(std::fs::metadata(&path).unwrap().len() < TERMINAL_RESERVE_BYTES * 2);

    let mut buffer = LineBuffer::new(5);
    assert_eq!(buffer.write(b"12345").unwrap(), 5);
    assert!(buffer.write(b"6").is_err());
    assert_eq!(buffer.bytes, b"12345");
}

#[test]
fn serialization_failure_discards_partial_data_and_private_error_text() {
    struct Failed;
    impl Serialize for Failed {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut sequence = serializer.serialize_seq(None)?;
            sequence.serialize_element("uncommitted value")?;
            Err(S::Error::custom("private failure detail"))
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("failed.jsonl");
    let registry = Registry::new();
    registry.configure(&path, MAX_TRACE_BYTES).unwrap();
    registry.record(1, "reader", "failed", &Failed);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("private failure detail"));
    assert!(!text.contains("uncommitted value"));
    assert_eq!(records(&path).last().unwrap()["data"]["reason"], "serialization_error");
}

#[test]
fn a_serializer_can_emit_a_nested_record_without_holding_the_sink_lock() {
    struct Nested<'a>(&'a Registry);
    impl Serialize for Nested<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.0.record(2, "reader", "nested", &());
            serializer.serialize_bool(true)
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested.jsonl");
    let registry = Registry::new();
    registry.configure(&path, MAX_TRACE_BYTES).unwrap();
    registry.record(1, "reader", "outer", &Nested(&registry));
    registry.sink.get().unwrap().stop(StopReason::Finished);
    let stages = records(&path).into_iter().map(|value| value["stage"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    assert_eq!(stages, ["trace_started", "nested", "outer", "trace_stopped"]);
}

#[derive(Default)]
struct MemoryState {
    bytes: Vec<u8>,
    fail_after: Option<usize>,
    fail_rollback: bool,
    closed: bool,
}

struct MemoryOutput(Arc<Mutex<MemoryState>>);

impl Write for MemoryOutput {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut state = self.0.lock().unwrap();
        if state.fail_after == Some(0) {
            state.fail_after = None;
            return Err(io::Error::other("Injected append failure"));
        }
        let count = state.fail_after.map_or(data.len(), |remaining| data.len().min(remaining));
        state.bytes.extend_from_slice(&data[..count]);
        state.fail_after = state.fail_after.map(|remaining| remaining - count);
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

impl TraceOutput for MemoryOutput {
    fn rollback(&mut self, bytes: u64) -> io::Result<()> {
        let mut state = self.0.lock().unwrap();
        if state.fail_rollback { return Err(io::Error::other("Injected rollback failure")); }
        state.bytes.truncate(bytes as usize);
        Ok(())
    }
}

impl Drop for MemoryOutput {
    fn drop(&mut self) { self.0.lock().unwrap().closed = true; }
}

#[test]
fn a_partial_write_rolls_back_before_terminal_append_or_reports_unknown_integrity() {
    for fail_rollback in [false, true] {
        let memory = Arc::new(Mutex::new(MemoryState::default()));
        let sink = Sink::new(MemoryOutput(memory.clone()), MAX_TRACE_BYTES);
        sink.emit(1, "reader", "complete", &());
        let before = memory.lock().unwrap().bytes.clone();
        {
            let mut state = memory.lock().unwrap();
            state.fail_after = Some(7);
            state.fail_rollback = fail_rollback;
        }
        sink.emit(2, "reader", "partial", &());
        let status = sink.status();
        assert!(!status.enabled);
        assert_eq!(status.stopped_reason, Some(StopReason::IoError));
        assert_eq!(status.output_integrity_unknown, fail_rollback);
        assert_eq!(status.terminal_written, !fail_rollback);
        let bytes = memory.lock().unwrap().bytes.clone();
        assert!(bytes.starts_with(&before));
        assert!(memory.lock().unwrap().closed);
        if !fail_rollback {
            let values = std::str::from_utf8(&bytes).unwrap().lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()).collect::<Vec<_>>();
            assert_eq!(values.len(), 2);
            assert_eq!(values[1]["data"]["reason"], "io_error");
        } else {
            assert_eq!(status.complete_bytes, before.len() as u64);
            assert_eq!(bytes.len(), before.len() + 7);
        }
        sink.emit(3, "reader", "disabled", &MustNotSerialize);
        assert_eq!(memory.lock().unwrap().bytes, bytes);
    }
}

#[test]
fn finish_completes_an_already_started_stop_without_replacing_its_reason() {
    let memory = Arc::new(Mutex::new(MemoryState::default()));
    let sink = Sink::new(MemoryOutput(memory.clone()), MAX_TRACE_BYTES);
    sink.emit(1, "reader", "complete", &());
    // Model an emitter that released the append lock after stopping admission
    // but was preempted before publishing its terminal record.
    sink.lock().stopped_reason = Some(StopReason::ByteLimit);
    sink.active.store(false, Ordering::Release);
    sink.stop(StopReason::Finished);
    let status = sink.status();
    assert_eq!(status.stopped_reason, Some(StopReason::ByteLimit));
    assert!(status.terminal_written);
    assert!(memory.lock().unwrap().closed);
    let bytes = memory.lock().unwrap().bytes.clone();
    sink.write_terminal(StopReason::ByteLimit);
    assert_eq!(memory.lock().unwrap().bytes, bytes);
}

#[test]
fn stamp_cpu_delta_requires_the_same_known_thread_and_nondecreasing_clock() {
    let base = Instant::now();
    let owner = std::thread::current().id();
    let started = Stamp { monotonic_ns: Some(100), thread_cpu_ns: Some(3000),
        thread_id: Some(7), instant: base, owner };
    let ended = Stamp { monotonic_ns: Some(1100), thread_cpu_ns: Some(9000),
        thread_id: Some(7), instant: base + Duration::from_micros(20), owner };
    assert_eq!(ended.elapsed_since(&started), SpanSample { wall_us: 20, cpu_us: Some(6) });
    for changed in [
        Stamp { thread_cpu_ns: None, ..ended },
        Stamp { thread_cpu_ns: Some(2000), ..ended },
        Stamp { thread_id: None, ..ended },
        Stamp { thread_id: Some(8), ..ended },
        Stamp { owner: std::thread::spawn(|| std::thread::current().id()).join().unwrap(), ..ended },
    ] {
        assert_eq!(changed.elapsed_since(&started), SpanSample { wall_us: 20, cpu_us: None });
    }
    assert_eq!(ended.elapsed_since(&Stamp { thread_cpu_ns: None, ..started }).cpu_us, None);
}

#[test]
fn stage_totals_keep_missing_cpu_unknown_and_distinguish_partial_attempts() {
    let mut stage = StageTotals::default();
    assert_eq!(stage.cpu_us, None);
    stage.record(SpanSample { wall_us: 8, cpu_us: Some(0) }, true);
    assert_eq!(stage.cpu_us, Some(0));
    stage.record(SpanSample { wall_us: 12, cpu_us: None }, false);
    stage.record(SpanSample { wall_us: 20, cpu_us: Some(11) }, true);
    assert_eq!(stage, StageTotals { wall_us: 40, cpu_us: None, attempts: 3, completed: 2, partial: 1 });
    let mut overflow = StageTotals::default();
    overflow.record(SpanSample { wall_us: u64::MAX, cpu_us: Some(u64::MAX) }, true);
    overflow.record(SpanSample { wall_us: 1, cpu_us: Some(1) }, false);
    assert_eq!(overflow.wall_us, u64::MAX);
    assert_eq!(overflow.cpu_us, None);
}

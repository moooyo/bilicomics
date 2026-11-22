use std::{
    cell::Cell,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

#[derive(Default)]
pub(crate) struct PreparedHandoff {
    state: Cell<State>,
}

#[derive(Clone, Copy, Default)]
enum State {
    #[default]
    Waiting,
    Invalid,
    Received(Sample),
    Shelf(Sample),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Sample {
    prepared_monotonic_ns: u64,
    received_monotonic_ns: u64,
    receive_source: &'static str,
}

impl PreparedHandoff {
    pub(crate) fn new(path: &Path, shelf_trace: &Path) -> io::Result<Self> {
        if shelf_trace.file_name().and_then(|value| value.to_str()).and_then(|value| value.strip_suffix(".jsonl"))
            != Some(run_id_from_path(path)?)
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Shelf and prepared handoff run identifiers differ"));
        }
        Ok(Self::default())
    }

    pub(crate) fn observe(&self, prepared: Option<u64>, source: &'static str, clock: impl FnOnce() -> Option<u64>) {
        if !matches!(self.state.get(), State::Waiting) { return; }
        self.state.set(State::Invalid);
        let Some(prepared_monotonic_ns) = prepared.filter(|value| *value > 0) else { return; };
        let Some(received_monotonic_ns) = clock().filter(|value| *value >= prepared_monotonic_ns) else { return; };
        let receive_source = match source {
            "maintenance_timer" => "maintenance_timer",
            "runtime_wakeup" => "runtime_wakeup",
            _ => return,
        };
        self.state.set(State::Received(Sample { prepared_monotonic_ns, received_monotonic_ns, receive_source }));
    }

    pub(crate) fn qualify_shelf(&self, rendered_monotonic_ns: Option<u64>) {
        if let State::Received(sample) = self.state.get()
            && rendered_monotonic_ns.is_some_and(|rendered| rendered >= sample.received_monotonic_ns)
        {
            self.state.set(State::Shelf(sample));
        }
    }

    pub(crate) fn write(&self, path: &Path) -> io::Result<()> {
        let State::Shelf(sample) = self.state.get() else { return Ok(()); };
        let run_id = run_id_from_path(path)?;
        let mut boot_id = String::new();
        File::open("/proc/sys/kernel/random/boot_id")?.take(38).read_to_string(&mut boot_id)?;
        let record = sample.json_line(run_id, validated_boot_id(&boot_id)?, std::process::id());
        OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?.write_all(record.as_bytes())
    }
}

impl Sample {
    fn json_line(self, run_id: &str, boot_id: &str, process_id: u32) -> String {
        let Self { prepared_monotonic_ns, received_monotonic_ns, receive_source } = self;
        format!(concat!(
            "{{\"schema\":\"kindle-prepared-handoff-v1\",\"run_id\":\"{}\",\"boot_id\":\"{}\",",
            "\"process_id\":{},\"clock\":\"CLOCK_MONOTONIC\",\"prepared_monotonic_ns\":{},",
            "\"received_monotonic_ns\":{},\"receive_source\":\"{}\"}}\n"),
            run_id, boot_id, process_id, prepared_monotonic_ns, received_monotonic_ns, receive_source)
    }
}

fn run_id_from_path(path: &Path) -> io::Result<&str> {
    path.file_name().and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".prepared-handoff.json"))
        .filter(|stem| stem.strip_prefix("bili-shelf-").is_some_and(|suffix| {
            suffix.len() == 32 && suffix.bytes().all(lower_hex)
        })).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid prepared handoff run identifier"))
}

fn validated_boot_id(value: &str) -> io::Result<&str> {
    let value = value.strip_suffix('\n').unwrap_or(value);
    if value.len() != 36 || !value.bytes().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) { byte == b'-' } else { lower_hex(byte) }
    }) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid prepared handoff boot identifier"));
    }
    Ok(value)
}

fn lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_handoff_keeps_the_first_receive_and_does_not_write_before_shelf() {
        let handoff = PreparedHandoff::default();
        handoff.observe(Some(100), "runtime_wakeup", || Some(380));
        handoff.observe(Some(200), "maintenance_timer", || panic!("duplicate State sampled a clock"));
        assert!(handoff.write(Path::new("not-an-output-path")).is_ok());
        handoff.qualify_shelf(None);
        handoff.qualify_shelf(Some(379));
        assert!(handoff.write(Path::new("not-an-output-path")).is_ok());
        handoff.qualify_shelf(Some(400));
        let State::Shelf(sample) = handoff.state.get() else { panic!("missing qualified sample"); };
        assert_eq!(sample, Sample { prepared_monotonic_ns: 100, received_monotonic_ns: 380,
            receive_source: "runtime_wakeup" });
    }

    #[test]
    fn prepared_handoff_rejects_missing_reversed_or_unrecognized_first_observations() {
        for (producer, consumer, source) in [(None, Some(10), "runtime_wakeup"),
            (Some(0), Some(10), "runtime_wakeup"), (Some(10), None, "runtime_wakeup"),
            (Some(10), Some(9), "maintenance_timer"), (Some(1), Some(10), "unknown")]
        {
            let handoff = PreparedHandoff::default();
            handoff.observe(producer, source, || consumer);
            handoff.observe(Some(20), "maintenance_timer", || panic!("invalid first State was replaced"));
            handoff.qualify_shelf(Some(30));
            assert!(matches!(handoff.state.get(), State::Invalid));
            assert!(handoff.write(Path::new("not-an-output-path")).is_ok());
        }
    }

    #[test]
    fn prepared_handoff_sidecar_contains_only_one_bound_public_record() {
        let run = "bili-shelf-0123456789abcdef0123456789abcdef";
        let boot = "12345678-9abc-4def-8123-456789abcdef";
        let path = Path::new("private-ancestor").join(format!("{run}.prepared-handoff.json"));
        let boot_file = format!("{boot}\n");
        let handoff = PreparedHandoff::new(&path, &Path::new("different-directory").join(format!("{run}.jsonl"))).unwrap();
        handoff.observe(Some(10), "maintenance_timer", || Some(10));
        handoff.qualify_shelf(Some(15));
        let State::Shelf(sample) = handoff.state.get() else { panic!("missing qualified sample"); };
        let record = sample.json_line(run_id_from_path(&path).unwrap(), validated_boot_id(&boot_file).unwrap(), 123);
        assert_eq!(record, concat!(
            "{\"schema\":\"kindle-prepared-handoff-v1\",\"run_id\":\"bili-shelf-0123456789abcdef0123456789abcdef\",",
            "\"boot_id\":\"12345678-9abc-4def-8123-456789abcdef\",\"process_id\":123,\"clock\":\"CLOCK_MONOTONIC\",",
            "\"prepared_monotonic_ns\":10,\"received_monotonic_ns\":10,\"receive_source\":\"maintenance_timer\"}\n"));
        assert!(run_id_from_path(&path.with_extension("jsonl")).is_err());
        assert!(run_id_from_path(Path::new("private.prepared-handoff.json")).is_err());
        assert!(PreparedHandoff::new(&path, Path::new("bili-shelf-ffffffffffffffffffffffffffffffff.jsonl")).is_err());
        assert!(validated_boot_id("arbitrary text").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn prepared_handoff_writes_once_to_a_new_private_sidecar() -> io::Result<()> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let run = "bili-shelf-0123456789abcdef0123456789abcdef";
        let parent = std::env::temp_dir().join(format!("bilicomics-prepared-handoff-test-{}", std::process::id()));
        let sidecar = parent.join(format!("{run}.prepared-handoff.json"));
        std::fs::DirBuilder::new().mode(0o700).create(&parent)?;
        let result = (|| {
            let handoff = PreparedHandoff::new(&sidecar, &parent.join(format!("{run}.jsonl")))?;
            handoff.observe(Some(50), "runtime_wakeup", || Some(300));
            handoff.write(&sidecar)?;
            let existed_before_shelf = sidecar.exists();
            handoff.qualify_shelf(Some(310));
            handoff.write(&sidecar)?;
            let original = std::fs::read(&sidecar)?;
            let mode = std::fs::metadata(&sidecar)?.permissions().mode();
            let second_write = handoff.write(&sidecar);
            let contents_unchanged = std::fs::read(&sidecar)? == original;
            Ok::<_, io::Error>((existed_before_shelf, original, mode, second_write, contents_unchanged))
        })();
        if sidecar.exists() { std::fs::remove_file(&sidecar)?; }
        std::fs::remove_dir(&parent)?;
        let (existed_before_shelf, original, mode, second_write, contents_unchanged) = result?;
        assert!(!existed_before_shelf);
        assert_eq!(original.iter().filter(|&&byte| byte == b'\n').count(), 1);
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(second_write.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert!(contents_unchanged);
        Ok(())
    }
}

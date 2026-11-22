mod parsers;

use anyhow::{ensure, Result};
use parsers::{collect_all, first_twenty};
use std::hint::black_box;
use std::time::Instant;

type Values = (u32, u32, u32, u64);
type Parser = fn(&[u8], u32) -> Result<Values>;
const ITERATIONS: usize = 262_144;

fn current_stat() -> Result<(Vec<u8>, u32)> {
    let process = std::process::id();
    let bytes = std::fs::read("/proc/self/stat")?;
    let identity = collect_all(&bytes, process)?;
    ensure!(identity.0 > 0 && identity.1 > 0 && identity.2 > 0 && identity.3 > 0);
    ensure!(identity == first_twenty(&bytes, process)?);
    Ok((bytes, process))
}

fn timed(parser: Parser, row: &[u8], process: u32) -> u128 {
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        black_box(parser(black_box(row), black_box(process)).unwrap());
    }
    start.elapsed().as_nanos()
}

fn main() -> Result<()> {
    let (row, process) = current_stat()?;
    let tail = std::str::from_utf8(&row)?.rsplit_once(") ").unwrap().1;
    let fields = tail.split_ascii_whitespace().count();
    println!("bytes={} fields={} iterations={}", row.len(), fields, ITERATIONS);
    let arms: [(&str, Parser); 2] = [("A", collect_all), ("B", first_twenty)];
    let mut measured = Vec::with_capacity(24);
    for pair in 0..=12 {
        let sequence = if pair == 0 || pair % 2 == 1 { [0, 1] } else { [1, 0] };
        for index in sequence {
            let (arm, parser) = arms[index];
            let nanoseconds = timed(parser, &row, process);
            if pair > 0 {
                measured.push((pair, arm, nanoseconds));
            }
        }
    }
    for (pair, arm, nanoseconds) in measured {
        println!("{pair} {arm} {nanoseconds}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROCESS: u32 = 42;

    fn valid_fields() -> Vec<String> {
        let mut fields = vec!["unused".to_string(); 20];
        for (index, value) in [(0, "S"), (1, "1"), (2, "2"), (3, "3"), (19, "0040")] {
            fields[index] = value.into();
        }
        fields
    }

    fn row(fields: &[String]) -> Vec<u8> {
        format!("42 (stub) {}\n", fields.join(" ")).into_bytes()
    }

    fn compare(bytes: &[u8]) -> std::result::Result<Values, String> {
        let recorded = collect_all(bytes, PROCESS).map_err(|error| format!("{error:#}"));
        assert_eq!(first_twenty(bytes, PROCESS).map_err(|error| format!("{error:#}")), recorded);
        recorded
    }

    #[test]
    fn real_current_linux_stat_agrees() {
        current_stat().unwrap();
    }

    #[test]
    fn short_records_and_dead_states_still_take_precedence_over_bad_fields() {
        for size in 0..20 {
            let mut fields = valid_fields();
            fields[1] = "invalid".into();
            assert_eq!(compare(&row(&fields[..size])).unwrap_err(), "The native-read process is not alive");
        }
        for state in ["Z", "X", "x"] {
            let mut fields = valid_fields();
            fields[0] = state.into();
            fields[1] = "invalid".into();
            assert_eq!(compare(&row(&fields)).unwrap_err(), "The native-read process is not alive");
        }
    }

    #[test]
    fn pid_group_session_and_birth_use_the_original_numeric_parsers() {
        for index in [1, 2, 3, 19] {
            for value in ["0", "001", "+1", "-1", "x", "4294967295", "4294967296", "18446744073709551615", "18446744073709551616"] {
                let mut fields = valid_fields();
                fields[index] = value.into();
                let _ = compare(&row(&fields));
            }
        }
        let mut fields = valid_fields();
        fields[1] = "0".into();
        fields[2] = "18446744073709551616".into();
        fields[19] = "x".into();
        assert_eq!(compare(&row(&fields)).unwrap_err(), "Invalid launcher process identifier");
    }

    #[test]
    fn comm_delimiters_whitespace_and_unused_fields_keep_stock_behavior() {
        let fields = valid_fields();
        for comm in ["", "has spaces", "x) (inner) (", "with\nline", "unpaired) (and\twhitespace"] {
            let value = format!("42 ({comm}) {}", fields.join("\t \r\n"));
            assert_eq!(compare(value.as_bytes()).unwrap(), (1, 2, 3, 40));
        }
        let mut fields = fields;
        fields.extend(["not-a-number".into(), "xxx".into()]);
        assert_eq!(compare(&row(&fields)).unwrap(), (1, 2, 3, 40));
    }

    #[test]
    fn outer_frame_identity_utf8_and_size_bounds_still_precede_parsing() {
        for (bytes, message) in [(b"42".as_slice(), "Invalid native-read process framing"),
            (b"43 (".as_slice(), "The native-read process identifier changed"),
            (b"42 (".as_slice(), "Invalid native-read process framing")]
        {
            assert_eq!(compare(bytes).unwrap_err(), message);
        }
        let mut bytes = row(&valid_fields());
        bytes.resize(64 * 1024, b' ');
        assert_eq!(compare(&bytes).unwrap(), (1, 2, 3, 40));
        bytes[64 * 1024 - 1] = 0xff;
        assert!(compare(&bytes).unwrap_err().contains("utf-8"));
        bytes.push(0xff);
        assert_eq!(compare(&bytes).unwrap_err(), "The native-read process record is too large");
    }
}

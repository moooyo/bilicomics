use std::{
    env, fs,
    io::{self, Read, Write},
    os::fd::AsRawFd,
    path::Path,
    process::{self, Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Sender},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug)]
enum PlatformError { Operation(String), Io(io::Error) }

impl From<io::Error> for PlatformError {
    fn from(error: io::Error) -> Self { Self::Io(error) }
}

#[cfg(not(feature = "split"))]
include!("stock.rs");
#[cfg(feature = "split")]
include!("split.rs");

#[cfg(feature = "split")]
thread_local! { static POLLS: std::cell::Cell<[(u64, Duration); 2]> = const {
    std::cell::Cell::new([(0, Duration::ZERO); 2])
}; }

#[cfg(feature = "split")]
fn observe_poll(fd: libc::c_int, poll: impl FnOnce() -> libc::c_int) -> libc::c_int {
    let bucket = usize::from(fd == -1);
    let start = Instant::now();
    let result = poll();
    let errno = (result == -1).then(|| unsafe { *libc::__errno_location() });
    let elapsed = start.elapsed();
    POLLS.with(|cell| {
        let mut totals = cell.get();
        totals[bucket].0 += 1;
        totals[bucket].1 += elapsed;
        cell.set(totals);
    });
    if let Some(errno) = errno { unsafe { *libc::__errno_location() = errno }; }
    result
}

fn getter(case: &str) {
    fs::write("getter.pid", process::id().to_string()).unwrap();
    let hold = Duration::from_millis(120);
    if case == "open" { thread::sleep(hold); }
    let mut stdout = io::stdout().lock();
    stdout.write_all(b"Y8\n").unwrap();
    stdout.flush().unwrap();
    drop(stdout);
    if case == "open" { return; }
    assert_eq!(unsafe { libc::close(libc::STDOUT_FILENO) }, 0);
    if case == "withdrawal" { fs::remove_file("fixture-permit").unwrap(); }
    thread::sleep(match case {
        "eof" | "nonzero" => hold,
        "timeout" | "withdrawal" => Duration::from_secs(8),
        _ => panic!("Unknown getter case"),
    });
    process::exit(if case == "nonzero" { 7 } else { 0 });
}

fn main() {
    let mut arguments = env::args().skip(1);
    let case = arguments.next().unwrap();
    if case == "--getter" { return getter(&arguments.next().unwrap()); }
    let executable = env::current_exe().unwrap();
    fs::File::create_new("fixture-permit").unwrap();
    let runner = Commands::new().unwrap();
    let mut checks = 0;
    let start = Instant::now();
    let result = runner.run(&executable, &["--getter", &case], start + CAPTURE_TIMEOUT, || {
        checks += 1;
        if Path::new("fixture-permit").try_exists()? { Ok(()) } else { Err(failure("Fixture permission withdrawn")) }
    });
    let elapsed = start.elapsed();
    let outcome = match (case.as_str(), result) {
        ("open" | "eof", Ok(raw)) if raw == "Y8\n" && record_text(&raw).unwrap() == "Y8" => "parsed_Y8",
        ("nonzero", Err(PlatformError::Operation(reason))) if reason == "Native display getter was rejected" => "rejected_exit",
        ("timeout", Err(PlatformError::Operation(reason))) if reason == "Native display capture timed out" && elapsed >= CAPTURE_TIMEOUT => "deadline",
        ("withdrawal", Err(PlatformError::Operation(reason))) if reason == "Fixture permission withdrawn" && elapsed < CAPTURE_TIMEOUT && checks >= 2 => "withdrawn",
        (_, Err(PlatformError::Io(error))) => panic!("Unexpected I/O failure: {error}"),
        (_, other) => panic!("Unexpected getter result: {other:?}"),
    };
    let pid: u32 = fs::read_to_string("getter.pid").unwrap().parse().unwrap();
    let proc = format!("/proc/{pid}");
    let end = Instant::now() + Duration::from_secs(2);
    while Path::new(&proc).try_exists().unwrap() {
        assert!(Instant::now() < end, "Fixture getter was not reaped");
        thread::sleep(Duration::from_millis(5));
    }
    print!("{{\"outcome\":\"{outcome}\",\"run_ns\":{},\"context_checks\":{checks},\"getter_reaped\":true", elapsed.as_nanos());
    #[cfg(feature = "split")]
    for (label, (calls, wall)) in ["stdout_open", "after_eof"].into_iter().zip(POLLS.with(|cell| cell.get())) {
        print!(",\"{label}\":{{\"calls\":{calls},\"elapsed_ns\":{}}}", wall.as_nanos());
    }
    println!("}}");
}

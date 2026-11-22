#![allow(dead_code)]

extern crate self as slint_backend_kindle;

use std::{env, fs, io, path::PathBuf, time::Duration};

mod native_presentation;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DeviceProfile { Scribe, Paperwhite6 }

impl DeviceProfile {
    fn id(self) -> &'static str {
        match self { Self::Scribe => "scribe", Self::Paperwhite6 => "pw6" }
    }
}

#[derive(Debug)]
pub enum PlatformError { Operation(String), Unsupported(&'static str), UnapprovedDevice, Io(io::Error) }

impl From<io::Error> for PlatformError {
    fn from(error: io::Error) -> Self { Self::Io(error) }
}

pub struct Observed { pub rotation: u32 }

pub fn probe_native_framebuffer(viewport: Option<(u32, u32)>) -> Result<Observed, PlatformError> {
    assert!(matches!(viewport, Some((1860, 2480) | (1264, 1680))));
    #[cfg(feature = "profile")]
    probe::count("metadata");
    Ok(Observed { rotation: 0 })
}

pub fn native_accelerometer_available() -> Result<bool, PlatformError> {
    Ok(env::var("BILICOMICS_APPROVED_PROFILE").unwrap() == "scribe")
}

mod platform {
    use super::*;
    pub mod native_capture;
    pub trait DeviceControl { fn native_viewport(&self) -> Option<(u32, u32)>; }
    include!("launch_approval.rs");
    pub mod native {
        use super::*;
        pub struct NativeControl(DeviceProfile);
        impl NativeControl { pub fn new(profile: DeviceProfile) -> Result<Self, PlatformError> { Ok(Self(profile)) } }
        impl DeviceControl for NativeControl {
            fn native_viewport(&self) -> Option<(u32,u32)> {
                Some(match self.0 { DeviceProfile::Scribe => (1860,2480), DeviceProfile::Paperwhite6 => (1264,1680) })
            }
        }
        pub fn find_command(name: &str) -> Option<PathBuf> {
            assert_eq!(name, "lipc-get-prop");
            Some(PathBuf::from(env::var_os("FIXTURE_GETTER").unwrap()))
        }
    }
    pub fn capture(profile: DeviceProfile) -> Result<String, PlatformError> { native_capture::capture(profile) }
}

#[cfg(feature = "profile")]
mod probe {
    use super::*;
    use std::{cell::RefCell, collections::BTreeMap, io::Write};
    thread_local! { static COUNTS: RefCell<BTreeMap<&'static str, (u64,u64)>> = RefCell::default(); }
    pub fn now() -> u64 {
        let mut value = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        assert_eq!(unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value) }, 0);
        value.tv_sec as u64 * 1_000_000_000 + value.tv_nsec as u64
    }
    fn add(label: &'static str, elapsed: u64) {
        COUNTS.with_borrow_mut(|map| { let entry=map.entry(label).or_default(); entry.0+=1; entry.1+=elapsed; });
    }
    pub fn count(label: &'static str) { add(label, 0); }
    pub struct Span(&'static str,u64);
    pub fn span(label: &'static str) -> Span { Span(label, now()) }
    impl Drop for Span { fn drop(&mut self) { add(self.0,now()-self.1); } }
    pub fn getter(start: u64, end: u64) {
        let mut file=fs::OpenOptions::new().create(true).append(true).open("getter.tsv").unwrap();
        writeln!(file,"{start} {end}").unwrap();
    }
    pub fn finish(start: u64, end: u64) {
        let mut file=fs::OpenOptions::new().create_new(true).write(true).open("capture.tsv").unwrap();
        writeln!(file,"helper {start} {end}").unwrap();
        COUNTS.with_borrow(|rows| for (name,(count,elapsed)) in rows { writeln!(file,"{name} {count} {elapsed}").unwrap(); });
    }
}

fn getter(arguments: &[String]) {
    #[cfg(feature = "profile")]
    let start=probe::now();
    let behavior=env::var("FIXTURE_MODE").unwrap();
    let text=match arguments.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["-s", "com.lab126.powerd", "state"] if behavior=="inactive-first" => "screenSaver",
        ["-s", "com.lab126.powerd", "state"] if behavior=="inactive-last" => {
            if fs::OpenOptions::new().write(true).create_new(true).open("first-power").is_ok() { "active" } else { "screenSaver" }
        }
        ["-s", "com.lab126.powerd", "state"] => "active",
        ["-s", "com.lab126.winmgr", "epdcMode"] => match behavior.as_str() {
            "bad-mode" => "unrecognized",
            "fail-mode" => std::process::exit(3),
            "timeout" | "revoke" => {
                fs::write("getter-pid", std::process::id().to_string()).unwrap();
                if behavior=="revoke" { fs::remove_file(PathBuf::from(env::var_os("BILICOMICS_LIFECYCLE_DIR").unwrap()).join("native-read-go")).unwrap(); }
                std::thread::sleep(Duration::from_secs(6)); "Y8"
            }
            _ => "Y8"
        },
        ["com.lab126.winmgr", "accelerometer"] => "L",
        _ => std::process::exit(2),
    };
    println!("{text}");
    #[cfg(feature = "profile")]
    probe::getter(start, probe::now());
}

fn main() {
    let arguments=env::args().collect::<Vec<_>>();
    let program = std::path::Path::new(&arguments[0]).file_name().unwrap();
    if program == "usleep" {
        let delay=arguments[1].parse().unwrap(); assert!(matches!(delay, 10000 | 20000));
        std::thread::sleep(Duration::from_micros(delay)); return;
    }
    if program == "lipc-get-prop" { getter(&arguments[1..]); return; }
    assert!(arguments.len()==5 && arguments[1..4]==["--capture-native-display", "--kindle", "--device"]);
    let profile=match arguments[4].as_str() { "scribe" => DeviceProfile::Scribe, "pw6" => DeviceProfile::Paperwhite6, _ => panic!("Invalid profile") };
    #[cfg(feature = "profile")]
    let start=probe::now();
    let record=platform::capture(profile);
    #[cfg(feature = "profile")]
    probe::finish(start, probe::now());
    match record { Ok(text) => println!("{text}"), Err(_) => std::process::exit(1) }
}

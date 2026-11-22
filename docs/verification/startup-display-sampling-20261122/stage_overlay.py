#!/usr/bin/env python3
"""Generate a fresh, study-only Rust overlay without writing to the base tree."""

import argparse
from pathlib import Path


def exact(source, old, new, count=1):
    if source.count(old) != count:
        raise ValueError(f"expected {count} exact source anchor(s): {old.splitlines()[0]}")
    return source.replace(old, new)


def capture_source(source):
    source = exact(source, "const CAPTURE_TIMEOUT:", """macro_rules! cost {
    ($bucket:ident, $value:expr) => {{
        #[cfg(all(feature = "kindle", target_os = "linux"))]
        let result = crate::platform::capture_cost::observe(crate::platform::capture_cost::Bucket::$bucket, || $value);
        #[cfg(not(all(feature = "kindle", target_os = "linux")))]
        let result = $value;
        result
    }};
}

const CAPTURE_TIMEOUT:""")
    source = exact(source, """    let remaining = checkpoint(source)?;
    let raw = source.query(query, remaining)?;
    checkpoint(source)?;""", """    let remaining = cost!(QueryOuterPre, checkpoint(source))?;
    let raw = source.query(query, remaining)?;
    cost!(QueryOuterPost, checkpoint(source))?;""")
    source = exact(source, """fn read_metadata(source: &mut impl CaptureSource) -> Result<PortraitMetadata, PlatformError> {
    let remaining = checkpoint(source)?;
    let metadata = source.metadata(remaining)?;
    checkpoint(source)?;""", """fn read_metadata(source: &mut impl CaptureSource, bucket: bool) -> Result<PortraitMetadata, PlatformError> {
    let remaining = cost!(MetadataOuterPre, checkpoint(source))?;
    let metadata = if bucket { cost!(Metadata1, source.metadata(remaining))? }
        else { cost!(Metadata2, source.metadata(remaining))? };
    cost!(MetadataOuterPost, checkpoint(source))?;""")
    source = exact(source, "let first = read_metadata(source)?;", "let first = read_metadata(source, true)?;")
    source = exact(source, "if read_metadata(source)? != first {", "if read_metadata(source, false)? != first {")
    source = exact(source, """        let started = Instant::now();
        validate_native_launch_approval(profile)?;
        let permit = NativeReadPermit::acquire().map_err(context_error)?;
        permit.recheck().map_err(context_error)?;""", """        let started = Instant::now();
        let sampling = crate::platform::capture_cost::Sampling::begin();
        cost!(LaunchApproval, validate_native_launch_approval(profile))?;
        let permit = cost!(PermitAcquire, NativeReadPermit::acquire()).map_err(context_error)?;
        cost!(PermitInitial, permit.recheck()).map_err(context_error)?;
        let sampling = sampling.and_then(|sampling| {
            let identity = permit.startup_cost_identity(sampling.run_id())?;
            Some((sampling, identity))
        });""")
    source = exact(source, """        capture_with(profile, &mut source)
    }

    struct NativeSource""", """        let result = capture_with(profile, &mut source);
        if let Some((sampling, identity)) = sampling && result.as_ref().is_ok_and(|value| value.starts_with("ready:")) {
            sampling.finish(identity);
        }
        result
    }

    struct NativeSource""")
    source = exact(source, """            self.commands.run(&self.lipc, arguments, deadline, || {
                self.permit.recheck().map_err(context_error)
            })""", """            cost!(CommandsRun, self.commands.run(&self.lipc, arguments, deadline, || {
                self.permit.recheck().map_err(context_error)
            }))""")
    source = exact(source, "            checkpoint(self)?;", "            cost!(MetadataCheckpoint, checkpoint(self))?;", 3)
    source = exact(source, """            ensure_deadline(deadline)?;
            check_context()?;
            ensure_deadline(deadline)?;""", """            ensure_deadline(deadline)?;
            cost!(QueryInnerPre, check_context())?;
            ensure_deadline(deadline)?;""")
    source = exact(source, """            let child = Command::new(program)
                .args(arguments)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?;""", """            let child = cost!(CommandSpawn, Command::new(program)
                .args(arguments)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn())?;""")
    source = exact(source, "match stdout.read(&mut buffer[..allowance + 1]) {",
                   "match cost!(CommandRead, stdout.read(&mut buffer[..allowance + 1])) {")
    source = exact(source, "if let Some(status) = owned.try_wait()? {",
                   "if let Some(status) = cost!(CommandTryWait, owned.try_wait())? {")
    source = exact(source, """                if closed && exited {
                    check_context()?;""", """                if closed && exited {
                    cost!(QueryInnerPost, check_context())?;""")
    source = exact(source, """                let result = unsafe {
                    libc::poll(
                        &mut descriptor,
                        1,
                        remaining.min(POLL_INTERVAL).as_millis().max(1) as libc::c_int,
                    )
                };""", """                let result = cost!(CommandPoll, unsafe {
                    libc::poll(
                        &mut descriptor,
                        1,
                        remaining.min(POLL_INTERVAL).as_millis().max(1) as libc::c_int,
                    )
                });""")
    return exact(source, """        if Instant::now() >= *next_check {
            check_context()?;""", """        if Instant::now() >= *next_check {
            cost!(QueryInnerPeriodic, check_context())?;""")


def writer_source(source):
    source = exact(source, """    pub(crate) fn recheck(&self) -> Result<()> {
        ensure!(
            self.writer.ownership_for(OwnershipPurpose::Capture)? == self.ownership,
            "Native-read ownership changed during capture"
        );
        Ok(())
    }""", """    pub(crate) fn recheck(&self) -> Result<()> {
        ensure!(
            self.writer.ownership_for(OwnershipPurpose::Capture)? == self.ownership,
            "Native-read ownership changed during capture"
        );
        Ok(())
    }

    #[cfg(feature = "kindle")]
    pub(crate) fn startup_cost_identity(&self, run_id: &str) -> Option<crate::platform::capture_cost::Identity> {
        use crate::platform::capture_cost::Identity;
        let sources = &self.ownership.sources;
        let find = |key: &str| sources.iter().find(|(name, _)| *name == key).map(|(_, value)| value);
        if find("application-pid")?.is_some() || find("lifecycle-request")?.is_some() {
            return None;
        }
        let [supervisor, leader, chain @ ..] = self.ownership.processes.as_slice() else { return None; };
        let helper = chain.iter().find(|process| process.pid == self.writer.helper)?;
        let home = Path::new("/mnt/us").join(run_id);
        if std::env::var_os("BILICOMICS_HOME").as_deref() != Some(home.as_os_str())
            || helper.executable != home.join("app/bilicomics")
        {
            return None;
        }
        let sequence = find("native-read-go")?.as_deref()?.split(':').nth(1)?.parse().ok()?;
        Some(Identity {
            supervisor_pid: supervisor.pid, supervisor_birth: supervisor.birth,
            native_read_pid: leader.pid, native_read_birth: leader.birth, native_read_sequence: sequence,
            helper_pid: helper.pid, helper_birth: helper.birth,
        })
    }""")
    return exact(source, """    fn process(&self, pid: u32) -> Result<ProcessIdentity> {
        use std::os::unix::fs::OpenOptionsExt;""", """    fn process(&self, pid: u32) -> Result<ProcessIdentity> {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(feature = "kindle")]
        crate::platform::capture_cost::process_stat_read();""")


def generate(base, output):
    base = base.resolve(strict=True)
    output = output.resolve()
    if output.is_relative_to(base):
        raise ValueError("the overlay must be outside its base tree")
    platform_path = Path("src/platform/mod.rs")
    capture_path = Path("src/platform/native_capture.rs")
    writer_path = Path("src/native_presentation/portrait_writer.rs")
    new_module = Path("src/platform/capture_cost.rs")
    if (base / new_module).exists():
        raise ValueError("the base already contains the study module")
    staged = {
        platform_path: exact((base / platform_path).read_text(encoding="utf-8"), "mod native_capture;",
                             "mod native_capture;\n#[cfg(all(feature = \"kindle\", target_os = \"linux\"))]\npub(crate) mod capture_cost;"),
        capture_path: capture_source((base / capture_path).read_text(encoding="utf-8")),
        writer_path: writer_source((base / writer_path).read_text(encoding="utf-8")),
        new_module: Path(__file__).with_name("capture_cost.rs").read_text(encoding="utf-8"),
    }
    output.mkdir(mode=0o700, parents=False)
    for path, content in staged.items():
        target = output / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8", newline="\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    try:
        generate(args.base, args.output)
    except (OSError, ValueError) as error:
        parser.exit(1, f"overlay refused: {error}\n")


if __name__ == "__main__":
    main()

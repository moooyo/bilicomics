#!/usr/bin/env python3
"""Generate a private startup display-poll overlay from unmodified product source."""

import argparse
import importlib.util
from pathlib import Path


PREVIOUS = Path(__file__).resolve().parent.parent / "startup-display-sampling-20261122"
SPEC = importlib.util.spec_from_file_location("previous_display_overlay", PREVIOUS / "stage_overlay.py")
PREVIOUS_OVERLAY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PREVIOUS_OVERLAY)
exact = PREVIOUS_OVERLAY.exact

POLL_SITE = """                let result = cost!(CommandPoll, unsafe {
                    libc::poll(
                        &mut descriptor,
                        1,
                        remaining.min(POLL_INTERVAL).as_millis().max(1) as libc::c_int,
                    )
                });"""
PARTITIONED_SITE = POLL_SITE.replace(
    "cost!(CommandPoll, unsafe {", "crate::platform::capture_cost::observe_poll(descriptor.fd, || unsafe {"
)


def capture_source(source):
    return exact(PREVIOUS_OVERLAY.capture_source(source), POLL_SITE, PARTITIONED_SITE)


def recorder_source(source):
    source = exact(source, "CommandTryWait, CommandPoll,", "CommandTryWait, CommandPoll, CommandPollStdoutOpen, CommandPollAfterEof,")
    source = exact(source, "Bucket::CommandPoll as usize + 1", "Bucket::CommandPollAfterEof as usize + 1")
    source = exact(source, '    "command_try_wait", "command_poll",',
                   '    "command_try_wait", "command_poll", "command_poll_stdout_open", "command_poll_after_eof",')
    source = exact(source, "BILICOMICS_DISPLAY_COST_RUN", "BILICOMICS_DISPLAY_POLL_RUN")
    source = exact(source, "kindle-startup-display-cost-v1", "kindle-startup-display-poll-v1")
    source = exact(source, ".display-cost.json", ".display-poll.json", 2)
    source = exact(source,
                   "            observe_with(Bucket::CommandPoll, || {}, step);",
                   "            observe_poll_with(7, || {}, step);")
    source = exact(source, """fn observe_with<T>(bucket: Bucket, work: impl FnOnce() -> T, clock: impl Fn() -> Option<u64>) -> T {
    if !ACTIVE.with(|active| active.borrow().is_some()) { return work(); }""",
                   """pub(crate) fn observe_poll(fd: libc::c_int, work: impl FnOnce() -> libc::c_int) -> libc::c_int {
    observe_poll_with(fd, work, monotonic_ns)
}

fn observe_poll_with<T>(fd: libc::c_int, work: impl FnOnce() -> T, clock: impl Fn() -> Option<u64>) -> T {
    let state = if fd == -1 { Bucket::CommandPollAfterEof } else { Bucket::CommandPollStdoutOpen };
    observe_partition_with(Bucket::CommandPoll, Some(state), work, clock)
}

fn observe_with<T>(bucket: Bucket, work: impl FnOnce() -> T, clock: impl Fn() -> Option<u64>) -> T {
    observe_partition_with(bucket, None, work, clock)
}

fn observe_partition_with<T>(bucket: Bucket, child: Option<Bucket>, work: impl FnOnce() -> T,
    clock: impl Fn() -> Option<u64>) -> T
{
    if !ACTIVE.with(|active| active.borrow().is_some()) { return work(); }""")
    source = exact(source, "            totals.add(bucket, started, finished);", """            totals.add(bucket, started, finished);
            if let Some(child) = child { totals.add(child, started, finished); }""")
    return source + "\n" + Path(__file__).with_name("poll_partition_tests.inc").read_text(encoding="utf-8")


def generate(base, output):
    base = base.resolve(strict=True)
    output = output.resolve()
    if output.is_relative_to(base):
        raise ValueError("the overlay must be outside its base tree")
    platform_path = Path("src/platform/mod.rs")
    capture_path = Path("src/platform/native_capture.rs")
    writer_path = Path("src/native_presentation/portrait_writer.rs")
    recorder_path = Path("src/platform/capture_cost.rs")
    if (base / recorder_path).exists():
        raise ValueError("the base already contains a study recorder")
    staged = {
        platform_path: exact((base / platform_path).read_text(encoding="utf-8"), "mod native_capture;",
                             'mod native_capture;\n#[cfg(all(feature = "kindle", target_os = "linux"))]\npub(crate) mod capture_cost;'),
        capture_path: capture_source((base / capture_path).read_text(encoding="utf-8")),
        writer_path: PREVIOUS_OVERLAY.writer_source((base / writer_path).read_text(encoding="utf-8")),
        recorder_path: recorder_source((PREVIOUS / "capture_cost.rs").read_text(encoding="utf-8")),
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
        parser.exit(1, f"poll overlay refused: {error}\n")


if __name__ == "__main__":
    main()

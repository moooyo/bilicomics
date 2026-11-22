"""Run one isolated Linux qualification of the stock getter's two poll states."""

import argparse
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import tomllib


CASES = ("open", "eof", "nonzero", "timeout", "withdrawal")
POLL = """                let result = unsafe {
                    libc::poll(
                        &mut descriptor,
                        1,
                        remaining.min(POLL_INTERVAL).as_millis().max(1) as libc::c_int,
                    )
                };"""


def require(condition, message):
    if not condition:
        raise ValueError(message)


def section(text, start, stop):
    require(text.count(start) == text.count(stop) == 1 and text.index(start) < text.index(stop),
            f"Ambiguous source boundary: {start}")
    return text[text.index(start):text.index(stop)]


def prepare(source, work):
    text = (source / "src/platform/native_capture.rs").read_text()
    names = ("CAPTURE_TIMEOUT", "RECORD_LIMIT", "POLL_INTERVAL", "CONTEXT_INTERVAL")
    parts = []
    for name in names:
        line, = (line.strip() for line in text.splitlines() if line.lstrip().startswith(f"const {name}:"))
        parts.append(line)
    parts += [section(text, "fn failure(", "fn checkpoint("), section(text, "fn record_text(", "fn read_query("),
              section(text, "    struct Commands {", "\n    #[cfg(test)]\n    mod tests {")]
    stock = "\n\n".join(parts)
    require(stock.count(POLL) == stock.count("libc::poll(") == 1, "Unexpected stock poll site")
    split = POLL.replace("= unsafe {", "= observe_poll(descriptor.fd, || unsafe {").removesuffix("};") + "});"
    src = work / "src"
    src.mkdir()
    (src / "stock.rs").write_text(stock)
    (src / "split.rs").write_text(stock.replace(POLL, split))
    shutil.copyfile(Path(__file__).with_name("fixture.rs"), src / "main.rs")
    packages = tomllib.loads((source / "Cargo.lock").read_text())["package"]
    version, = (package["version"] for package in packages if package["name"] == "libc")
    (work / "Cargo.toml").write_text('[package]\nname="poll-state-fixture"\nversion="0.0.0"\nedition="2024"\n'
                                    f'[features]\nsplit=[]\n[dependencies]\nlibc="={version}"\n')
    return version


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("source", type=Path, help="Stock source root with Cargo.lock and src/platform/native_capture.rs")
    cli.add_argument("output", type=Path, help="New JSON receipt outside the temporary build")
    args = cli.parse_args()
    require(sys.platform == "linux" and {name for _, name in socket.if_nameindex()} == {"lo"}
            and not any(Path(node).exists() for node in ("/dev/fb0", "/dev/input")), "Use the isolated test-env userspace")
    require(not args.output.exists(), "Use a fresh output file")
    os.umask(0o077)
    rows = {}
    with tempfile.TemporaryDirectory(dir="/tmp", prefix="bili-poll-state-") as temporary:
        work = Path(temporary)
        version = prepare(args.source, work)
        target = work / "target"
        env = {**os.environ, "CARGO_TARGET_DIR": str(target), "CARGO_INCREMENTAL": "0"}
        for variant, flags in (("stock", []), ("split", ["--features", "split"])):
            subprocess.run(["cargo", "build", "--offline", "-j1", "--release", *flags], cwd=work, env=env,
                           timeout=120, check=True)
            for case in CASES:
                trial = work / f"{variant}-{case}"
                trial.mkdir()
                reply = subprocess.run([target / "release/poll-state-fixture", case], cwd=trial,
                                       capture_output=True, text=True, check=True, timeout=12)
                row = json.loads(reply.stdout)
                rows.setdefault(case, {})[variant] = row
                if variant == "split":
                    require(row["outcome"] == rows[case]["stock"]["outcome"], f"Changed outcome for {case}")
                    opened, eof = row["stdout_open"], row["after_eof"]
                    require(0 < opened["elapsed_ns"] + eof["elapsed_ns"] <= row["run_ns"], "Invalid poll wall total")
                    dominant, other = (opened, eof) if case == "open" else (eof, opened)
                    require(all(dominant[key] > other[key] for key in ("calls", "elapsed_ns")),
                            f"Controlled wait was not distinguished: {case}")
    report = {"schema": "bili-host-poll-state-v1", "physical_device": False, "libc": version,
              "clock": "Linux monotonic elapsed time (Rust Instant)", "permission": "synthetic callback",
              "cases": rows}
    with args.output.open("x") as output:
        json.dump(report, output, indent=2)
        output.write("\n")


if __name__ == "__main__":
    main()

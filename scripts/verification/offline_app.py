#!/usr/bin/env python3
"""Read recovered synthetic pages in the real non-demo App across a cold restart."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import sys
import time


def require(value, message):
    if not value:
        raise RuntimeError(message)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def interrupt(signum, frame):
    raise KeyboardInterrupt("Offline App verification was interrupted")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--expected-binary-sha256", required=True)
    parser.add_argument("--binary-source-manifest", type=Path, required=True)
    parser.add_argument("--harness-root", type=Path, required=True)
    parser.add_argument("--fixture-data", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    require(sys.platform == "linux", "Run on the designated remote Linux host")
    signal.signal(signal.SIGTERM, interrupt)
    signal.signal(signal.SIGHUP, interrupt)
    require({name for _, name in socket.if_nameindex()} == {"lo"},
            "Run under unshare --net with no external network interface")
    require(digest(args.binary) == args.expected_binary_sha256, "The selected application changed")
    args.output.mkdir(parents=True, exist_ok=False)
    data = args.output / "data"
    shutil.copytree(args.fixture_data, data)
    sys.path.insert(0, str(args.harness_root.resolve(strict=True)))
    from reader_v2_runtime import Pointer, trace_records
    from PIL import Image

    def mapped_window(process):
        tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True, timeout=5)
        matches = []
        for candidate in re.finditer(r'(0x[0-9a-fA-F]+)\s+"BiliComics"', tree):
            identifier = candidate.group(1)
            owner = subprocess.check_output(["xprop", "-id", identifier, "_NET_WM_PID"], text=True, timeout=5)
            pid = re.search(r"=\s*(\d+)", owner)
            if pid is None or int(pid.group(1)) != process.pid:
                continue
            details = subprocess.check_output(["xwininfo", "-id", identifier], text=True, timeout=5)
            if "Map State: IsViewable" not in details:
                continue
            fields = {key: int(re.search(pattern, details).group(1)) for key, pattern in {
                "x": r"Absolute upper-left X:\s*(-?\d+)", "y": r"Absolute upper-left Y:\s*(-?\d+)",
                "width": r"Width:\s*(\d+)", "height": r"Height:\s*(\d+)",
            }.items()}
            matches.append({"id": identifier, **fields})
        require(len(matches) <= 1, "Multiple mapped application windows belong to the selected process")
        return matches[0] if matches else None

    report = {"passed": False, "physical_device": False, "live_account": False,
              "demo": False, "offline": True, "network_interfaces": ["lo"],
              "input": "One real XTest page turn, followed by a separate application process",
              "binary_sha256": digest(args.binary),
              "binary_source_manifest_sha256": digest(args.binary_source_manifest),
              "harness_sha256": digest(Path(__file__)),
              "pointer_helper_sha256": digest(args.harness_root / "reader_v2_runtime.py"),
              "cases": []}

    def body_hash(path):
        with Image.open(path) as image:
            return hashlib.sha256(image.convert("RGB").crop((60, 170, 870, 1080)).tobytes()).hexdigest()

    def identity(process):
        root = Path("/proc") / str(process.pid)
        stat = (root / "stat").read_text()
        executable = (root / "exe").resolve(strict=True)
        require(digest(executable) == args.expected_binary_sha256, "The live process has another executable")
        return {"pid": process.pid, "start_ticks": stat[stat.rfind(")") + 2:].split()[19],
                "executable": str(executable), "sha256": digest(executable)}

    def run_case(name, expected_page, turn):
        output = args.output / name
        output.mkdir()
        trace = output / "lifecycle.jsonl"
        capture = output / "final.png"
        command = [str(args.binary), "--device", "scribe", "--offline", "--data-dir", str(data),
                   "--lifecycle-trace", str(trace), "--capture", str(capture), "--exit-after-ms", "6500"]
        environment = os.environ.copy()
        for key in ("BILICOMICS_LIFECYCLE_DIR", "BILICOMICS_LAUNCH_TOKEN", "BILICOMICS_LAUNCH_GENERATION"):
            environment.pop(key, None)
        log = (output / "application.log").open("w")
        process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT,
                                   env=environment, start_new_session=True)
        pointer = None
        result = {"name": name, "command": command, "passed": False}
        report["cases"].append(result)

        def ready(page):
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                require(process.poll() is None, "The application exited before the expected Reader frame")
                for record in reversed(trace_records(trace)):
                    state = record.get("reader_state") or {}
                    if record.get("process_id") == process.pid and record.get("route") == "reader" \
                            and record.get("reader_loading") is False \
                            and state.get("comic_id") == "900001" and state.get("episode_id") == "900002" \
                            and state.get("page_index") == page and state.get("frame_width", 0) > 0 \
                            and state.get("frame_height", 0) > 0 and not state.get("transition"):
                        return record
                time.sleep(0.05)
            raise RuntimeError("The recovered offline Reader did not reach the expected source page")

        try:
            pointer = Pointer()
            initial = ready(expected_page)
            result["process_identity"] = baseline = identity(process)
            deadline = time.monotonic() + 5
            window = mapped_window(process)
            while window is None and time.monotonic() < deadline:
                time.sleep(0.05)
                window = mapped_window(process)
            require(window and (window["width"], window["height"]) == (930, 1240), "The Scribe window is absent or has another size")
            owner = subprocess.check_output(["xprop", "-id", window["id"], "_NET_WM_PID"], text=True, timeout=5)
            require(int(re.search(r"=\s*(\d+)", owner).group(1)) == process.pid, "The captured X11 window belongs to another process")
            time.sleep(0.15)
            result["initial_reader_state"] = initial["reader_state"]
            result["initial_capture"] = pointer.capture(window, output / "initial.png")
            if turn:
                pointer.click(window, (790, 620))
                final = ready(expected_page + 1)
                require(identity(process) == baseline, "The application process changed during the page turn")
                require(final["reader_state"]["revision"] == initial["reader_state"]["revision"], "The source revision changed")
                time.sleep(0.15)
                result["turned_capture"] = pointer.capture(window, output / "turned.png")
                require(body_hash(output / "initial.png") != body_hash(output / "turned.png"), "The comic body did not change after XTest input")
            else:
                final = initial
            result["final_reader_state"] = final["reader_state"]
            result["exit_code"] = process.wait(timeout=15)
            require(result["exit_code"] == 0, "The bounded application did not exit normally")
            metadata = json.loads(capture.with_suffix(".json").read_text())
            require(metadata["process_id"] == process.pid and metadata["demo"] is False
                    and metadata["offline"] is True and metadata["route"] == "reader"
                    and metadata["reader"]["loading"] is False
                    and metadata["reader"]["page"] == expected_page + (2 if turn else 1),
                    "The actual final application capture disagrees with the Reader state")
            result["capture_metadata"] = metadata
            result["body_sha256"] = body_hash(capture)
            result["passed"] = True
            return result
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
            if pointer is not None:
                pointer.close()
            log.close()

    try:
        first = run_case("initial-and-turn", 1, True)
        second = run_case("cold-restart", 2, False)
        require(first["process_identity"]["pid"] != second["process_identity"]["pid"], "The restart reused a process")
        require(first["body_sha256"] == second["body_sha256"], "The cold restart restored different comic pixels")
        for key in ("comic_id", "episode_id", "revision", "page_id", "page_index", "mode", "fit", "direction"):
            require(first["final_reader_state"][key] == second["final_reader_state"][key], "The cold restart changed the durable Reader identity")
        require(digest(args.binary) == args.expected_binary_sha256, "The selected application changed during verification")
        report["passed"] = True
    except Exception as error:
        report["error"] = str(error)
    finally:
        (args.output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": report["passed"], "result": str(args.output / "result.json")}), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())

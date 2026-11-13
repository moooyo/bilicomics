#!/usr/bin/env python3
"""Exercise persisted dithering choices in the real offline desktop reader.

Run on the designated remote worker inside a private Xvfb display, or use the
explicitly authorized --local mode on this machine's Linux/WSL runtime. Local
mode requires BILICOMICS_LOCAL_VERIFICATION=1 and does not verify a native
Windows executable. The application receives XTest input and its own empty
network namespace. A design profile never enables native hardware: these cases
verify software fallback, not an EPDC submission or physical gray-level acceptance.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import subprocess
import sys
import traceback

import reader_guest_fixtures as guest


PROFILES = {"pw5": (618, 824), "scribe": (930, 1240)}
ANCHOR_KEYS = (
    "comic_id", "episode_id", "revision", "page_id", "page_index", "crop_index",
    "source_x", "source_y", "zoom_ratio", "rotation", "core_mode", "fit", "direction",
)
CONTEXT_KEYS = (
    "BILICOMICS_LIFECYCLE_DIR", "BILICOMICS_LAUNCH_TOKEN", "BILICOMICS_LAUNCH_GENERATION",
)


def require(value, message):
    if not value:
        raise AssertionError(message)


def interrupt(signum, frame):
    raise KeyboardInterrupt("Dithering verification was interrupted")


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def anchor(state):
    require(all(key in state for key in ANCHOR_KEYS), "The live reader omits an anchor invariant")
    return {key: state[key] for key in ANCHOR_KEYS}


def seed_data(output, assets):
    """Extend the existing fictional guest seed before any App process starts."""
    data, database, proof = guest.seed_data(output, "hardware-y4", assets, 18)
    manifest = json.loads((assets / "manifest.json").read_text(encoding="utf-8"))
    second = next(value for value in manifest["pages"] if value["number"] == 19)
    source = (assets / second["filename"]).resolve(strict=True)
    require(source.is_relative_to(assets), "The second fictional page escaped its asset root")
    content = source.read_bytes()
    width, height = guest.png_dimensions(content)
    require([width, height] == [value * second["scale_factor"] for value in second["logical_size"]],
            "The second PNG dimensions differ from the frozen design manifest")
    sha = hashlib.sha256(content).hexdigest()
    filename = sha + ".png"
    pages = Path(proof["cached_path"]).parent
    (pages / filename).write_bytes(content)
    receipt = {
        "episode_id": guest.EPISODE, "revision": guest.REVISION, "page_id": sha, "page_index": 1,
        "file_name": filename, "sha256": sha, "bytes": len(content), "width": width,
        "height": height, "retained": True, "verified_at": 0,
    }
    with guest.sql(database) as connection:
        row = connection.execute("SELECT payload FROM image_indices WHERE account_key=? AND episode_id=?",
                                 (guest.ACCOUNT, guest.EPISODE)).fetchone()
        index = json.loads(row[0])
        index["images"].append({"id": sha, "index": 1, "width": width, "height": height})
        connection.execute("UPDATE image_indices SET payload=? WHERE account_key=? AND episode_id=?",
                           (json.dumps(index), guest.ACCOUNT, guest.EPISODE))
        connection.execute("INSERT INTO page_receipts VALUES(?,?,?,?,?,?)",
                           (guest.ACCOUNT, guest.EPISODE, guest.REVISION, sha, 1, json.dumps(receipt)))
        connection.execute("INSERT INTO page_identity_history VALUES(?,?,?,?,?,?)",
                           (guest.ACCOUNT, guest.EPISODE, guest.REVISION, sha, sha, len(content)))
        row = connection.execute("SELECT payload FROM comic_details WHERE account_key=? AND comic_id=?",
                                 (guest.ACCOUNT, guest.COMIC)).fetchone()
        detail = json.loads(row[0])
        detail["episodes"][0]["extra"]["page_count"] = 2
        connection.execute("UPDATE comic_details SET payload=? WHERE account_key=? AND comic_id=?",
                           (json.dumps(detail), guest.ACCOUNT, guest.COMIC))
        preferences = guest.read_preferences(database)
        preferences.update({"dither": True, "dither_method": "software", "launch": "resume"})
        connection.execute("UPDATE preferences SET payload=? WHERE account_key=? AND key='preferences'",
                           (json.dumps(preferences), guest.ACCOUNT))
    proof["synthetic_page_count"] = 2
    proof["second_page"] = {"origin_path": str(source), "sha256": sha, "bytes": len(content),
                            "dimensions": [width, height], "receipt": receipt}
    proof["prelaunch_preferences"] = preferences
    proof["modifications_after_app_launch"] = False
    guest.write_json(output / "seed.json", proof)
    guest.check_isolation(data, database)
    return data, database, proof


def persisted_preferences(database):
    with guest.sql(database) as connection:
        rows = connection.execute(
            "SELECT key,payload FROM preferences WHERE account_key=? AND key IN (?,?)",
            (guest.ACCOUNT, "preferences", "reader_preferences_" + guest.COMIC),
        ).fetchall()
    return {key: json.loads(value) for key, value in rows}


def find_dithering_row(path, profile):
    """Locate the final choice border in actual pixels, including note reflow."""
    from PIL import Image

    with Image.open(path) as source:
        image = source.convert("L")
    width, height = image.size
    edge, label, row_height = (48, 96, 60) if profile == "scribe" else (24, 76, 52)
    left, right = edge + label + 12, width - edge
    dark_rows = []
    for y in range(height // 3, height - 8):
        values = list(image.crop((left + 4, y, right - 4, y + 1)).getdata())
        if sum(value < 80 for value in values) >= len(values) * 0.96:
            dark_rows.append(y)
    groups = []
    for y in dark_rows:
        if not groups or y > groups[-1][-1] + 1:
            groups.append([y])
        else:
            groups[-1].append(y)
    borders = [group for group in groups if len(group) <= 4]
    candidates = [(top, bottom) for top, bottom in zip(borders, borders[1:])
                  if abs((bottom[0] - top[0]) - (row_height - 2)) <= 3]
    require(candidates, "The actual display sheet has no final choice row of the expected height")
    top, bottom = candidates[-1]
    y = (top[-1] + bottom[0]) // 2
    require(image.getpixel((left, y)) < 80 and image.getpixel((right - 1, y)) < 80,
            "The detected dithering row lacks its expected vertical borders")
    return {"left": left, "right": right, "top": top[0], "bottom": bottom[-1], "y": y,
            "source": "Actual X11 pixels; final full-width choice border"}


class DesktopRun:
    def __init__(self, args, x11, profile, data, output):
        self.args, self.x11, self.profile = args, x11, profile
        self.data, self.output = data, output
        self.trace, self.capture = output / "lifecycle.jsonl", output / "final.png"
        self.process, self.log, self.window, self.state = None, None, None, None
        self.result = {"passed": False, "observations": [], "clicks": [], "screenshots": []}

    def start(self):
        self.output.mkdir(mode=0o700)
        self.log = (self.output / "application.log").open("w", encoding="utf-8")
        environment = os.environ.copy()
        for key in CONTEXT_KEYS:
            environment.pop(key, None)
        environment["SLINT_SCALE_FACTOR"] = "1"
        command = ["unshare", "--net", "--", str(self.args.binary), "--offline", "--device", self.profile,
                   "--screen", "reader", "--data-dir", str(self.data), "--lifecycle-trace", str(self.trace),
                   "--capture", str(self.capture), "--exit-after-ms", "180000"]
        self.process = subprocess.Popen(command, env=environment, stdout=self.log, stderr=subprocess.STDOUT,
                                        start_new_session=True)
        self.result.update({"command": command, "process_id": self.process.pid})
        self.observe("initial completed reader frame", {}, timeout=90)
        self.window = guest.wait_for("the PID-owned reader window", lambda: self.x11.window(self.process),
                                     process=self.process)
        require((self.window["width"], self.window["height"]) == PROFILES[self.profile],
                "The actual reader window differs from its design geometry")
        process_root = Path("/proc") / str(self.process.pid)
        require(digest(process_root / "exe") == self.args.expected_binary_sha256,
                "The live process is running a different binary")
        stat = (process_root / "stat").read_text()
        namespace = os.readlink(process_root / "ns/net")
        require(namespace != os.readlink("/proc/self/ns/net"), "The App shares the harness network namespace")
        interfaces = [line.split(":", 1)[0].strip() for line in (process_root / "net/dev").read_text().splitlines()
                      if ":" in line]
        require(interfaces == ["lo"], "The App has an external network interface")
        self.result["identity"] = {"pid": self.process.pid,
                                   "start_ticks": stat[stat.rfind(")") + 2:].split()[19],
                                   "binary_sha256": self.args.expected_binary_sha256,
                                   "network_namespace": namespace, "network_interfaces": interfaces}
        return self.state

    def observe(self, description, expected, after=0, timeout=30):
        def find():
            for record in reversed(guest.trace_records(self.trace)[after:]):
                state = record.get("reader_state") or {}
                if (record.get("event") == "reader-ready" and record.get("process_id") == self.process.pid
                        and record.get("route") == "reader" and record.get("reader_loading") is False
                        and state.get("rendering") is False and state.get("frame_width", 0) > 0
                        and state.get("frame_height", 0) > 0
                        and state.get("comic_id") == guest.COMIC and state.get("episode_id") == guest.EPISODE
                        and all(state.get(key) == value for key, value in expected.items())):
                    return record
            return None
        record = guest.wait_for(description, find, process=self.process, timeout=timeout)
        self.state = record["reader_state"]
        self.result["observations"].append({"description": description, "expected": expected, "record": record})
        return self.state

    def click(self, name, point):
        require(0 <= point[0] < self.window["width"] and 0 <= point[1] < self.window["height"],
                "A planned click escaped the actual client area")
        after = len(guest.trace_records(self.trace))
        self.x11.click(self.window, *point)
        self.result["clicks"].append({"name": name, "client": list(point), "after_record": after,
                                      "window_id": self.window["id"], "process_id": self.process.pid})
        return after

    def screenshot(self, name):
        value = self.x11.stable_capture(self.window, self.output / (name + ".png"), self.process)
        value["name"] = name
        self.result["screenshots"].append(value)
        return value

    def open_display(self):
        width, height = PROFILES[self.profile]
        after = self.click("open-reading-menu", (width // 2, height // 2))
        self.observe("reading menu opened", {"menu": True, "sheet": ""}, after)
        after = self.click("open-display-settings", (width // 2, height - (54 if self.profile == "scribe" else 44)))
        self.observe("display settings opened", {"menu": True, "sheet": "display"}, after)
        return self.screenshot("display-before-selection")

    def select(self, value, database):
        before_anchor = anchor(self.state)
        image = self.screenshot("display-before-" + value)
        row = find_dithering_row(Path(image["path"]), self.profile)
        index = {"software": 0, "hardware": 1, "off": 2}[value]
        point = (round(row["left"] + 2 + (index + 0.5) * (row["right"] - row["left"] - 4) / 3), row["y"])
        after = self.click("select-dithering-" + value, point)
        expected = {"dither": value != "off", "hardware_y4_available": False,
                    "hardware_dither": False, "hardware_y4_requested": False,
                    "menu": True, "sheet": "display"}
        if value != "off":
            expected["dither_method"] = value
        self.observe("completed " + value + " dithering selection", expected, after)
        require(anchor(self.state) == before_anchor, "A dithering selection moved the reading anchor")

        def persisted():
            values = persisted_preferences(database)
            keys = ("preferences", "reader_preferences_" + guest.COMIC)
            return values if all(key in values and values[key].get("dither") is (value != "off")
                                 and (value == "off" or values[key].get("dither_method") == value)
                                 for key in keys) else None
        preferences = guest.wait_for("global and per-comic dithering persistence", persisted, process=self.process)
        capture = self.screenshot("display-selected-" + value)
        self.result.setdefault("selections", []).append({"value": value, "row": row, "anchor": before_anchor,
                                                          "persisted_preferences": preferences, "capture": capture})
        return self.state

    def close_display(self):
        width, height = PROFILES[self.profile]
        before_anchor = anchor(self.state)
        after = self.click("close-display-settings", (width // 2, height // 5))
        self.observe("quiet reader after closing settings", {"menu": False, "sheet": ""}, after)
        require(anchor(self.state) == before_anchor, "Closing display settings moved the reading anchor")

    def stop(self):
        self.process.send_signal(signal.SIGTERM)
        require(self.process.wait(timeout=20) == 0, "The actual App did not exit cleanly")
        self.log.close()
        metadata = json.loads(self.capture.with_suffix(".json").read_text(encoding="utf-8"))
        require(metadata.get("process_id") == self.process.pid and metadata.get("demo") is False
                and metadata.get("offline") is True and metadata.get("backend") == "desktop-software"
                and metadata.get("profile") == self.profile and metadata.get("route") == "reader"
                and metadata.get("network_prompt") is False,
                "The final capture does not describe the actual offline desktop reader")
        require(metadata.get("reader", {}).get("pages") == 2
                and metadata["reader"].get("page") == self.state["page_number"]
                and metadata["reader"].get("menu") is False and metadata["reader"].get("sheet") == "",
                "The final capture differs from the observed quiet two-page reader")
        self.result.update({"passed": True, "final_state": self.state, "capture_metadata": metadata})
        guest.write_json(self.output / "result.json", self.result)
        return self.state

    def cleanup(self):
        if self.process is not None and self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGTERM)
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait(timeout=5)
        if self.log is not None:
            self.log.close()
        if self.output.exists():
            guest.write_json(self.output / "result.json", self.result)


def profile_case(args, x11, profile, output):
    output.mkdir(mode=0o700)
    data, database, proof = seed_data(output, args.source_root / "assets/demo")
    result = {"profile": profile, "passed": False, "seed": proof, "runs": []}
    first = DesktopRun(args, x11, profile, data, output / "software-to-hardware")
    try:
        first.start()
        first.observe("default software output", {"dither": True, "dither_method": "software",
                      "hardware_y4_available": False, "hardware_dither": False, "hardware_y4_requested": False})
        initial_anchor = anchor(first.state)
        software = first.screenshot("quiet-software")
        first.open_display()
        first.select("hardware", database)
        first.close_display()
        fallback = first.screenshot("quiet-hardware-fallback")
        require(fallback["rgb_sha256"] == software["rgb_sha256"],
                "Desktop hardware fallback changed the actual software-dithered comic pixels")
        after = first.click("continue-to-next-source-page", (round(PROFILES[profile][0] * 0.85), PROFILES[profile][1] // 2))
        first.observe("continued reading with a hardware preference", {"page_index": 1, "dither_method": "hardware",
                      "hardware_dither": False, "hardware_y4_requested": False}, after)
        require(first.state["page_id"] == proof["second_page"]["sha256"], "The page turn did not reach the second fixture page")
        second_anchor = anchor(first.state)
        second_pixels = first.screenshot("continued-hardware-fallback")
        require(second_pixels["rgb_sha256"] != software["rgb_sha256"], "The actual page image did not change after a page turn")
        first.stop()
        result["runs"].append(first.result)
    finally:
        first.cleanup()
    second = DesktopRun(args, x11, profile, data, output / "hardware-restart-to-off")
    try:
        second.start()
        second.observe("persisted hardware choice on cold restart", {"page_index": 1, "dither": True,
                       "dither_method": "hardware", "hardware_y4_available": False,
                       "hardware_dither": False, "hardware_y4_requested": False})
        require(anchor(second.state) == second_anchor, "Cold restart lost the continued reading anchor")
        restored = second.screenshot("restored-hardware-fallback")
        require(restored["rgb_sha256"] == second_pixels["rgb_sha256"], "Cold restart changed the actual retained comic pixels")
        second.open_display()
        second.select("off", database)
        second.close_display()
        second.screenshot("quiet-off")
        second.stop()
        result["runs"].append(second.result)
    finally:
        second.cleanup()
    third = DesktopRun(args, x11, profile, data, output / "off-restart-to-software")
    try:
        third.start()
        third.observe("persisted off choice on cold restart", {"page_index": 1, "dither": False,
                      "hardware_y4_available": False, "hardware_dither": False, "hardware_y4_requested": False})
        require(anchor(third.state) == second_anchor, "The off preference changed the restored anchor")
        third.open_display()
        third.select("software", database)
        third.close_display()
        software_again = third.screenshot("quiet-software-restored")
        require(software_again["rgb_sha256"] == second_pixels["rgb_sha256"],
                "Returning to software dithering did not restore the expected actual comic pixels")
        third.stop()
        result["runs"].append(third.result)
    finally:
        third.cleanup()
    guest.check_isolation(data, database)
    require(len({run["identity"]["pid"] for run in result["runs"]}) == 3, "A cold restart reused the prior process identity")
    result.update({"passed": True, "initial_anchor": initial_anchor, "continued_anchor": second_anchor,
                   "software_and_hardware_fallback_pixels_match": True, "restart_anchor_preserved": True,
                   "visual_review_required": [capture["path"] for run in result["runs"]
                                              for capture in run["screenshots"] if capture["name"].startswith("display-selected-")]})
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--expected-binary-sha256", required=True)
    parser.add_argument("--binary-source-manifest", type=Path, required=True)
    parser.add_argument("--harness-root", type=Path, required=True)
    parser.add_argument("--workspace", type=Path, help="Defaults to the parent of the frozen application source")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--profiles", nargs="+", choices=tuple(PROFILES), default=list(PROFILES))
    parser.add_argument("--local", action="store_true",
                        help="Run on the local Linux/WSL runtime with BILICOMICS_LOCAL_VERIFICATION=1; not native Windows verification")
    args = parser.parse_args()
    if args.local:
        require(sys.platform == "linux" and os.environ.get("BILICOMICS_LOCAL_VERIFICATION") == "1",
                "Local Linux/WSL execution requires --local and BILICOMICS_LOCAL_VERIFICATION=1")
        require(os.environ.get("BILICOMICS_REMOTE_VERIFICATION") != "1",
                "Local execution must not be marked as remote verification")
    else:
        require(sys.platform == "linux" and os.environ.get("BILICOMICS_REMOTE_VERIFICATION") == "1",
                "Run only on the designated remote worker with BILICOMICS_REMOTE_VERIFICATION=1")
    signal.signal(signal.SIGTERM, interrupt)
    signal.signal(signal.SIGHUP, interrupt)
    os.umask(0o077)
    args.binary = args.binary.resolve(strict=True)
    args.harness_root = args.harness_root.resolve(strict=True)
    require(args.harness_root == Path(guest.__file__).resolve().parent,
            "The imported guest helper differs from the selected frozen harness root")
    args.source_root = args.harness_root.parents[1]
    args.workspace = (args.workspace or args.source_root.parent).resolve(strict=True)
    args.output = args.output.resolve()
    args.expected_binary_sha256 = args.expected_binary_sha256.lower()
    require(args.output.is_relative_to(args.workspace) and args.output != args.workspace,
            "Evidence must use a new directory inside this run's isolated workspace")
    require(digest(args.binary) == args.expected_binary_sha256, "The selected binary differs from its frozen digest")
    require(args.binary_source_manifest.is_file(), "The selected binary has no source manifest")
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    kernel_release = platform.release()
    kernel_version = platform.version()
    is_wsl = any(marker in (kernel_release + " " + kernel_version).casefold() for marker in ("microsoft", "wsl"))
    report = {"passed": False, "class": "fixture", "demo": False, "offline": True,
              "execution_environment": ("local-wsl" if is_wsl else "local-linux") if args.local else "test-env",
              "local_verification": args.local, "remote_verification": not args.local,
              "runtime_metadata": {"sys_platform": sys.platform, "system": platform.system(),
                                   "machine": platform.machine(), "kernel_release": kernel_release,
                                   "is_wsl": is_wsl, "native_windows": False},
              "physical_device": False, "live_account": False, "native_driver_exercised": False,
              "hardware_acceptance": False, "binary_sha256": args.expected_binary_sha256,
              "binary_source_manifest_sha256": digest(args.binary_source_manifest),
              "harness_sha256": digest(Path(__file__)),
              "guest_helper_sha256": digest(Path(guest.__file__)), "cases": [],
              "input": "XTest clicks on the PID-owned real application window; no named actions or hardware mocks",
              "scope": "Desktop preference persistence, software fallback, unchanged anchors, real continued reading and restart",
              "manual_review_scope": "Readable selected options and fallback note; no native 256-level capability claim"}
    notes = json.loads((args.source_root / "assets/i18n/reader.json").read_text(encoding="utf-8"))
    dithering_copy = {key: value for key, value in notes.items() if key.startswith("read_dither")}
    require(dithering_copy and all("256" not in value for value in dithering_copy.values()),
            "Dithering interface copy unexpectedly contains a 256-level claim")
    report["localized_dithering_copy_has_no_256_claim"] = True
    x11 = None
    try:
        x11 = guest.X11()
        for profile in dict.fromkeys(args.profiles):
            directory = args.output / profile
            try:
                result = profile_case(args, x11, profile, directory)
            except Exception as error:
                result = {"profile": profile, "passed": False, "error": str(error), "traceback": traceback.format_exc()}
            report["cases"].append(result)
            guest.write_json(directory / "result.json", result)
            print(json.dumps({"profile": profile, "passed": result["passed"], "error": result.get("error")}), flush=True)
        require(digest(args.binary) == args.expected_binary_sha256, "The selected binary changed during verification")
        report["passed"] = all(case["passed"] for case in report["cases"])
    finally:
        if x11 is not None:
            x11.close()
        guest.write_json(args.output / "results.json", report)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())

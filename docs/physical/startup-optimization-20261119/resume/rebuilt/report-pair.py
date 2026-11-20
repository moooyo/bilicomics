"""Analyze a closed independent batch using the original click-to-Shelf timing definition."""

import argparse
import json
from pathlib import Path
from statistics import median

from PIL import Image

from project_archived import bounds, observed_times, one, require


def load(path):
    return json.loads(path.read_text())


def lines(path):
    return [json.loads(line) for line in path.read_text().splitlines()]


def ms(duration):
    return round(duration / 1_000_000, 3)


def fingerprint(rgb):
    value = 0xcbf29ce484222325
    for channel in rgb:
        value = ((value ^ channel) * 0x100000001b3) & 0xffffffffffffffff
    return f"{value:016x}"


def summary(values):
    return {"median_ms": ms(median(values)), "min_ms": ms(min(values)), "max_ms": ms(max(values))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("batch", type=Path)
    args = parser.parse_args()
    root = args.batch.resolve(strict=True)
    admission, released = load(root / "admission.json"), load(root / "released.json")
    require(not admission["dry_only"] and not (root / "failure.json").exists() and released["ok"] and
            released["state"] == "released" and released["owner_thread"] == admission["owner"] and
            released["run_id"] == root.name == admission["run"] and (root / "uninstalled").is_file(), "Not a closed successful batch")
    original, restored = load(root / "original-navigation.json"), load(root / "native-restored.json")
    require(original["pointer"] == restored["pointer"] and restored["buttons_clear"], "Pointer not restored")
    order = [arm + f"{pair:02}" for pair in range(7) for arm in ("ab" if pair == 0 or pair % 2 else "ba")]
    require(order == admission["schedule"], "Wrong paired schedule")
    cases, processes = {}, set()
    reference, reference_fingerprint = None, None
    for name in order:
        case = root / name
        final, click, target = load(case / "final.json"), load(case / "click.json"), load(case / "target.json")
        require(target["title"]["count"] == target["library"]["count"] == 1 and target["library"]["underline_pct"] >= 60
                and target["home"]["underline_pct"] <= 20, "Click target was not the selected native Library")
        require(click["target"] == "open-card" and click["clock"] == "CLOCK_MONOTONIC" and click["mode"] == "XTest-button-1" and
                click["down_confirmed"] and click["up_sent"] and click["buttons_clear_after"] and click["hold_requested_ms"] == 50 and
                0 < click["down_before_ns"] <= click["down_after_ns"] <= click["up_before_ns"] - 50_000_000 < click["up_after_ns"] and
                click["up_before_ns"] <= click["up_after_ns"], "Invalid input bracket")
        life, raw = lines(case / "lifecycle.jsonl"), lines(case / "performance.jsonl")
        pid = final["process_id"]
        require(pid not in processes and all(record["process_id"] == pid for record in raw + life) and
                (case / "entry.status").read_text().strip() == "0" and (case / "guard-done").is_file(), "Case process or normal exit differs")
        processes.add(pid)
        records = [{"source_line_1based": index, "fields": record} for index, record in enumerate(raw, 1)]
        private_render = admission["protocol"] == "shelf-render-click-3" and name[0] == "b"
        marker, observed, _ = observed_times(final, life, records, click["up_before_ns"], private_shelf_render=private_render)
        send = observed["request_to_shelf_send"]
        done = observed["request_to_shelf_driver_complete"]
        ready = click["up_before_ns"] + observed["request_to_ui_ready"]
        frames = [row for row in raw if (row["kind"], row["stage"]) == ("native", "frame") and
                  row["data"]["rendered"] and row["data"]["refresh_accepted"] and
                  bounds(row["data"]["stages"]["refresh_submit"])[1] <= click["up_before_ns"] + send]
        require(frames and frames[-1]["data"]["accepted_update_marker"] == marker and
                frames[0]["data"]["rgb_fingerprint"] == "fe0d23ce1e398933", "Wrong first loading scene or Shelf frame")
        with Image.open(case / "final.png") as image:
            require(image.size == (1860, 2480), "Wrong logical Shelf dimensions")
            rgb = image.convert("RGB").tobytes()
        if reference is None:
            reference = rgb
            reference_fingerprint = fingerprint(reference)
        require(reference == rgb and frames[-1]["data"]["rgb_fingerprint"] == reference_fingerprint,
                "The first rendered Shelf does not match the common final Shelf pixels")
        route = one((row for row in raw if (row["kind"], row["stage"]) == ("startup", "attach-route")
                     and row["data"].get("phase") == "end"), "route end")["monotonic_ns"]
        views = one((row for row in raw if (row["kind"], row["stage"]) == ("startup", "attach-views")
                     and row["data"].get("phase") == "begin"), "views begin")["monotonic_ns"]
        require(route <= views <= ready, "Out-of-order Shelf attachment")
        duplicates = [current for previous, current in zip(frames, frames[1:]) if
                      current["data"]["rgb_fingerprint"] == previous["data"]["rgb_fingerprint"] and
                      current["data"]["complete_frame_replacement"] and current["data"]["refresh_flashing"] and
                      current["data"]["refresh_waveform"] == "gc16" and route <= bounds(current["data"]["stages"]["slint_render"])[0] and
                      current["monotonic_ns"] <= views]
        cases[name] = {"send_ns": send, "driver_ns": done, "process_entry_ns": observed["request_to_process_enter"],
                       "first_loading_send_ns": observed["request_to_app_splash_send"], "shelf_ready_ns": observed["request_to_ui_ready"],
                       "marker": marker, "loading_frames": len(frames) - 1,
                       "route_gap_ns": views - route, "duplicate_gap_frames": len(duplicates),
                       "release_bracket_ns": click["up_after_ns"] - click["up_before_ns"]}
    frame_pixels = []
    for name in ("b06", "a06"):
        frame = load(root / name / "frame-info.json")
        require(frame["read_only"] and frame["bits_per_pixel"] == 8 and frame["width"] == 1860 and frame["height"] == 2480
                and frame["visible_bytes"] == 4612800, "Invalid physical framebuffer geometry")
        frame_pixels.append((root / name / "frame-pixels.txt").read_text().strip())
    require(frame_pixels[0] == frame_pixels[1] and len(frame_pixels[0]) == 64, "Visible Gray8 pixels differ between arms")
    timings = {}
    for label in ("send", "driver"):
        values = {arm: [cases[arm + f"{pair:02}"][label + "_ns"] for pair in range(1, 7)] for arm in "ab"}
        differences = [candidate - control for candidate, control in zip(values["b"], values["a"], strict=True)]
        timings[label] = {"A": summary(values["a"]), "B": summary(values["b"]), "B_minus_A": summary(differences),
                          "paired_differences_ms": list(map(ms, differences)), "B_faster_pairs": sum(value < 0 for value in differences)}
    result = {"run": admission["run"], "source": admission["source"], "schedule": order, "warmups_excluded": ["a00", "b00"],
              "cases": cases, "timings": timings, "common_shelf_rgb_fingerprint": reference_fingerprint,
              "common_visible_gray8_sha256": frame_pixels[0], "visible_matches_previous_packed_frame": frame_pixels[0] ==
              "12ff9def5d77c72f473a4703635a192e9c79efaa71420605a5145bbc436dacc3",
              "restored_native_tab_and_pointer": True, "installation_removed": True, "lease_released": True}
    with (root / "paired-result.json").open("x") as output:
        json.dump(result, output, indent=2)
        output.write("\n")
    print(json.dumps(timings, indent=2))


if __name__ == "__main__":
    main()

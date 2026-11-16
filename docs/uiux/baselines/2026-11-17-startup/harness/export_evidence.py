#!/usr/bin/env python3
"""Copy an explicit allowlist of saved startup evidence without running the app."""

import argparse
import datetime
import hashlib
import json
from pathlib import Path
import shutil


RESULT_SHA256 = "d326d46304a29d985f5794858ae6f82ebe72781ef234774229d9a8e47bbc05d0"
RUNNER_SHA256 = "172f6c80ed9e7e2df1a78da663e90471913a503fa612a2a9b098c1e5146e50c8"
PROFILES = {"pw6": [1264, 1680], "scribe": [1860, 2480]}
VARIANTS = ("baseline", "candidate")
SCENARIOS = ("normal", "failure")


def require(value, message):
    if not value:
        raise AssertionError(message)


def sha256(path):
    result = hashlib.sha256()
    with Path(path).open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def read_json(path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8", newline="\n") as output:
        json.dump(value, output, ensure_ascii=False, indent=2)
        output.write("\n")


def main(arguments):
    base = arguments.base.resolve(strict=True)
    source = base / "desktop-r2"
    result_path = source / "results.json"
    require(sha256(result_path) == RESULT_SHA256, "The selected saved desktop result changed")
    result = read_json(result_path)
    require(result["passed"] is True and len(result["cases"]) == 8
            and len(result["comparisons"]) == 10, "The selected comparison did not pass")
    require(result["harness_sha256"] == RUNNER_SHA256
            and result["physical_device_used"] is False
            and result["live_account_used"] is False, "The selected evidence has another scope")
    output = arguments.output.resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    files, captures = [], []

    def copy(original, relative, role, expected=None):
        original = original.resolve(strict=True)
        require(original.is_file(), "The selected evidence is not a regular file")
        relative = Path(relative)
        require(not relative.is_absolute() and ".." not in relative.parts,
                "Unsafe evidence destination")
        require(not any(part in {"data", "demo"} for part in relative.parts),
                "Application data is outside the evidence allowlist")
        payload_digest = sha256(original)
        if expected is not None:
            require(payload_digest == expected, "A selected original differs from its stored receipt")
        target = output / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        require(not target.exists(), "Refusing to overwrite an exported original")
        shutil.copyfile(original, target)
        require(sha256(target) == payload_digest, "Evidence copying changed the original bytes")
        files.append({"path": relative.as_posix(), "bytes": target.stat().st_size,
                      "sha256": payload_digest, "role": role, "original": str(original),
                      "byte_preserved": True})
        return relative.as_posix()

    def project(relative, value, role):
        relative = Path(relative)
        target = output / relative
        write_json(target, value)
        files.append({"path": relative.as_posix(), "bytes": target.stat().st_size,
                      "sha256": sha256(target), "role": role,
                      "original": None, "byte_preserved": False,
                      "derivation": "Metadata projection only; original receipts are retained separately"})
        return relative.as_posix()

    copy(result_path, "evidence/desktop-r2-results.json", "Original successful aggregate result", RESULT_SHA256)
    copy(base / "tools/desktop-inputs-r1.json", "evidence/source/selected-inputs.json",
         "Original selected software identities", result["selection_sha256"])
    for variant in VARIANTS:
        selected = result["inputs"][variant]
        copy(Path(selected["source_manifest"]), "evidence/source/" + variant + "-source-inputs.sha256",
             "Original production source inventory", selected["source_manifest_sha256"])
    copy(base / "tools/desktop_verify-r2.py", "harness/desktop_verify-r2.py", "Original successful runner", RUNNER_SHA256)
    copy(base / "tools/desktop_verify-r1.py", "harness/desktop_verify-r1.py", "Original failed runner")
    copy(Path(result["capture_dependency"]["path"]), "harness/pointer_v2.py",
         "Original X11 capture dependency", result["capture_dependency"]["sha256"])
    copy(base / "desktop-r1/failure.json", "evidence/desktop-r1/failure.json", "Original runner failure receipt")
    copy(base / "reports/desktop-r1.log", "evidence/desktop-r1/run.log", "Original failed runner output")
    copy(arguments.readme, "README.md", "Evidence narrative; human visual review pending")
    copy(Path(__file__), "harness/export_evidence.py", "Artifact-only allowlist exporter")

    expected_cases = {"-".join((variant, profile, scenario))
                      for profile in PROFILES for variant in VARIANTS for scenario in SCENARIOS}
    require({case["case_id"] for case in result["cases"]} == expected_cases,
            "The selected case set differs from the accepted comparison")
    for case in result["cases"]:
        case_id = case["case_id"]
        profile, variant, scenario = case["profile"], case["variant"], case["scenario"]
        require(case_id == "-".join((variant, profile, scenario)), "A case identity is inconsistent")
        directory = source / case_id
        raw = Path("raw") / profile / variant / scenario
        case_record = Path("evidence/cases") / case_id / "result.json"
        require(read_json(directory / "result.json")
                == {key: value for key, value in case.items() if key != "outer_cleanup"},
                "A saved case result differs from the accepted aggregate")
        require(read_json(directory / "outer-cleanup.json") == case["outer_cleanup"],
                "A saved cleanup receipt differs from the accepted aggregate")
        process_receipt = read_json(directory / "outer-process.json")
        require(process_receipt["pid"] == process_receipt["group"] == case["outer_cleanup"]["group"]
                and process_receipt["observations"][-1]["marker_seen"] is True,
                "The saved outer process identity differs from its accepted cleanup group")
        copy(directory / "result.json", case_record, "Original application case result")
        copy(directory / "outer-process.json", Path("evidence/cases") / case_id / "outer-process.json",
             "Original outer process identity")
        copy(directory / "outer-cleanup.json", Path("evidence/cases") / case_id / "outer-cleanup.json",
             "Original private process group closure")
        for name in ("startup-0.png", "startup-1.png", "final.png"):
            image = case["images"][name]
            require(image["size"] == PROFILES[profile], "The stored capture geometry changed")
            exported = copy(directory / name, raw / name, "Original Rust/Slint main PNG", image["sha256"])
            sidecar = Path(name).with_suffix(".json")
            expected_metadata = case[{"startup-0.png": "first_frame", "startup-1.png": "activity_frame",
                                      "final.png": "final_state"}[name]]
            require(read_json(directory / sidecar) == expected_metadata,
                    "An original application sidecar differs from the accepted aggregate")
            metadata = copy(directory / sidecar, raw / sidecar, "Original Rust/Slint JSON sidecar")
            captures.append({"id": case_id + "/" + Path(name).stem, "profile": profile,
                             "software_variant": variant, "scenario": scenario, "file": exported,
                             "metadata": metadata, "metadata_kind": "Original application JSON sidecar",
                             "size": image["size"], "scale_factor": 2, "sha256": image["sha256"],
                             "source": image["source"], "case_record": case_record.as_posix(),
                             "source_commit": case["source_commit"], "binary_sha256": case["binary_sha256"],
                             "physical_device_used": False, "live_account_used": False,
                             "visual_review": "pending"})
        if scenario == "normal":
            image = case["images"]["final-x11.png"]
            exported = copy(directory / "final-x11.png", raw / "final-x11.png",
                            "Original stable actual X11 main PNG", image["sha256"])
            metadata = project(raw / "final-x11.json", {
                "record_kind": "Exported metadata projection; not an original application sidecar",
                "case_id": case_id, "profile": profile, "software_variant": variant,
                "source_commit": case["source_commit"], "binary_sha256": case["binary_sha256"],
                "harness_sha256": RUNNER_SHA256, "original_case_result": case_record.as_posix(),
                "original_case_result_sha256": sha256(directory / "result.json"),
                "image": image, "stability": case["stable_x11"],
                "pixel_processing": "None; the original PNG was copied without modification",
                "physical_device_used": False, "live_account_used": False, "visual_review": "pending",
            }, "Projected metadata for the original stable X11 PNG")
            captures.append({"id": case_id + "/final-x11", "profile": profile,
                             "software_variant": variant, "scenario": scenario, "file": exported,
                             "metadata": metadata, "metadata_kind": "Explicit export projection",
                             "size": image["size"], "scale_factor": 2, "sha256": image["sha256"],
                             "source": image["source"], "case_record": case_record.as_posix(),
                             "source_commit": case["source_commit"], "binary_sha256": case["binary_sha256"],
                             "physical_device_used": False, "live_account_used": False,
                             "visual_review": "pending"})
            samples = case["stable_x11"]["samples"]
            require([sample["file"] for sample in samples]
                    == ["settle/000.png", "settle/001.png", "settle/002.png"],
                    "The accepted settling sample set differs from the explicit allowlist")
            for sample in samples:
                copy(directory / sample["file"], raw / sample["file"], "Original X11 settling PNG", sample["sha256"])

    require(len(captures) == 28 and len(list(output.rglob("*.png"))) == 40,
            "The exported main and settling PNG counts differ from the accepted evidence")
    project("capture-index.json", {"schema_version": 1,
            "scope": "Original source-bound desktop startup comparison; no physical device",
            "automated_checks": "passed", "visual_review": "pending", "main_capture_count": 28,
            "settling_capture_count": 12, "captures": captures}, "Original capture locations and identities")
    manifest = {"schema_version": 1, "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                "scope": "Allowlist export of saved evidence only; application and tests were not run",
                "source_result_sha256": RESULT_SHA256, "runner_sha256": RUNNER_SHA256,
                "main_png_count": 28, "settling_png_count": 12,
                "physical_device_used": False, "live_account_used": False, "visual_review": "pending",
                "excluded": ["Application data", "Generated demo assets", "SQLite databases",
                             "Private case configuration and ownership nonces", "Executable and source archives"],
                "files": sorted(files, key=lambda record: record["path"])}
    write_json(output / "export-manifest.json", manifest)
    for path in output.rglob("*"):
        if path.is_file() and path.suffix == ".png":
            path.chmod(0o444)
    print(json.dumps({"output": str(output), "files": len(files) + 1, "main_pngs": 28,
                      "settling_pngs": 12, "export_manifest_sha256": sha256(output / "export-manifest.json"),
                      "application_or_tests_executed": False}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", type=Path, required=True)
    parser.add_argument("--readme", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    main(parser.parse_args())

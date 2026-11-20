#!/usr/bin/env python3
"""Read the independently recovered URI failure: one first launch, zero primary samples.

Usage: python3 -B project_partial.py ORIGINAL_FAILED_BATCH CLOSED_RECOVERY
Only named files in the closed archives are read; nothing is extracted to disk.
"""

import argparse
import hashlib
import io
import json
from pathlib import Path
import tarfile

from project_fresh import CASES, DOC, RELEASED, RESTORED, Sources, all_true, case_row, require


class MemberSources(Sources):
    def __init__(self, tar, members, name):
        self.tar, self.members, self.name, self.hashes = tar, members, name, {}

    def exists(self, name):
        return f"out/{self.name}/{name}" in self.members

    def read(self, name, limit=1_048_576):
        item = self.members[f"out/{self.name}/{name}"]
        require(item.isfile() and 0 < item.size <= limit, f"Unexpected archive member: {name}")
        with self.tar.extractfile(item) as source:
            data = source.read(limit + 1)
        require(len(data) == item.size, "Incomplete archive member")
        self.hashes[name] = hashlib.sha256(data).hexdigest()
        return data


def project(batch_path, recovery_path):
    original, recovered = Sources(batch_path), Sources(recovery_path)
    report, admission = original.json("report.json"), original.json("admission.json")
    receipt = recovered.json("recovery-receipt.json")
    release = recovered.json("evidence/exact-lease-release.json")
    run = report["run_id"]
    require(run == "lib-t-1119-a599027a" and report.get("finished_at_utc") and report["route"] == "uri"
            and report["success"] is False and report["lease_released"] is False and report["cases"] == CASES
            and report["native_uri"] == "file://" + DOC and report["completed_cases"] == [CASES[0]],
            "Not the closed first-launch-only failed URI batch")
    require((admission["run_id"], admission["route"], admission["first_case"], admission["warm_repetitions"], admission["plugin_sha256"])
            == (run, "uri", CASES[0], CASES[1:], {}), "Wrong original admission")
    arm = report["variants"]["candidate"]
    require(arm["startup_arguments"] == ["--demo", "--offline", "--screen", "shelf"]
            and arm["entry_arguments"] == ["--library"] and arm["binary_sha256"] ==
            "04fc6f93b4327911ca1a3bb5200175c9f2e13fb21ac5e56280012ff44b6e2e59", "Unexpected software identity")
    require(receipt["run_id"] == release["run_id"] == run and receipt["original_report_sha256"] == original.hashes["report.json"]
            and release["operation"] == "release" and release["state"] == "released", "Recovery not bound to original")
    all_true(receipt, ("success", "source_batch_failed", "other_five_nonlaunches_and_completed_first_proven", *RESTORED))
    all_true(release, RELEASED)
    raw = recovered.read("outputs.tar.gz", 16_777_216)
    require(receipt["outputs_sha256"] == recovered.hashes["outputs.tar.gz"], "Collected archive hash differs")
    with tarfile.open(fileobj=io.BytesIO(raw), mode="r:gz") as tar:
        items = tar.getmembers()
        members = {item.name: item for item in items}
        require(len(items) == len(members) <= 1024, "Ambiguous archive")
        for name in CASES[1:]:
            src = MemberSources(tar, members, name)
            require(not any(src.exists(f) for f in ("request.json", "application.pid", "performance.jsonl", "final.json")),
                    "Unexpected evidence of another launch")
        row, _, _, _ = case_row(MemberSources(tar, members, CASES[0]), report, CASES[0])
    return {"schema_version": 1, "run": run, "status": "failed_batch_with_separately_recovered_first_only",
            "source_date": "2026-11-19", "route": "uri", "original_report_success": False,
            "original_report_lease_released": False, "independent_restoration_and_exact_release": True,
            "original_source_sha256": original.hashes, "recovery_source_sha256": recovered.hashes,
            "runtime_commit": arm["build_configuration"]["runtime_source_commit"], "package_commit": arm["source_commit"],
            "application_sha256": arm["binary_sha256"], "package_sha256": arm["package_sha256"],
            "canonical_uri": report["native_uri"], "first_completed": 1, "planned_primary": 5, "completed_primary": 0,
            "distribution_available": False, "first_in_batch_only": row,
            "scope": "Single retained fresh-process offline demo Shelf start on Scribe, no reboot or cache eviction. Candidate-01 stopped before request; remaining primary cases did not launch. No distribution or optimization estimate from this failure.",
            "endpoints": "Kindle CLOCK_MONOTONIC before native file-URI dispatch, not a click. Initial app SEND, unique software ready, first accepted full Shelf SEND after Ready, end of checked driver wait for the same marker. No optical, complete-cover or input-ready endpoint."}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("batch", type=Path)
    parser.add_argument("recovery", type=Path)
    args = parser.parse_args()
    print(json.dumps(project(args.batch, args.recovery), indent=2))

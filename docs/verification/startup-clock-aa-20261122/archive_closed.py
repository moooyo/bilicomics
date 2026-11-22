"""Retain only clock and closure witnesses from an already released, independently projected A/A prefix."""

import argparse
import io
import json
import os
from pathlib import Path
import tarfile
import tempfile

import aa_collect as collector
from analyze_aa import analyze, require, unique


def load(path, limit=16_384):
    return json.loads(collector.CONTROL.plain(path, limit).read_text(encoding="utf-8"), object_pairs_hook=unique)


def encoded(value):
    return json.dumps(value, separators=(",", ":"), ensure_ascii=True, allow_nan=False).encode("ascii") + b"\n"


def slot_witness(case, projected, origin):
    pair, arm, run = case["pair"], case["arm"], case["run"]
    batch = Path(case["batch"])
    release = load(batch / "released.json")
    retired = load(batch / "retired.json")
    require(release["state"] == "released" and release["run_id"] == run and release["device_disposition"] == "original-native-restored"
            and release["owner_thread"] == collector.CONTROL.configured_owner() and release["owner_and_token_checked"] is True
            and release["native_restored_and_connections_closed"] is True and retired["natural"] is True, "Missing exact native return")
    card = load(batch / "card.json")
    pid, boot = card["process_id"], card["boot_id"]
    require(card["run_id"] == run and pid == retired["app_pid"] and collector.software_binding(batch, run) == (pair, arm), "Slot, identity or treatment changed")
    clock = load(batch / collector.SIDECAR)
    require(set(clock) == collector.RAW_FIELDS and clock["run_id"] == run and clock["boot_id"] == boot and clock["app_pid"] == pid,
            "Invalid bounded clock record")
    trace = collector.BOUNDS.narrow_rows(collector.BOUNDS.read_jsonl(collector.CONTROL.plain(batch / (run + ".jsonl"), collector.CONTROL.MAX_TRACE)), run, boot, pid)
    audit = collector.BOUNDS.audit(card, trace)
    rendered, = [row["data"] for row in trace if row["stage"] == "shelf_rendered"]
    require(rendered["shelf_state"] == "ready", "Archive requires an observed ready Shelf")
    stamps = {key: clock[key] for key in ("launch_entry", "pre_app_invocation", "rust_main_entry", "state_produced")}
    stamps.update(release_before=card["click"]["up_before_ns"], release_after=card["click"]["up_after_ns"], trace_started=trace[0]["monotonic_ns"],
                  state_consumed=None, shelf_rendered=audit["landmarks_ns"]["shelf_rendered"], shelf_submitted=audit["landmarks_ns"]["shelf_submitted"],
                  driver_completed=audit["landmarks_ns"]["shelf_driver_completed"])
    require({key: value - origin if value is not None else None for key, value in stamps.items()} == projected["clock_offset_ns"], "Retained clocks disagree with the anonymous projection")
    return {"slot": {"pair": pair, "arm": arm, "run": run}, "input_binding": load(batch / "clock-inputs-verified.json"),
            "release": {"run": run, "exact_owner_matched": True, "owner_and_token_checked": True, "native_restored_and_connections_closed": True},
            "retired": {"natural": True, "app_pid": pid}, "card": card, "clock": clock, "shelf_trace": trace}


def archive(plan_path, projection_path, destination):
    collector.CONTROL.private(plan_path.parent)
    plan = load(plan_path, 65_536)
    require(set(plan) == {"schema", "owner", "host_boot", "cases"} and plan["schema"] == "bili-shelf-clock-aa-v1"
            and plan["owner"] == collector.CONTROL.configured_owner() and collector.CONTROL.UUID.fullmatch(plan["host_boot"]), "Invalid preparation plan")
    output = collector.CONTROL.private(destination.parent)
    require(destination.is_absolute() and destination.parent == output and not os.path.lexists(destination), "Expected a new private output")
    projection = load(projection_path, 65_536)
    require(projection == collector.series_projection(plan["cases"], host_boot=plan["host_boot"]), "Saved projection differs from closed evidence")
    analyze(projection)
    rows = projection["cases"]
    require(rows, "No qualified closed clocks to archive")
    plan_cases = plan["cases"]
    first = load(Path(plan_cases[0]["batch"]) / "card.json")["click"]["up_before_ns"]
    members = {"schedule.json": encoded({"schema": "bili-aa-private-clock-archive-v1", "planned_slots": [list(slot) for slot in collector.SCHEDULE],
                                        "qualified_prefix_count": len(rows)}), "projection.json": encoded(projection)}
    for case, row in zip(plan_cases, rows):
        witness = slot_witness(case, row, first)
        members[f"slots/{row['pair']}-{row['arm']}.json"] = encoded(witness)
    with tempfile.NamedTemporaryFile(dir=output) as temp:
        with tarfile.open(fileobj=temp, mode="w:gz") as bundle:
            for name, payload in members.items():
                entry = tarfile.TarInfo(name)
                entry.mode, entry.size = 0o600, len(payload)
                bundle.addfile(entry, io.BytesIO(payload))
        temp.flush()
        temp.seek(0)
        with tarfile.open(fileobj=temp, mode="r:gz") as replay:
            require(replay.getnames() == list(members) and all(item.type == tarfile.REGTYPE and replay.extractfile(item).read() == members[item.name]
                    for item in replay.getmembers()), "Archive round-trip mismatch")
        os.link(temp.name, destination)
    return {"qualified_closed_visits": len(rows), "regular_allowlisted_members": len(members)}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("plan", "projection", "destination"):
        parser.add_argument(key, type=Path)
    args = parser.parse_args()
    print(json.dumps(archive(args.plan, args.projection, args.destination)))

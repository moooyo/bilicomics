"""Summarize closed ordinary Shelf cache-maintenance cases; optionally audit their original traces."""

import argparse
import json
from pathlib import Path
from statistics import median


PHASES = "timeout_setup begin_immediate recency durable delete_scopes eviction_candidates delete_details delete_members delete_comics reconcile recency_upsert commit".split()
DELETES = ("delete_scopes", "delete_details", "delete_members", "delete_comics")
SCHEDULE = ["f00", "r00"] + [f"{arm}{number:02d}" for number in range(1, 7) for arm in ("fr" if number % 2 else "rf")]
COSTS = ("wall_us", "cpu_us")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def distinct(items):
    result = dict(items)
    require(len(items) == len(result), "Duplicate JSON key")
    return result


def load(path):
    return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=distinct)


def raw_case(directory, case):
    name, pid = directory.name, case["pid"]
    require(load(directory / "case.json") == case, f"{name}: saved case differs from projection")
    perf, life = ([json.loads(line) for line in (directory / leaf).read_text(encoding="utf-8").splitlines()]
                  for leaf in ("performance.jsonl", "lifecycle.jsonl"))
    final = load(directory / "final.json")
    require(all(row["process_id"] == pid for row in perf + life) and final["process_id"] == pid and final["route"] == "shelf"
            and final["backend"] == "desktop-software" and final["profile"] == "scribe" and final["demo"] is False
            and final["offline_requested"] is True and final["offline"] is True and not final["startup"]["booting"]
            and not final["startup"]["error"] and final["ui"]["catalog"]["shelf_pages"] == 2, f"{name}: wrong process or final scene")
    def events(key):
        return [row for row in perf if row["kind"] == "startup" and row["stage"] == key]
    start, end = events("state-cache-maintenance")
    inner, = events("catalog_cache_maintenance")
    require(all(row["thread_id"] == case["worker_tid"] for row in (start, end, inner)) and start["data"]["phase"] == "begin"
            and end["data"]["phase"] == "end" and case["parent"] == {"begin_ns": start["monotonic_ns"], "end_ns": end["monotonic_ns"],
            **{key: end["data"][key] for key in (*COSTS, "outcome")}} and
            case["maintenance"] == {"record_ns": inner["monotonic_ns"], **inner["data"]}, f"{name}: different worker spans")
    ready, = events("ui_ready")
    terminal, = [row for row in life if row["event"] == "ready"]
    require(ready["thread_id"] == pid and case["ui_ready_ns"] == ready["monotonic_ns"] <= terminal["monotonic_ns"] and
            terminal["route"] == "shelf" and terminal["reader_resources"]["cover_resident_count"] == case["ready_covers"] and
            terminal["reader_resources"]["cover_resident_bytes"] == case["resident_bytes"], f"{name}: readiness differs")
    for label, field in (("process_enter", "process_enter_ns"), ("state_prepared", "state_prepared_ns")):
        row, = events(label)
        require(row["thread_id"] == pid and row["data"]["observed"]["monotonic_ns"] == case[field] <= row["monotonic_ns"] and
                [item["monotonic_ns"] for item in life if item.get("stage") == label] == [case[field]], f"{name}: different {label} clock")


def costs(name, case):
    arm = name[0]
    require(case["case"] == name and case["scenario"] == arm and all(case[key] is True for key in
            ("same_executable", "semantics_preserved", "expected_final_representation", "row_hypothesis_matches")) and
            case["ready_covers"] == case["final_marker_count"] == 10 and case["resident_bytes"] == 5_324_800,
            f"{name}: ineligible fixture or unexpected row/representation result; not a qualified timing")
    parent, inner = case["parent"], case["maintenance"]
    stamps = [case["process_enter_ns"], parent["begin_ns"], parent["end_ns"], inner["record_ns"], case["state_prepared_ns"], case["ui_ready_ns"]]
    require(all(type(value) is int and value > 0 for value in (case["pid"], case["worker_tid"], *stamps)) and
            case["worker_tid"] != case["pid"] and stamps == sorted(stamps), f"{name}: invalid worker or clock order")
    require(parent["outcome"] == "success" and inner["outcome"] == "committed" and all(inner[key] is True for key in
            ("committed", "timeout_restored", "recency_present")), f"{name}: busy, missing, failed or uncommitted transaction")
    phases = inner["phases"]
    require([part["stage"] for part in phases] == PHASES and all(part["ok"] is True for part in phases),
            f"{name}: missing, failed or reordered phase")
    expected = {**dict.fromkeys((*DELETES, "eviction_candidates"), 0), "durable": 20, "recency_upsert": int(arm == "f")}
    for part in phases:
        value = expected.get(part["stage"])
        require(type(part["rows"]) is type(value) and part["rows"] == value, f"{name}: unexpected {part['stage']} row count")
    series = {"parent": parent, **{part["stage"]: part for part in phases}}
    require(all(type(row[key]) is int and row[key] >= 0 for row in series.values() for key in COSTS)
            and all(sum(part[key] for part in phases) <= parent[key] for key in COSTS) and
            parent["wall_us"] <= (parent["end_ns"] - parent["begin_ns"]) // 1000 + 1, f"{name}: uncontained or missing costs")
    return series


def table(rows):
    return "\n".join("| " + " | ".join(map(str, row)) + " |" for row in rows)


def summarize(projection, raw=None):
    require(projection["schema"] == "ordinary-cache-maintenance-profile-v1" and projection["phase_cost_unit"] == "microseconds"
            and projection["event_stamp_unit"] == "nanoseconds" and projection["clock"] == "CLOCK_MONOTONIC and worker thread CPU",
            "Unknown maintenance projection or timing units")
    require(projection["schedule"] == SCHEDULE and
            projection["excluded_warmups"] == SCHEDULE[:2] and set(projection["cases"]) == set(SCHEDULE), "Expected two warmups and six fixed FR/RF pairs")
    if raw is not None:
        require(not (raw / "failure.json").exists() and {path.name for path in raw.iterdir() if path.is_dir()} == set(SCHEDULE),
                "Failed, incomplete or extended raw comparison")
    cases = projection["cases"]
    values = {}
    for name in SCHEDULE:
        if raw is not None:
            raw_case(raw / name, cases[name])
        values[name] = costs(name, cases[name])
    require(all(cases[first]["ui_ready_ns"] < cases[second]["process_enter_ns"] for first, second in zip(SCHEDULE, SCHEDULE[1:])),
            "Overlapping, repeated or reordered launches")
    pairs = [(values[f"f{index:02d}"], values[f"r{index:02d}"]) for index in range(1, 7)]
    def amounts(row):
        return "/".join(f"{value:g}" for value in row)
    def delta(fresh, stable, label):
        return [stable[label][key] - fresh[label][key] for key in COSTS]
    rows = [["Pair/order", "Parent F", "Parent R", "Parent R−F", "Commit F", "Commit R", "Commit R−F"], ["---"] * 7]
    for index, (fresh, stable) in enumerate(pairs, 1):
        row = [f"{index} {'FR' if index % 2 else 'RF'}"]
        for label in ("parent", "commit"):
            row.extend([amounts(item[label][key] for key in COSTS) for item in (fresh, stable)] + [amounts(delta(fresh, stable, label))])
        rows.append(row)
    phases = [["Stage", "F median", "R median", "Paired R−F", "FR R−F", "RF R−F"], ["---"] * 6]
    for label in ("parent", *PHASES):
        arms = [amounts(median(pair[arm][label][key] for pair in pairs) for key in COSTS) for arm in (0, 1)]
        changes = [delta(fresh, stable, label) for fresh, stable in pairs]
        groups = [amounts(median(item[index] for item in selected) for index in (0, 1)) for selected in (changes, changes[::2], changes[1::2])]
        phases.append([label, *arms, *groups])
    return ("# Ordinary startup cache maintenance: six paired comparisons\n\n"
            "F is the original fixture text; R contains the same lists in the product's persisted text. The executable is unchanged. "
            "Each cell is elapsed/thread CPU in microseconds. Negative R−F means less time on R. Two warmups are excluded.\n\n"
            + table(rows) + "\n\n" + table(phases) + "\n\n"
            "All 14 launches committed and restored the timeout. Each saw the existing key and 20 durable comic IDs; all four deletes affected zero rows. "
            "The recency UPSERT affected one row on F and zero on R. Phase clocks measure application work, not SQL I/O, fsyncs, window visibility or Kindle latency. "
            "The parent includes uninstrumented intervals; do not add arm medians or attribute all remaining time to storage.\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("projection", type=Path)
    parser.add_argument("--raw", type=Path, help="Also cross-check every case against the closed raw trial")
    args = parser.parse_args()
    print(summarize(load(args.projection), args.raw), end="")

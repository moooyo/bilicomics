"""Replay only the six retained pre-takeover launcher projections; never contact a device."""

import argparse
import json
from pathlib import Path
import re
from statistics import median
import sys

sys.path.append(str(Path(__file__).resolve().parents[2] / "physical" / "shelf-render-20261121"))
from project_launcher_stages import MAX_TRACE, SEQUENCE, TRACE, TRACE_ROW, bounded_read


COLUMNS = (
    "logging_ready_to_sample_begin", "display_sample_begin_to_go", "display_go_to_ready", "display_ready_to_reaped",
    "display_reaped_to_sample_ready", "sample_ready_to_portrait_go", "portrait_go_to_ready", "portrait_ready_to_reaped",
    "portrait_reaped_to_cover_go", "cover_go_to_ready", "cover_ready_to_reaped", "cover_reaped_to_take_begin", "take_begin_to_done",
    "display_sample_total", "display_outside_go_to_ready", "sample_ready_to_take_begin", "sample_begin_to_take_begin_outside_all_reads_and_reaps",
)


def durations(case):
    worker = re.fullmatch(rb"([1-9][0-9]{0,9}) [0-9]{1,20}\n", bounded_read(case / "worker.pid", 40))
    rows = bounded_read(case / TRACE, MAX_TRACE).split(b"\n")
    if not worker or len(rows) != len(SEQUENCE) + 1 or rows.pop() != b"":
        raise ValueError("Invalid worker identity or retained stage count")
    times = []
    for raw, (name, sequence) in zip(rows, SEQUENCE, strict=True):
        row = TRACE_ROW.fullmatch(raw)
        if not row or row[3] != name.encode("ascii") or row[4] != worker[1] or int(row[5]) != sequence:
            raise ValueError("Missing, unowned or reordered launcher stage")
        times.append((int(row[1]) * 100 + int(row[2])) * 10)
    if times != sorted(times):
        raise ValueError("Decreasing launcher clock")
    gaps = [end - start for start, end in zip(times, times[1:])]
    total = times[5] - times[1]
    return [*gaps, total, total - gaps[2], times[12] - times[5], sum(gaps[index] for index in (1, 4, 5, 8, 11))]


def project(batch):
    cases = {f"b{number:02d}": durations(batch / f"b{number:02d}") for number in range(1, 7)}
    return {"scope": "November 21: six measured B launches, first-generation Scribe 5.19.5, offline demonstration. No new launches.",
            "clock": "/proc/uptime", "resolution_ms": 10, "events_per_case": len(SEQUENCE), "columns": list(COLUMNS),
            "case_durations_ms": cases, **{label: [statistic(values) for values in zip(*cases.values(), strict=True)]
                                         for label, statistic in (("min_ms", min), ("median_ms", median), ("max_ms", max))}}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("batch", type=Path, help="Closed directory containing b01 through b06")
    print(json.dumps(project(parser.parse_args().batch), indent=2))

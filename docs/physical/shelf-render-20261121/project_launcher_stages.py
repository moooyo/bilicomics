"""Project one case's pre-takeover shell intervals, entirely on the /proc/uptime clock."""

import argparse
import json
import os
from pathlib import Path
import re
import stat


TRACE = "launcher-stages.tsv"
MAX_TRACE = 2048
SEQUENCE = (
    ("startup_logging_ready", 0), ("startup_sample_begin", 0),
    ("native_read_go", 1), ("native_read_ready", 1), ("native_read_reaped", 1),
    ("startup_sample_ready", 1),
    ("native_read_go", 2), ("native_read_ready", 2), ("native_read_reaped", 2),
    ("native_read_go", 3), ("native_read_ready", 3), ("native_read_reaped", 3),
    ("native_take_begin", 3), ("native_take_done", 3),
)
TRACE_ROW = re.compile(rb"([0-9]{1,12})\.([0-9]{2})\t([a-z_]+)\t([1-9][0-9]{0,9})\t([0-3])")


def bounded_read(path, limit):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or not 0 < info.st_size <= limit:
            raise ValueError(f"Invalid size or type: {path.name}")
        data = source.read(limit + 1)
        if len(data) != info.st_size:
            raise ValueError(f"Unstable input: {path.name}")
        return data


def project(case):
    worker = re.fullmatch(rb"([1-9][0-9]{0,9}) [0-9]{1,20}\n", bounded_read(case / "worker.pid", 40))
    if not worker:
        raise ValueError("Invalid worker identity")
    data = bounded_read(case / TRACE, MAX_TRACE)
    rows = data.split(b"\n")
    if len(rows) != len(SEQUENCE) + 1 or rows[-1]:
        raise ValueError("Missing or repeated launcher stages")
    times = []
    for raw, (event, sequence) in zip(rows, SEQUENCE):
        row = TRACE_ROW.fullmatch(raw)
        if not row or row[3].decode("ascii") != event or row[4] != worker[1] or int(row[5]) != sequence:
            raise ValueError("Unexpected event, worker, or read sequence")
        elapsed = (int(row[1]) * 100 + int(row[2])) * 10
        if times and elapsed < times[-1]:
            raise ValueError("Decreasing launcher uptime")
        times.append(elapsed)
    return {
        "clock": "/proc/uptime", "resolution_ms": 10,
        "interval_ms": {
            "logging_ready_to_sample_begin": times[1] - times[0],
            "sample_begin_to_sample_ready": times[5] - times[1],
            "sample_ready_to_take_begin": times[12] - times[5],
            "take_begin_to_take_done": times[13] - times[12],
        },
        "nested_read_ms": {
            name: {"go_to_ready": times[index + 1] - times[index],
                   "ready_to_reaped": times[index + 2] - times[index + 1]}
            for name, index in (("display_sample", 2), ("portrait_publication", 6), ("cover_query", 9))
        },
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("case", type=Path)
    print(json.dumps(project(parser.parse_args().case), indent=2))

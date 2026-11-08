"""Create the compact input dictionary from the pinned AOSP source file.

This is a source transformation, not a downloader. See README.md for source
revision, attribution, and the original data format.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import re


def transform(source: Path, destination: Path, limit: int | None) -> None:
    entries: dict[tuple[str, str], float] = {}
    for line in source.read_text(encoding="utf-16").splitlines():
        parts = line.split()
        if len(parts) < 4:
            continue
        phrase, frequency, _extended_flag, *syllables = parts
        key = "".join(syllables).lower().replace("u:", "v").replace("\u00fc", "v")
        if not re.fullmatch("[a-z]+", key):
            continue
        if not phrase or any(
            not ("\u3400" <= char <= "\u9fff" or char == "\u3007") for char in phrase
        ):
            continue
        identity = key, phrase
        entries[identity] = max(float(frequency), entries.get(identity, 0.0))

    by_frequency = sorted(entries, key=lambda item: (-entries[item], item))
    # Full coverage is the default. An optional size limit retains every
    # single-character reading before selecting phrases by source frequency.
    selected = set(by_frequency)
    if limit is not None:
        selected = {item for item in by_frequency if len(item[1]) == 1}
        if len(selected) > limit:
            raise ValueError("Dictionary limit is smaller than the single-character set")
        for item in by_frequency:
            if len(selected) >= limit:
                break
            selected.add(item)
    rank = {item: index + 1 for index, item in enumerate(by_frequency)}
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("w", encoding="utf-8", newline="\n") as output:
        for key, phrase in sorted(selected, key=lambda item: (item[0], rank[item], item[1])):
            output.write(f"{key}\t{phrase}\t{rank[(key, phrase)]}\n")
    print(f"Wrote {len(selected)} entries to {destination}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--limit", type=int)
    args = parser.parse_args()
    if args.limit is not None and args.limit < 1:
        parser.error("--limit must be positive")
    transform(args.source, args.destination, args.limit)


if __name__ == "__main__":
    main()

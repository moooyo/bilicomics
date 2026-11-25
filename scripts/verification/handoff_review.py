#!/usr/bin/env python3
"""Index and display reference/capture pairs without changing original pixels."""

import argparse
import json
from pathlib import Path
import shutil

from PIL import Image, ImageDraw


GROUPS = {
    "startup-auth": ("startup", "welcome", "login", "sleep"),
    "catalog": ("shelf", "store", "network", "search"),
    "chapters": ("detail", "directory"),
    "reader": ("reader", "light"),
    "purchase": ("purchase",),
    "downloads": ("downloads",),
    "account": ("account", "acct"),
}


def group(name):
    return next(key for key, prefixes in GROUPS.items() if name.split("-", 1)[0] in prefixes)


def sheet(rows, output):
    w, h = 188, 251
    grid = Image.new("RGB", (4 * w + 20, ((len(rows) + 1) // 2) * (h + 30)), "white")
    draw = ImageDraw.Draw(grid)
    for index, (label, reference, candidate) in enumerate(rows):
        x, y = (index % 2) * (2 * w + 10), (index // 2) * (h + 30)
        draw.text((x + 3, y + 4), label, fill="black")
        for offset, path in ((0, reference), (w, candidate)):
            if path is None:
                draw.text((x + offset + 8, y + 80), "No capture", fill="black")
                continue
            with Image.open(path) as image:
                image = image.convert("RGB")
                image.thumbnail((w, h), Image.Resampling.LANCZOS)
                grid.paste(image, (x + offset, y + 20))
    grid.save(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference", type=Path, required=True, help="Reference screenshots directory only")
    parser.add_argument("--captured", type=Path, required=True, action="append", help="Candidate raw directory, ordered by preference")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--baseline", type=Path, help="Export only original captures and their state metadata")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    if args.baseline:
        args.baseline.mkdir(parents=True, exist_ok=False)
    pairs = []
    for profile, design_profile in (("pw6", "paperwhite"), ("scribe", "scribe")):
        groups = {key: [] for key in GROUPS}
        for reference in sorted((args.reference / design_profile).glob("*.png")):
            candidate = next((root / profile / reference.name for root in args.captured
                              if (root / profile / reference.name).is_file()), None)
            native_size = None
            scope = None
            keep = None
            if candidate:
                with Image.open(reference) as left, Image.open(candidate) as right:
                    native_size = left.size == right.size
                sidecar = candidate.with_suffix(".json")
                metadata = json.loads(sidecar.read_text()) if sidecar.is_file() else {}
                scope = metadata.get("scope", "application-intent replay under Xvfb")
                keep = Path("raw") / profile / candidate.name
                if args.baseline:
                    target = args.baseline / keep
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(candidate, target)
                    if sidecar.is_file():
                        shutil.copyfile(sidecar, target.with_suffix(".json"))
            groups[group(reference.stem)].append((reference.stem, reference, candidate))
            pairs.append({"profile": profile, "state": reference.stem, "same_dimensions": native_size,
                          "scope": scope, "candidate": str(keep) if keep else None})
        for key, rows in groups.items():
            for i in range(0, len(rows), 12):
                sheet(rows[i:i + 12], args.output / f"{profile}-{key}-{i // 12 + 1}.png")
    index = json.dumps(pairs, indent=2) + "\n"
    (args.output / "pairs.json").write_text(index)
    if args.baseline:
        (args.baseline / "scene-index.json").write_text(index)
    matching = sum(row["same_dimensions"] is True for row in pairs)
    print(f"Compared {matching} / {len(pairs)} dimension-matched pairs")
    for row in pairs:
        if row["same_dimensions"] is not True:
            print(row["profile"], row["state"], "missing" if row["candidate"] is None else "dimension mismatch")
    return int(not pairs or matching != len(pairs))


if __name__ == "__main__":
    raise SystemExit(main())

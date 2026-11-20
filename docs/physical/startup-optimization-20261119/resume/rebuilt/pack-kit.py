"""Package only the allowlisted ARM read-only probe and its private distribution runtime."""

import argparse
import io
import json
from pathlib import Path
import re
import subprocess
import tarfile


root = Path("/work")
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--paired", action="store_true")
args = parser.parse_args()
libraries = root / "runtime/usr/lib/arm-linux-gnueabihf"
models = root / "runtime/usr/share/tesseract-ocr/5/tessdata"
loader = libraries / "ld-linux-armhf.so.3"
probe = root / ("card-input" if args.paired else "native-target")
listing = subprocess.check_output(["qemu-arm", "-cpu", "cortex-a9", loader, "--library-path", libraries, "--list", probe], text=True)
members = {"native-target": probe, "lib/ld-linux-armhf.so.3": loader}
if args.paired:
    members.update({"native-target": root / "native-target", "card-input": probe})
for row in listing.splitlines():
    if " => " not in row:
        continue
    requested, resolved = row.strip().split(" => ", 1)
    if requested == "/lib/ld-linux-armhf.so.3":
        continue
    path = Path(resolved.split()[0])
    if not re.fullmatch(r"lib[A-Za-z0-9._+-]+", requested) or path.parent != libraries or not path.resolve(strict=True).is_relative_to(libraries):
        raise ValueError(f"Unqualified loader path: {requested}")
    members["lib/" + requested] = path
for language in ("eng", "chi_sim"):
    name = language + ".traineddata"
    members["models/" + name] = models / name
if not {"lib/libX11.so.6", "lib/libtesseract.so.5", "lib/libc.so.6"} <= members.keys():
    raise ValueError("Incomplete ARM runtime")
stem = "paired-kit" if args.paired else "navigation-kit"
with tarfile.open(root / (stem + ".tar.gz"), "x:gz") as archive:
    for name in sorted(members):
        data = members[name].read_bytes()
        info = tarfile.TarInfo(name)
        info.size = len(data)
        info.mode = 0o700 if not name.startswith("models/") else 0o600
        archive.addfile(info, io.BytesIO(data))
(root / (stem + ".json")).write_text(json.dumps({"members": sorted(members), "bytes": sum(
    path.stat().st_size for path in members.values())}, indent=2) + "\n")

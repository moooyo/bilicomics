"""Assemble an isolated, single-launch diagnostic route outside the device lease."""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import stat
import struct


SOURCE = Path(__file__).resolve().parent
RUN = re.compile(r"bili-shelf-[0-9a-f]{32}")
APP_ARGS = b' --kindle --device "$PROFILE" --data-dir "$DATA_DIR" >&6'
DOCUMENT = b"#!/bin/sh\n# Name: BiliComics for Kindle\nset -eu\nexec sh /mnt/us/bilicomics/launch.sh --library\n"
SERIAL_OPTION = b" --serial-shelf-render"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def regular(path):
    before = path.lstat()
    require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1, f"Not a plain, single-link input: {path.name}")
    return before


def arm_executable(path):
    size = regular(path).st_size
    require(52 <= size <= 128 * 1024 * 1024, f"Invalid ARM executable size: {path.name}")
    with path.open("rb") as source:
        header = source.read(52)
        require(header[:7] == b"\x7fELF\x01\x01\x01", f"Not little-endian ELF32: {path.name}")
        kind, machine = struct.unpack_from("<HH", header, 16)
        require(kind in (2, 3) and machine == 40 and struct.unpack_from("<I", header, 36)[0] & 0x400,
                f"Not an ARM hard-float executable: {path.name}")
        offset = struct.unpack_from("<I", header, 28)[0]
        width, count = struct.unpack_from("<HH", header, 42)
        require(52 <= offset and width == 32 and 0 < count <= 64 and offset + width * count <= size,
                f"Invalid ELF program headers: {path.name}")
        source.seek(offset)
        segments = [struct.unpack_from("<I", source.read(width))[0] for _index in range(count)]
        require(1 in segments and 3 not in segments, f"Executable requires a dynamic interpreter: {path.name}")


def diagnostic_launcher(shipping, run, serial_shelf_render=False):
    require(RUN.fullmatch(run) and type(serial_shelf_render) is bool, "Invalid diagnostic binding")
    require(shipping.startswith(b"#!/bin/sh\n") and shipping.endswith(b"\n") and b"\r" not in shipping and
            b"\x00" not in shipping and shipping.count(APP_ARGS) == 1 and
            b'"$APP_DIR/bilicomics"' + APP_ARGS in shipping,
            "Shipping launcher does not have the one recognized application invocation")
    args = (b' --demo --offline --shelf-render-trace "$DATA_DIR/' + run.encode("ascii") +
            b'.jsonl" --exit-after-ms 2500')
    if serial_shelf_render:
        args += SERIAL_OPTION
    return shipping.replace(APP_ARGS, APP_ARGS.replace(b" >&6", args + b" >&6"))


def diagnostic_switch(source, serial_shelf_render):
    binding = b"    order_option=''\n"
    require(source.count(binding) == 1 and type(serial_shelf_render) is bool, "Unknown route verifier binding")
    return source.replace(binding, b"    order_option='" + SERIAL_OPTION + b"'\n") if serial_shelf_render else source


def entry(run):
    return f'''#!/bin/sh
set -eu
[ "$#" -eq 1 ] && [ "$1" = --library ] || exit 1
BILICOMICS_HOME=/mnt/us/{run}
[ -d "$BILICOMICS_HOME/armed" ] && [ ! -L "$BILICOMICS_HOME/armed" ] || exit 1
umask 077
mkdir "$BILICOMICS_HOME/picked" || exit 1
IFS= read -r process_stat <"/proc/$$/stat" || exit 1
[ "${{process_stat%% *}}" = "$$" ] || exit 1
set -- ${{process_stat##*) }}
[ "$#" -ge 20 ] || exit 1
shift 19
case "$1" in ''|*[!0-9]*) exit 1;; esac
set -C
printf '%s %s %s\\n' '{run}' "$$" "$1" >"$BILICOMICS_HOME/picked/entry" || exit 1
[ -d "$BILICOMICS_HOME/armed" ] && [ ! -L "$BILICOMICS_HOME/armed" ] || exit 1
export BILICOMICS_HOME
unset BILICOMICS_LAUNCH_TRACE
exec sh "$BILICOMICS_HOME/launch.sh" --library
'''.encode("ascii")


def demo_roster():
    root = SOURCE.parents[3] / "assets/demo"
    require(root.is_dir(), "The source-matched bundled demo assets are required")
    result = []
    for path in sorted(root.rglob("*")):
        if path.is_dir() and not path.is_symlink():
            continue
        regular(path)
        name = path.relative_to(root).as_posix()
        require(re.fullmatch(r"(?:covers/|pages/)?[a-z0-9-]+\.(?:png|json)", name), "Unexpected demo asset path")
        result.append(f"data/demo/fixtures/{name}")
    require("data/demo/fixtures/catalog.json" in result and "data/demo/fixtures/manifest.json" in result,
            "Incomplete bundled demo asset roster")
    return "\n".join(result).encode("ascii") + b"\n"


def case_files(shipping, run, serial_shelf_render):
    launcher = diagnostic_launcher(shipping, run, serial_shelf_render)
    shim = entry(run)
    return {"launch.sh": launcher, "run-id": run.encode("ascii") + b"\n", "entry.sh": shim,
            "entry-ready.sh": shim, "switch.sh": diagnostic_switch((SOURCE / "switch.sh").read_bytes(), serial_shelf_render)}


def assemble(args, roster, serial_shelf_render=False):
    require(RUN.fullmatch(args.run), "Expected a fresh bili-shelf run ID")
    for path in (args.application, args.worker, args.route_fs):
        arm_executable(path)
    for path in (args.shipping_launcher, args.document_launcher):
        require(regular(path).st_size <= 200_000, "Unreasonable launcher size")
    shipping = args.shipping_launcher.read_bytes()
    require(args.document_launcher.read_bytes() == DOCUMENT, "Not the known lowercase Library card source")
    bindings = case_files(shipping, args.run, serial_shelf_render)
    output = args.output
    output.mkdir(mode=0o700)
    for relative in ("app", "data", "data/demo"):
        (output / relative).mkdir(mode=0o700)
    for source, relative in ((args.application, "app/bilicomics"), (args.worker, "app/kindle-jpeg-worker"),
                             (args.route_fs, "route-fs")):
        destination = output / relative
        with source.open("rb") as input_file, destination.open("xb") as output_file:
            shutil.copyfileobj(input_file, output_file)
        destination.chmod(0o700)
    files = {
        **bindings,
        "shipping-launcher.sh": shipping,
        "document-launcher.sh": DOCUMENT,
        "data/device-profile": b"scribe\n",
        "data/device-allowlist": b"scribe 5.19.5\n",
        "cleanup-assets": roster,
    }
    for relative, content in files.items():
        with (output / relative).open("xb") as file:
            file.write(content)
    seed = output / "data/demo/state.sqlite3"
    with sqlite3.connect(seed) as database:
        database.execute("CREATE TABLE preferences (account_key TEXT NOT NULL, key TEXT NOT NULL, "
                         "payload TEXT NOT NULL, PRIMARY KEY(account_key,key))")
        database.execute("INSERT INTO preferences VALUES (?, ?, ?)",
                         ("design-demo", "preferences", json.dumps({"launch": "shelf"})))
    seed.chmod(0o600)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", required=True)
    parser.add_argument("--serial-shelf-render", action="store_true", help="Use the serial comparison arm instead of the default render order")
    for name in ("application", "worker", "shipping-launcher", "document-launcher", "route-fs", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    try:
        assemble(args, demo_roster(), args.serial_shelf_render)
    except (OSError, ValueError, sqlite3.Error) as error:
        parser.exit(1, f"prepare_route: {error}\n")


if __name__ == "__main__":
    main()

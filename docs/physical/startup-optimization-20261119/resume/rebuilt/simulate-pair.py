"""Rehearse the same-package install, 14 arm selections and removal in a private host fixture."""

import argparse
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import tarfile
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("batch", type=Path)
    args = parser.parse_args()
    root = Path("/work")
    batch = args.batch.resolve(strict=True)
    assert Path.cwd() == root and batch.parent == root and os.environ.get("BILI_CLICK_FIXTURE") == "1"
    admission = json.loads((batch / "admission.json").read_text())
    assert admission["dry_only"] and admission["source"]["A"] == admission["source"]["B"]
    run = admission["run"]
    with tempfile.TemporaryDirectory(dir=root) as temporary:
        fixture = Path(temporary)
        media, private, tools, local = (fixture / name for name in ("media", "private", "bin", "local"))
        for path in (media, private, tools, local, media / "documents"):
            path.mkdir(mode=0o700)
        stage, runtime = [parent / ("bilicomics-" + run) for parent in (media, private)]
        for path in (stage, runtime):
            path.mkdir(mode=0o700)
            (path / "owner").write_text(run + "\n")
        with tarfile.open(batch / "payload.tar.gz", "r:gz") as archive:
            for entry in archive:
                if not entry.name.startswith("probe/"):
                    archive.extract(entry, stage, filter="data")
        entry = stage / "tools/entry.sh"
        control = stage / "tools/control.sh"
        for path in (entry, control):
            text = path.read_text().replace("/mnt/us", str(media)).replace("/var/tmp", str(private)).replace("/var/local", str(local))
            text = text.replace("/tmp/bilicomics-launcher.lock", str(private / "bilicomics-launcher.lock"))
            text = text.replace("/tmp/$project-launcher.lock", str(private) + "/$project-launcher.lock")
            path.write_text(text)
            subprocess.run(["sh", "-n", path], check=True)
        for arm in "AB":
            subprocess.run(["sh", "-n", stage / f"park-{arm}/launch.sh"], check=True)
        document = media / "documents/bilicomics.sh"
        weread = (media / "documents/weread.sh", media / "weread/launch.sh", media / "weread/app/weread")
        for path in weread:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"fixture installation: " + path.name.encode())
            path.chmod(0o755)
        registry = local / "appreg.db"
        with sqlite3.connect(registry) as database:
            database.execute("CREATE TABLE properties(handlerId, name, value)")
            database.execute("INSERT INTO properties VALUES (?, 'command', ?)",
                             ("tech.hackerdude.shell_integration.launcher", str(local / "kmc/bin/sh_integration_launcher")))
        catalog = local / "cc.db"
        with sqlite3.connect(catalog) as database:
            database.execute("CREATE TABLE Entries(p_location TEXT COLLATE NOCASE, p_titles_0_nominal TEXT, p_mimeType TEXT)")
        live_row = (str(weread[0]), "WeRead fixture", "text/x-shellscript")
        legacy_path = str(weread[0].with_name("WeRead.sh"))

        def set_weread_rows(rows):
            with sqlite3.connect(catalog) as database:
                database.execute("DELETE FROM Entries WHERE p_location COLLATE BINARY IN (?, ?)", (live_row[0], legacy_path))
                database.executemany("INSERT INTO Entries VALUES (?, ?, ?)", rows)

        set_weread_rows([live_row])
        scripts = {
            "lipc-get-prop": '''case "$*" in *appmgrd*) echo com.lab126.KPPMainApp;; *powerd\\ state) echo active;; *) echo 10;; esac''',
            "status": "echo start/running",
            "pidof": 'echo "$BILI_FIXTURE_PID"',
            "lipc-set-prop": f'''test "$*" = "-i com.lab126.scanner doFullScan 1"
exec python3 - <<'PY'
import sqlite3
from pathlib import Path
document = Path({str(document)!r})
with sqlite3.connect({str(catalog)!r}) as database:
    database.execute("DELETE FROM Entries WHERE p_location COLLATE BINARY=?", (str(document),))
    if document.is_file():
        database.execute("INSERT INTO Entries VALUES (?, ?, ?)", (str(document), "BiliComics for Kindle", "text/x-shellscript"))
PY''',
            "sqlite3": f'''exec python3 - "$@" <<'PY'
import sqlite3, sys
from pathlib import Path
readonly, command, timeout, filename, query = sys.argv[1:]
assert (readonly, command, timeout) == ("-readonly", "-cmd", ".timeout 1500")
assert filename in ({str(catalog)!r}, {str(registry)!r})
with sqlite3.connect(Path(filename).as_uri() + "?mode=ro", uri=True) as database:
    for row in database.execute(query):
        print("|".join("" if value is None else str(value) for value in row))
PY''',
        }
        for name, body in scripts.items():
            path = tools / name
            path.write_text("#!/bin/sh\nset -eu\n" + body + "\n")
            path.chmod(0o700)
        env = dict(os.environ, PATH=str(tools) + ":" + os.defpath, BILI_FIXTURE_PID=str(os.getpid()))
        sentinel = media / "documents/unrelated"
        sentinel.write_bytes(b"untouched")
        def action(*parts, ok=True):
            result = subprocess.run(["sh", control, run, *parts], env=env, capture_output=True, timeout=60)
            assert (result.returncode == 0) == ok, (parts, result.stderr.decode()[-350:])
        canonical = media / "bilicomics"
        canonical.mkdir()
        foreign = canonical / "foreign"
        foreign.write_bytes(b"untouched")
        action("bootstrap", ok=False)
        assert foreign.read_bytes() == b"untouched"
        foreign.unlink()
        canonical.rmdir()
        unavailable = registry.with_suffix(".offline")
        registry.rename(unavailable)
        action("bootstrap", ok=False)
        assert not (runtime / "protection.before").exists() and not canonical.exists() and not document.exists()
        unavailable.rename(registry)
        action("bootstrap")
        assert document.read_bytes() == (stage / "document").read_bytes()

        first_case = admission["schedule"][0]
        for path in weread:
            parked = path.with_suffix(".parked")
            path.rename(parked)
            action("arm", first_case, ok=False)
            assert not path.exists() and not (runtime / first_case).exists()
            parked.rename(path)
        for changed in ([], [live_row, live_row], [(legacy_path, *live_row[1:])],
                        [(live_row[0], "Changed title", live_row[2])], [(*live_row[:2], "application/octet-stream")]):
            set_weread_rows(changed)
            action("arm", first_case, ok=False)
            assert not (runtime / first_case).exists()
        set_weread_rows([live_row])
        unrelated = (str(sentinel), "Unrelated fixture", "text/plain")
        with sqlite3.connect(catalog) as database:
            database.execute("INSERT INTO Entries VALUES (?, ?, ?)", unrelated)

        router = canonical / "launch.sh"
        original_inode = router.stat().st_ino
        intercept = fixture / "intercept"
        intercept.mkdir()
        shell = intercept / "sh"
        shell.write_text('#!/bin/sh\nprintf "%s\\n" "$BILICOMICS_HOME" "$@" >"$BILI_FIXTURE_DISPATCH"\n')
        shell.chmod(0o700)
        for name in admission["schedule"]:
            action("arm", name)
            assert (runtime / "active").read_text().strip() == name
            assert router.stat().st_ino == original_inode and router.read_bytes() == entry.read_bytes()
            captured = fixture / (name + ".dispatch")
            dispatch_env = dict(env, BILI_FIXTURE_DISPATCH=str(captured), PATH=str(intercept) + ":" + env["PATH"])
            dispatch = ["/bin/sh", router, "--library"]
            assert subprocess.run(dispatch, env=dispatch_env, capture_output=True).returncode != 0
            (runtime / name / "armed").mkdir()
            subprocess.run(dispatch, env=dispatch_env, check=True)
            selected = stage / ("park-" + name[0].upper())
            assert captured.read_text().splitlines() == [str(selected), str(selected / "launch.sh"), "--library"]
            assert subprocess.run(dispatch, env=dispatch_env, capture_output=True).returncode != 0
        document.write_bytes(b"unexpected owner")
        action("uninstall", ok=False)
        assert document.read_bytes() == b"unexpected owner"
        shutil.copyfile(stage / "document", document)
        action("quiesce")
        set_weread_rows([live_row, live_row])
        action("uninstall", ok=False)
        with sqlite3.connect(catalog) as database:
            assert database.execute("SELECT COUNT(*) FROM Entries WHERE p_location=?", (live_row[0],)).fetchone() == (2,)
        set_weread_rows([live_row])
        action("uninstall")
        assert not document.exists() and not (media / "bilicomics").exists() and sentinel.read_bytes() == b"untouched"
        with sqlite3.connect(catalog) as database:
            assert sorted(database.execute("SELECT * FROM Entries")) == sorted([live_row, unrelated])
    receipt = {"passed": True, "dry_only": True, "same_package": True, "arms": admission["schedule"],
               "foreign_document_preserved": True, "foreign_install_preserved": True, "canonical_entry_unchanged": True,
               "weread_live_paths_and_index_drift_rejected": True, "unrelated_index_preserved": True,
               "one_shot_library_dispatch_checked_for_all_cases": True, "lease_acquired": False, "device_accessed": False}
    with (batch / "rehearsal.json").open("x") as output:
        json.dump(receipt, output, indent=2)
        output.write("\n")
    print(json.dumps(receipt), flush=True)


if __name__ == "__main__":
    main()

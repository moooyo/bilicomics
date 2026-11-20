"""Rehearse release installation, scoped process proof and refusal cases in the isolated Linux fixture."""

import argparse
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import tempfile
import zipfile


def executable(path, body):
    path.write_text("#!/bin/sh\nset -eu\n" + body + "\n")
    path.chmod(0o700)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    args = parser.parse_args()
    assert Path.cwd() == Path("/work") and os.environ.get("BILI_ROLLOUT_FIXTURE") == "1"
    source = Path(__file__).with_name("rollout-device.sh").read_text()
    with tempfile.TemporaryDirectory(dir="/work") as directory:
        root = Path(directory)
        media, private, tools, local, locks, proc = (root / part for part in ("media", "private", "bin", "local", "locks", "proc"))
        for path in (media, private, tools, local, locks, proc, media / "documents"):
            path.mkdir()
        run = "bili-prod-a0123456"
        stage, private_run = (base / ("bilicomics-" + run) for base in (media, private))
        for path in (stage, private_run):
            path.mkdir()
            (path / "owner").write_text(run + "\n")
        (stage / "tools").mkdir()
        package = stage / "package"
        with zipfile.ZipFile(args.archive) as archive:
            assert len(archive.namelist()) == 49
            archive.extractall(package)
        home, doc = media / "bilicomics", media / "documents/bilicomics.sh"
        doc_peer = media / "documents/weread.sh"
        doc_peer.write_text("unchanged")
        lock = locks / "bilicomics-launcher.lock"
        firmware = root / "firmware"
        firmware.write_text("Kindle 5.19.5 (fixture)\n")
        busybox, system_shell = root / "busybox", root / "system-sh"
        busybox.write_bytes(b"fixture shell executable")
        system_shell.symlink_to(busybox)
        for literal, replacement in (("/mnt/us", str(media)), ("/var/tmp", str(private)), ("/var/local", str(local)),
                                     ("/etc/prettyversion.txt", str(firmware)), ("/tmp/", str(locks) + "/"), ("/proc/", str(proc) + "/")):
            source = source.replace(literal, replacement)
        assert source.count("document_lookup() {\n") == source.count('document_matches() { cmp -s "$1" "$2"; }') == 1
        source = source.replace("document_lookup() {\n", '''fixture_target() {
    case "$1" in
        "${doc%/*}/BiliComics.sh") test "$BILI_FIXTURE_ALIAS" != none && printf '%s\\n' "$doc";;
        "$stage/.document-casefold/FoldProbe") test "$BILI_FIXTURE_PROBE" != blind && printf '%s\\n' "$stage/.document-casefold/foldprobe";;
        *) return 1;;
    esac
}
document_lookup() {
    if ! present "$1" && mapped=$(fixture_target "$1"); then
        if [ "$1" != "${doc%/*}/BiliComics.sh" ] && [ -f "$priv/probe-published" ] && ! present "$mapped"; then
            test ! -d "$stage/.document-casefold" || return 1
            reads=$(cat "$priv/probe-published")
            printf '%s\\n' "$((reads + 1))" >"$priv/probe-published"
            if [ "$BILI_FIXTURE_PROBE" = lingering ] || { [ "$BILI_FIXTURE_PROBE" = delayed ] && [ "$reads" -lt 3 ]; }; then
                printf '1:2\\n'; return 0
            fi
        fi
        if present "$mapped"; then
            actual=$(kind_id f "$mapped") || return 1
            if [ "$1" = "${doc%/*}/BiliComics.sh" ]; then
                case "$BILI_FIXTURE_ALIAS" in
                    same) printf '%s\\n' "$actual"; return 0;;
                    other-device) printf '0:1\\n'; return 0;;
                esac
            elif [ "$BILI_FIXTURE_PROBE" = lingering ] || [ "$BILI_FIXTURE_PROBE" = delayed ]; then printf '0\\n' >"$priv/probe-published"
            fi
            printf '%s:%s\\n' "${actual%%:*}" "$((${actual#*:} + 1))"; return 0
        fi
    fi
''').replace('document_matches() { cmp -s "$1" "$2"; }', '''document_matches() {
    if ! present "$2" && mapped=$(fixture_target "$2"); then
        if [ "$2" = "${doc%/*}/BiliComics.sh" ] && [ "$BILI_FIXTURE_ALIAS" = other-bytes ]; then return 1; fi
        if [ "$2" != "${doc%/*}/BiliComics.sh" ] && [ "$BILI_FIXTURE_PROBE" = stale ] &&
            grep -q ':second$' "$mapped"; then return 1; fi
        set -- "$1" "$mapped"
    fi
    cmp -s "$1" "$2"
}''')
        source = source.replace("readlink -f /bin/sh", f"readlink -f {system_shell}")
        assert source.count('kill -TERM "$worker"') == 1
        source = source.replace('kill -TERM "$worker"', 'fixture-term "$worker"')
        control = stage / "tools/control.sh"
        control.write_text(source)
        subprocess.run(["sh", "-n", control], check=True)
        (proc / "67").mkdir()
        (proc / "67/status").write_text("State:\tS (fixture)\n")
        (proc / "sys/kernel/random").mkdir(parents=True)
        (proc / "sys/kernel/random/boot_id").write_text("fixture-boot\n")
        executable(tools / "pidof", "echo 67")
        executable(tools / "status", "echo start/running")
        executable(tools / "lipc-get-prop", 'case "$*" in *appmgrd*) echo com.lab126.KPPMainApp;; *powerd\\ state) echo active;; *) echo 8;; esac')
        executable(tools / "usleep", ":")
        executable(tools / "sleep", ":")
        executable(tools / "setsid", f'''test "$1" = sh && test "$3" = {run} && test "$4" = guard
printf '90100:200\\n' >"{private_run}/state/guard.pid"''')
        handler = local / "kindle-catalog-entry/v1-qualified/aidoku-library-dispatch"
        handler.parent.mkdir(parents=True)
        executable(handler, "exit 1")
        registry, catalog = local / "appreg.db", local / "cc.db"
        with sqlite3.connect(registry) as database:
            database.execute("CREATE TABLE properties(handlerId, name, value)")
            database.execute("INSERT INTO properties VALUES('tech.hackerdude.shell_integration.launcher','command',?)", (str(handler),))
        with sqlite3.connect(catalog) as database:
            database.execute("CREATE TABLE Entries(p_location TEXT COLLATE NOCASE,p_titles_0_nominal,p_mimeType)")
            database.execute("INSERT INTO Entries VALUES(?, 'peer', 'text/x-shellscript')", (str(doc_peer),))
        executable(tools / "sqlite3", f'''exec python3 - "$@" <<'PY'
import sqlite3, sys
from pathlib import Path
assert sys.argv[1:4] == ["-readonly","-cmd",".timeout 1500"] and sys.argv[4] in ({str(registry)!r},{str(catalog)!r})
with sqlite3.connect(Path(sys.argv[4]).as_uri() + "?mode=ro", uri=True) as db:
    for row in db.execute(sys.argv[5]): print(*row, sep="|")
PY''')
        executable(tools / "lipc-set-prop", f'''test "$*" = "-i com.lab126.scanner doFullScan 1"
exec python3 - <<'PY'
import sqlite3
from pathlib import Path
with sqlite3.connect({str(catalog)!r}) as db:
    db.execute("DELETE FROM Entries WHERE p_location COLLATE BINARY=?", ({str(doc)!r},))
    if Path({str(doc)!r}).is_file(): db.execute("INSERT INTO Entries VALUES(?, 'BiliComics for Kindle', 'text/x-shellscript')", ({str(doc)!r},))
PY''')
        state = private_run / "state"
        env = dict(os.environ, PATH=str(tools) + ":" + os.defpath, BILI_FIXTURE_ALIAS="different", BILI_FIXTURE_PROBE="fold")
        def action(op, good=True):
            command = ["/bin/sh", "-c", f'''mkdir "{proc}/$$"
sed 's/^\\([0-9]*\\) ([^)]*) .*/\\1 (control) S 1 1 1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 100/' /proc/$$/stat >"{proc}/$$/stat"
trap 'rm -rf "{proc}/$$"' 0
script=$1; shift
. "$script"''', "rehearse", control, run, op]
            result = subprocess.run(command, env=env, capture_output=True, timeout=30)
            assert (result.returncode == 0) == good, (op, result.returncode, result.stderr.decode()[-150:])
            return result.stdout.decode()
        def reset():
            shutil.rmtree(state)
        action("native")
        home.mkdir()
        action("install", False)
        home.rmdir()
        doc.symlink_to(doc_peer)
        action("install", False)
        assert doc.is_symlink() and doc_peer.read_text() == "unchanged"
        doc.unlink()
        uppercase = doc.with_name("BiliComics.sh")
        for spelling in (uppercase, doc.with_name("BILICOMICS.SH")):
            spelling.write_text("foreign entry")
            action("install", False)
            assert spelling.read_text() == "foreign entry" and not home.exists()
            spelling.unlink()
        occupied = stage / ".document-casefold"
        marker = private_run / "probe-published"
        occupied.mkdir()
        (occupied / "foreign").touch()
        action("install", False)
        assert not home.exists() and not doc.exists() and (occupied / "foreign").is_file()
        (occupied / "foreign").unlink()
        occupied.rmdir()
        reset()
        for behavior in ("stale", "lingering"):
            env["BILI_FIXTURE_PROBE"] = behavior
            action("install", False)
            assert not home.exists() and not doc.exists() and not occupied.exists() and not (state / "document-volume").exists()
            if behavior == "lingering":
                assert marker.read_text() == "71\n"
                marker.unlink()
            action("rollback")
            reset()
        env["BILI_FIXTURE_PROBE"] = "delayed"
        action("install")
        assert marker.read_text() == "4\n"
        marker.unlink()
        assert (home / "data/device-profile").read_bytes() == b"scribe\n"
        assert (home / "data/device-allowlist").read_bytes() == b"scribe 5.19.5\n"
        assert sum(path.is_file() for path in home.rglob("*")) == 48
        def proof():
            return dict(line.split("=", 1) for line in action("document-view").splitlines())
        view = proof()
        assert view["doc_alias_mode"] == "casefold-proven" and view["doc_lower_id"] != view["doc_upper_id"]
        for alias in ("other-device", "other-bytes", "none"):
            env["BILI_FIXTURE_ALIAS"] = alias
            action("document-view", False)
            action("rollback", False)
            assert doc.exists() and home.is_dir() and not occupied.exists()
        env.update(BILI_FIXTURE_ALIAS="different", BILI_FIXTURE_PROBE="stale")
        occupied.mkdir()
        (occupied / "foreign").touch()
        assert proof() == view
        assert (occupied / "foreign").is_file()
        (occupied / "foreign").unlink()
        occupied.rmdir()
        boot = proc / "sys/kernel/random/boot_id"
        boot.write_text("replaced-boot\n")
        action("document-view", False)
        boot.write_text("fixture-boot\n")
        witness = state / "document-volume"
        current = witness.read_bytes()
        witness.write_bytes(current.replace(run.encode(), b"bili-prod-deadbeef"))
        action("document-view", False)
        witness.write_bytes(current)
        witness.chmod(0o644)
        action("document-view", False)
        witness.chmod(0o600)
        assert proof() == view
        with sqlite3.connect(catalog) as database:
            database.execute("INSERT INTO Entries VALUES(?, 'BiliComics for Kindle','text/x-shellscript')",
                             (str(doc.with_name("BiLiCoMiCs.Sh")),))
        action("rollback", False)
        with sqlite3.connect(catalog) as database:
            database.execute("DELETE FROM Entries WHERE p_location COLLATE BINARY=?", (str(doc.with_name("BiLiCoMiCs.Sh")),))
        env["BILI_FIXTURE_ALIAS"] = "same"
        assert proof()["doc_alias_mode"] == "same-inode"
        for make_extra in (lambda: uppercase.write_text("foreign"), lambda: os.link(doc, uppercase),
                           lambda: uppercase.symlink_to(doc)):
            make_extra()
            action("rollback", False)
            assert doc.is_file() and uppercase.exists() and home.is_dir()
            uppercase.unlink()
        for spelling in ("bilicomics.sh.sdr", "BiliComics.sdr", "BILICOMICS.SH.SDR"):
            sidecar = doc.with_name(spelling)
            sidecar.mkdir()
            action("rollback", False)
            assert sidecar.is_dir() and doc.is_file()
            sidecar.rmdir()
        foreign = home / "notices/backend/unrelated"
        foreign.write_text("foreign")
        action("rollback", False)
        assert foreign.read_text() == "foreign" and doc.is_file()
        foreign.unlink()
        original_doc = root / "original-document"
        doc.rename(original_doc)
        shutil.copyfile(original_doc, doc)
        action("rollback", False)
        assert home.is_dir() and doc.is_file()
        doc.unlink()
        original_doc.rename(doc)
        with sqlite3.connect(catalog) as database:
            database.execute("UPDATE Entries SET p_titles_0_nominal='changed' WHERE p_location=?", (str(doc_peer),))
        action("arm", False)
        with sqlite3.connect(catalog) as database:
            database.execute("UPDATE Entries SET p_titles_0_nominal='peer' WHERE p_location=?", (str(doc_peer),))
        aidoku_doc = str(media / "documents/Aidoku.sh")
        with sqlite3.connect(catalog) as database:
            database.execute("INSERT INTO Entries VALUES(?, 'peer', 'text/x-shellscript')", (aidoku_doc,))
        action("arm", False)
        with sqlite3.connect(catalog) as database:
            database.execute("DELETE FROM Entries WHERE p_location COLLATE BINARY=?", (aidoku_doc,))
        env["BILI_FIXTURE_ALIAS"] = "different"
        action("rollback")
        assert not home.exists() and not doc.exists()
        reset()
        env.update(BILI_FIXTURE_ALIAS="none", BILI_FIXTURE_PROBE="blind")
        action("install")
        assert proof()["doc_alias_mode"] == "absent"
        action("arm"); action("arm", False)
        def task(pid, parent, group, session, birth, args, image):
            directory = proc / str(pid)
            directory.mkdir()
            (directory / "stat").write_text(f"{pid} (test (task)) " + " ".join(
                ["S", str(parent), str(group), str(session)] + ["0"] * 15 + [str(birth)]) + "\n")
            (directory / "exe").symlink_to(image)
            (directory / "cmdline").write_bytes(b"".join(os.fsencode(value) + b"\0" for value in args))
        entry, worker, runner, app = 81001, 81002, 81003, 81004
        task(entry, 1, 123, 123, 101, ["busybox", "sh", home / "launch.sh", "--library"], system_shell)
        task(worker, entry, worker, worker, 102, ["sh", home / "launch.sh", "--detached"], system_shell)
        task(runner, worker, worker, worker, 103, [], system_shell)
        task(app, runner, worker, worker, 104, [home / "app/bilicomics", "--kindle", "--device", "scribe", "--data-dir", home / "data"],
             home / "app/bilicomics")
        lock.mkdir(mode=0o700)
        nonce = "1" * 32
        for name, value in (("nonce", nonce), ("generation", "1:" + nonce), ("lifecycle-ready", "1:" + nonce),
                            ("pid", worker), ("application-pid", app)):
            (lock / name).write_text(str(value) + "\n")
        app_dir = proc / str(app)
        (app_dir / "environ").write_bytes(("\0".join((f"BILICOMICS_LIFECYCLE_DIR={lock}", "BILICOMICS_LAUNCH_GENERATION=1",
                                                      "BILICOMICS_LAUNCH_TOKEN=" + nonce)) + "\0").encode())
        fds = app_dir / "fd"
        fds.mkdir()
        (fds / "6").symlink_to("/dev/fb0")
        source = control.read_text()
        control.write_text(source.replace("remaining=110\n", "remaining=1\n"))
        action("observe", False)
        (fds / "7").symlink_to("/dev/input/event1")
        worker_args = proc / str(worker) / "cmdline"
        worker_argv = worker_args.read_bytes()
        worker_args.write_bytes(b"busybox\0ash\0" + worker_argv.split(b"\0", 1)[1])
        action("observe", False)
        worker_args.write_bytes(b"/bin/sh\0" + worker_argv.split(b"\0", 1)[1])
        argv = (app_dir / "cmdline").read_bytes()
        (app_dir / "cmdline").write_bytes(argv + b"--demo\0")
        action("observe", False)
        (app_dir / "cmdline").write_bytes(argv)
        live_database = home / "data/state.sqlite3"
        live_database.write_bytes(b"fixture database")
        for mode in (0o755, 0o2770):
            lock.chmod(mode)
            action("observe", False)
        lock.chmod(0o700)
        os.chown(lock, 65534, -1)
        action("observe", False)
        os.chown(lock, 0, -1)
        action("observe")
        (state / "ready").unlink()
        (state / "worker").unlink()
        canonical_lock = private / lock.name
        lock.rename(canonical_lock)
        locks.rmdir()
        intermediate = root / "intermediate"
        intermediate.symlink_to(private)
        locks.symlink_to(intermediate)
        action("observe", False)
        locks.unlink()
        intermediate.unlink()
        redirected = root / "redirected"
        redirected.mkdir()
        manifest_lock = redirected / lock.name
        canonical_lock.rename(manifest_lock)
        locks.symlink_to(redirected)
        action("observe", False)
        locks.unlink()
        manifest_lock.rename(canonical_lock)
        redirected.rmdir()
        locks.symlink_to(private)
        sibling = private / "foreign-launcher.lock"
        canonical_lock.rename(sibling)
        canonical_lock.symlink_to(sibling)
        action("observe", False)
        assert not (root / "signal").exists()
        canonical_lock.unlink()
        sibling.rename(canonical_lock)
        canonical_lock.chmod(0o2700)
        action("observe")
        assert not (state / "signalled").exists()
        executable(tools / "fixture-term", f'''test "$1" = "{worker}"
test ! -e "{root}/signal"
: >"{root}/signal"
rm -r "{lock}" "{proc}/{entry}" "{proc}/{worker}" "{proc}/{runner}" "{proc}/{app}"''')
        original = (proc / str(worker) / "stat").read_text()
        (proc / str(worker) / "stat").write_text(original.replace("102\n", "999\n"))
        action("stop", False)
        assert not (root / "signal").exists()
        (proc / str(worker) / "stat").write_text(original)
        (state / "guard.pid").unlink()
        action("guard"); action("stop"); action("verify")
        parked_database = root / "parked-database"
        live_database.rename(parked_database)
        live_database.write_bytes(b"foreign replacement")
        action("rollback", False)
        assert live_database.read_bytes() == b"foreign replacement" and doc.is_file()
        live_database.unlink()
        parked_database.rename(live_database)
        action("commit"); action("commit"); action("rollback", False)
        assert doc.is_file() and (home / "app/bilicomics").is_file() and (root / "signal").exists()
    print("PASS: pre-install volume witness and delayed/persistent cache checks; direct 0700 and trusted alias 02700 locks; refused parent chains, redirected parents, symlink leaves, foreign owners and loose lock permissions; process, exit and rollback gates")


if __name__ == "__main__":
    main()

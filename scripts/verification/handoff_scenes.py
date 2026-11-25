#!/usr/bin/env python3
"""Capture the real Slint app under both Kindle design profiles on test-env.

The input schedules replay application intents. Real pointer checks are performed
separately in handoff_pointer.py. No upstream service or native device is used.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
import json
import os
from pathlib import Path
import shutil
import subprocess

from PIL import Image


PROFILES = {"pw6": (1264, 1680), "scribe": (1860, 2480)}


@dataclass
class Scene:
    name: str
    route: str
    actions: list = field(default_factory=list)
    expected: dict = field(default_factory=dict)
    profiles: tuple = tuple(PROFILES)
    offline: bool = False
    guest: bool = False


def action(section, name, value="", at=None):
    return section, name, value, at


def catalog(name, value="", at=None):
    return action("catalog", name, value, at)


def reader(name, value="", at=None):
    return action("reader", name, value, at)


def account(name, value="", at=None):
    return action("account", name, value, at)


def downloads(name, value="", at=None):
    return action("downloads", name, value, at)


def purchase(name, value="", at=None):
    return action("purchase", name, value, at)


def auth(name, value="", at=None):
    return action("auth", name, value, at)


def section(name):
    return account("section", name)


def navigate(value):
    return action("shell", "navigate", value)


def scenes(fixture):
    resume = catalog("comic.read", "28601")
    menu = reader("menu", "open", 480)
    picked = [catalog("detail.download", "enter"), catalog("episode.select", "2860109"), catalog("episode.select", "2860114")]
    extra_downloads = [catalog("comic.open", "28609"), downloads("enqueue", "2860905"),
                       catalog("comic.open", "28704"), downloads("enqueue", "2870401"), navigate("downloads")]
    cancel_tasks = [intent for row in fixture["downloads"] if row["kind"] != "done"
                    for intent in (downloads("open", str(row["comic_id"])), downloads("remove-current"))]
    remove = [row["comic_id"] for row in fixture["downloads"]]
    ids = fixture["categories"]
    genre = f"demo:{ids.index('悬疑')}"
    queued_chapter = [catalog("episode.enqueue", "2860114")]
    recent_submit = [catalog("search.history", "猫")]
    shelf_size = sum(row.get("shelf_order") is not None for row in fixture["comics"])
    fill_second_shelf_page = [catalog("detail.favorite", str(row["id"])) for row in fixture["comics"]
                              if row.get("shelf_order") is None][:max(0, 20 - shelf_size)]
    return [
        Scene("startup", "shelf"), Scene("startup-error", "shelf"),
        Scene("welcome", "welcome", expected={"ui.auth.phase": "waiting", "ui.auth.step": 1}),
        Scene("welcome-prefs-skip", "welcome", [auth("skip")], {"ui.auth.step": 2}),
        Scene("login", "login", expected={"ui.auth.phase": "waiting"}),
        Scene("shelf", "shelf", expected={"ui.catalog.shelf_state": "ready"}, offline=True),
        Scene("shelf-filter", "shelf", [catalog("shelf.menu", "open")], {"ui.catalog.shelf_menu": True}, offline=True),
        Scene("shelf-list", "shelf", [catalog("shelf.view", "list")], {"ui.catalog.shelf_view": "list"}),
        Scene("shelf-p2", "shelf", [catalog("shelf.page", "1")], {"ui.catalog.shelf_page": 2}, profiles=("pw6",)),
        Scene("shelf-p2", "shelf", fill_second_shelf_page + [catalog("shelf.page", "1", 3900)],
              {"ui.catalog.shelf_page": 2, "ui.catalog.shelf_pages": 2, "ui.toast": ""}, profiles=("scribe",)),
        Scene("shelf-return-from-reader", "shelf", [resume, reader("back", "", 480)], {"route": "shelf"}),
        Scene("shelf-upd", "shelf", [catalog("shelf.filter", "updated")], {"ui.catalog.shelf_filter": "updated"}),
        Scene("shelf-guest", "shelf", expected={"ui.catalog.shelf_state": "signed-out"}, offline=True, guest=True),
        Scene("store", "store", offline=True),
        Scene("store-online", "store", expected={"ui.online": True}),
        Scene("store-p2", "store", [catalog("store.page", "1")], {"ui.catalog.store_page": 2}),
        Scene("store-p3", "store", [catalog("store.page", "1"), catalog("store.page", "1")],
              {"ui.catalog.store_page": 3}, profiles=("pw6",)),
        Scene("store-genre", "store", [catalog("store.picker", "open")], {"ui.catalog.category_picker": True}, profiles=("pw6",)),
        Scene("store-genre-picked", "store", [catalog("store.category", genre)], {"ui.catalog.store_page": 1}),
        Scene("network", "store", [catalog("comic.open", "28703")], {"ui.auth.net_visible": True, "ui.auth.net_phase": "idle"}, offline=True),
        Scene("network-preview", "store", [catalog("comic.open", "28703"), auth("net-connect")], {"ui.auth.net_visible": True}, offline=True),
        Scene("search", "search", expected={"ui.catalog.keyboard": True}),
        Scene("search-typing", "search", [catalog("search.key", key) for key in "mao"], {"ui.catalog.composition": "mao"}),
        Scene("search-numeric", "search", [catalog("search.by-id"), catalog("search.key", "2"), catalog("search.key", "8")],
              {"ui.catalog.numeric": True}),
        Scene("search-results", "search", recent_submit, {"ui.catalog.keyboard": False}),
        Scene("reader-restart-completed-search", "search", recent_submit + [catalog("comic.read", "28604")],
              {"route": "reader", "reader.episode_title": "第1话 风从海上来", "ui.reader.page": 1}),
        Scene("search-empty", "search", [catalog("search.history", "星际")], {"ui.catalog.keyboard": False, "ui.catalog.search_results": 0}),
        Scene("detail", "detail", expected={"ui.catalog.detail_id": "28601"}, offline=True),
        Scene("detail-desc", "detail", [catalog("episode.sort")], {"ui.catalog.episode_page": 1, "ui.catalog.locate_visible": True}),
        Scene("detail-locate-desc", "detail", [catalog("episode.sort"), catalog("detail.locate")],
              {"ui.catalog.episode_descending": True, "ui.catalog.locate_visible": False}),
        Scene("detail-dl", "detail", [catalog("episode.filter", "downloaded")], {"ui.catalog.episode_filter": "downloaded", "ui.catalog.episode_pages": 1}),
        Scene("detail-other", "store", [catalog("comic.open", "28704")], {"route": "detail", "ui.catalog.detail_id": "28704"}),
        Scene("detail-other2", "shelf", [catalog("comic.open", "28602")], {"route": "detail", "ui.catalog.detail_id": "28602"}),
        Scene("detail-unfollowed", "detail", [catalog("detail.favorite", "28601")], {"ui.catalog.detail_id": "28601"}),
        Scene("detail-queued", "detail", queued_chapter),
        Scene("detail-sel", "detail", picked, {"ui.catalog.download_selecting": True, "ui.catalog.selection_count": 2}),
        Scene("detail-sel0", "detail", picked[:1], {"ui.catalog.download_selecting": True, "ui.catalog.selection_count": 0}),
        Scene("detail-sel-all", "detail", picked[:1] + [catalog("detail.select-page")], {"ui.catalog.page_selected": True}),
        Scene("detail-sel-dl", "detail", picked[:1] + [catalog("episode.filter", "downloaded")], {"ui.catalog.can_select_page": False}),
        Scene("directory", "shelf", [resume, reader("sheet", "toc", 480)], {"ui.reader.sheet": "toc", "ui.reader.page": 18}),
        Scene("directory-unread", "shelf", [resume, reader("sheet", "toc", 480), catalog("episode.filter", "unread")],
              {"ui.reader.sheet": "toc", "ui.reader.chapter_filter": "unread", "ui.catalog.locate_visible": False}),
        Scene("reader", "reader", expected={"ui.reader.page": 18, "ui.reader.menu": False}, offline=True),
        Scene("reader-fast", "reader", [reader("setting", "refresh-mode:fast", 420)], {"ui.reader.presented_fast_mode": True}),
        Scene("reader-panel", "reader", [reader("setting", "mode:panel", 420), reader("turn", "next", 2050)], {"ui.reader.mode": "panel", "ui.reader.panel": 2}),
        Scene("reader-original", "reader", [reader("setting", "fit:original", 420)], {"ui.reader.fit": "original"}),
        Scene("reader-menu", "shelf", [resume, menu], {"ui.reader.menu": True, "ui.reader.download_state": "downloaded"}, offline=True),
        Scene("reader-menu-fromdetail", "reader", [menu], {"ui.reader.menu": True}),
        Scene("reader-menu-rtl", "shelf", [resume, reader("setting", "direction:rtl", 400), reader("menu", "open", 700)], {"ui.reader.direction": "rtl", "ui.reader.menu": True}),
        Scene("search-return-from-reader", "search", [catalog("search.history", "山海"), resume, reader("back", "", 720)],
              {"route": "search", "ui.catalog.query": "山海", "ui.catalog.keyboard": False}),
        Scene("directory-return-from-purchase", "reader", [reader("sheet", "toc", 480), catalog("episode.open", "2860113"), purchase("back")],
              {"route": "reader", "ui.reader.sheet": "toc", "ui.reader.page": 18}),
        Scene("reader-menu-other", "downloads", [downloads("open", "28602"), downloads("read"), menu], {"route": "reader", "ui.reader.menu": True}),
        Scene("reader-light", "shelf", [resume, reader("sheet", "light", 480)], {"ui.reader.sheet": "light"}),
        Scene("light-extreme", "reader", [reader("sheet", "light", 400)] + [reader("brightness", "+1") for _ in range(24)] + [reader("warmth", "-1") for _ in range(24)],
              {"ui.reader.sheet": "light"}),
        Scene("reader-layout", "shelf", [resume, reader("sheet", "display", 480)], {"ui.reader.sheet": "display"}, profiles=("pw6",)),
        Scene("reader-settings", "shelf", [resume, reader("sheet", "display", 480)], {"ui.reader.sheet": "display"}, profiles=("scribe",)),
        Scene("reader-settings-fast", "shelf", [resume, reader("setting", "refresh-mode:fast", 480), reader("sheet", "display", 720)],
              {"ui.reader.refresh_mode": "fast", "ui.reader.sheet": "display"}, profiles=("scribe",)),
        Scene("purchase", "purchase", expected={"ui.purchase.included": 1, "ui.purchase.phase": "review"}),
        Scene("purchase-b5", "purchase", [purchase("range", "batch:1")], {"ui.purchase.included": 5}),
        Scene("purchase-coupon", "purchase", [purchase("payment", "coupon")], {"ui.purchase.payment": "coupon"}),
        Scene("purchase-late", "detail", [catalog("episode.open", "2860128")], {"route": "purchase"}),
        Scene("purchase-preview", "purchase", [purchase("submit")], {"ui.purchase.phase": "done", "ui.purchase.can_read": False}),
        Scene("downloads", "downloads"),
        Scene("downloads-paused", "downloads", [downloads("toggle-all")], {"ui.downloads.all_paused": True}),
        Scene("downloads-done-only", "downloads", cancel_tasks, {"ui.download_badge": 0}),
        Scene("downloads-retried", "downloads", [downloads("retry-item", "28607")], {"ui.download_badge": 3}),
        Scene("downloads-queued", "downloads", extra_downloads, {"ui.downloads.page": 1, "ui.downloads.page_count": 2}),
        Scene("downloads-queued-p2", "downloads", extra_downloads + [downloads("page", "1")], {"ui.downloads.page": 2}),
        Scene("downloads-empty", "downloads", [downloads("select")] + [downloads("toggle-item", str(id_)) for id_ in remove] + [downloads("remove-selected")], {"ui.downloads.items": []}),
        Scene("downloads-sel0", "downloads", [downloads("select")], {"ui.downloads.selecting": True, "ui.downloads.selected_count": 0}),
        Scene("downloads-sel", "downloads", [downloads("select"), downloads("toggle-item", "28604"), downloads("toggle-item", "28605")], {"ui.downloads.selected_count": 2}),
        Scene("downloads-removed", "downloads", [downloads("select"), downloads("toggle-item", "28604"), downloads("toggle-item", "28605"), downloads("remove-selected")], {"ui.downloads.selecting": False}),
        Scene("downloads-sheet", "downloads", [downloads("open", "28601")], {"ui.downloads.sheet_id": "28601", "ui.downloads.can_read": True}),
        Scene("downloads-sheet-task", "downloads", [downloads("open", "28607")], {"ui.downloads.task_primary_action": "retry"}),
        Scene("downloads-sheet-active", "downloads", [downloads("open", "28602")], {"ui.downloads.sheet_open": True}),
        Scene("downloads-sheet-queued", "detail", [catalog("episode.enqueue", "2860109"), catalog("episode.enqueue", "2860114"),
              navigate("downloads"), downloads("open", "28601")],
              {"route": "downloads", "ui.downloads.task_primary_action": "priority", "ui.downloads.can_read": True}),
        Scene("downloads-cancel-queue-keeps-chapters", "detail", [catalog("episode.enqueue", "2860109"), navigate("downloads"),
              downloads("open", "28601"), downloads("task", "cancel-queued"), downloads("open", "28601")],
              {"route": "downloads", "ui.downloads.can_read": True, "ui.downloads.sheet_id": "28601"}),
        Scene("downloads-sheet-p2", "downloads", [downloads("open", "28604"), downloads("chapter-page", "1")], {"ui.downloads.chapter_page": 2}),
        Scene("account", "account", offline=True),
        Scene("account-online", "account", expected={"ui.online": True}, profiles=("pw6",)),
        Scene("account-refreshed", "account", [account("refresh-wallet")]),
        Scene("account-signedout", "account", offline=True, guest=True),
        Scene("account-purchases-empty", "account", [section("account"), account("purchase-history")], {"ui.account.section": "purchases"}),
        Scene("account-recharges-empty", "account", [section("account"), account("recharge-history")], {"ui.account.section": "recharges"}),
        Scene("acct-reading", "account", [section("reading")], {"ui.account.section": "reading"}, profiles=("pw6",)),
        Scene("acct-display", "account", [section("display")], {"ui.account.section": "display"}, profiles=("pw6",)),
        Scene("acct-display-fast", "account", [account("refresh-mode", "fast"), section("display")], {"ui.account.refresh_mode": "fast"}),
        Scene("acct-download", "account", [section("download")], {"ui.account.section": "download"}),
        Scene("acct-storage", "account", [section("storage")], {"ui.account.section": "storage"}),
        Scene("acct-storage-clean", "account", [section("storage"), account("clear-cache")], {"ui.account.section": "storage"}, profiles=("pw6",)),
        Scene("acct-system", "account", [section("system")], {"ui.account.section": "system"}),
        Scene("acct-account", "account", [section("account")], {"ui.account.section": "account"}),
        Scene("acct-about", "account", [section("about")], {"ui.account.section": "about"}),
        Scene("acct-about-acted", "account", [section("about"), account("export-diagnostics")], {"ui.account.section": "about"}),
    ]


def verdict_value(obj, key):
    for part in key.split("."):
        obj = obj.get(part) if isinstance(obj, dict) else None
    return obj


def run_scene(args, profile, scene):
    temp = args.output / "work" / profile / scene.name
    raw = args.output / "raw" / profile
    temp.mkdir(parents=True)
    raw.mkdir(parents=True, exist_ok=True)
    picture = temp / "window.png"
    target = raw / (scene.name + ".png")
    data = temp / "data"
    command = ["xvfb-run", "-a", "-s", "-screen 0 1940x2520x24 -nolisten tcp", str(args.binary),
               "--device", profile, "--screen", scene.route, "--data-dir", str(data), "--capture", str(picture)]
    expected_status = 0
    if not scene.guest:
        command += ["--demo"]
    if scene.guest or scene.offline:
        command += ["--offline"]
    if scene.name == "startup-error":
        expected_status = 1
        data.write_text("Controlled invalid data-directory path\n")
    elif scene.name == "startup":
        command += ["--startup-delay-ms", "1800", "--capture-startup", str(temp / "startup.png")]
    at = 0
    timed = []
    for module, name, value, when in scene.actions:
        at = max(at + 70, when or 0)
        timed.append(dict(at_ms=at, section=module, action=name, value=value))
    if timed:
        action_path = temp / "actions.json"
        action_path.write_text(json.dumps(timed, ensure_ascii=False))
        command += ["--actions", str(action_path)]
    reader_scene = scene.route == "reader" or scene.name.startswith(("reader-", "directory"))
    settle = 3800 if reader_scene else 3200 if scene.name in {
        "detail-unfollowed", "detail-queued", "downloads-queued", "downloads-queued-p2", "downloads-sheet-queued"
    } else 800
    command += ["--exit-after-ms", str(max(900, at + settle))]
    env = {key: val for key, val in os.environ.items() if not key.startswith(("BILICOMICS_", "SLINT_"))}
    env.update(SLINT_BACKEND="winit-software", SLINT_SCALE_FACTOR="2")
    result = {"profile": profile, "scene": scene.name, "scope": "application-intent replay", "ready": False}
    with (temp / "app.log").open("w") as log:
        finished = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=log, stderr=log, env=env, timeout=45)
    if scene.name == "startup":
        picture = temp / "startup-0.png"
    metadata = picture.with_suffix(".json")
    if picture.is_file():
        shutil.copyfile(picture, target)
        if metadata.is_file():
            shutil.copyfile(metadata, target.with_suffix(".json"))
        with Image.open(picture) as png:
            result["size"] = png.size
    checks = dict(scene.expected)
    if not scene.name.startswith("startup"):
        checks.setdefault("route", "reader" if reader_scene else scene.route)
        if checks["route"] == "reader":
            checks.update({"ui.reader.loading": False})
        checks.update(demo=not scene.guest, backend="desktop-software", **{"startup.booting": False})
    if scene.name == "startup-error":
        checks.update({"startup.error": True})
    state = json.loads(metadata.read_text()) if metadata.is_file() else {}
    result["failures"] = {key: {"expected": value, "actual": verdict_value(state, key)}
                          for key, value in checks.items() if verdict_value(state, key) != value}
    result["exit_status"] = finished.returncode
    result["ready"] = (result.get("size") == PROFILES[profile] and finished.returncode == expected_status and not result["failures"])
    print(f"{profile}/{scene.name}: {'captured' if result['ready'] else result}", flush=True)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--base", required=True, help="Base git revision of the working-tree snapshot")
    parser.add_argument("--workers", type=int, choices=(1, 2), default=2)
    parser.add_argument("--only", nargs="+", help="Restrict to exact scene names")
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.output = args.output.resolve()
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    fixture = json.loads((Path(__file__).resolve().parents[2] / "assets/demo/catalog.json").read_text())
    work = [(profile, scene) for scene in scenes(fixture) for profile in scene.profiles
            if not args.only or scene.name in args.only]
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        results = list(pool.map(lambda task: run_scene(args, *task), work))
    report = {"base_revision": args.base, "source": "working-tree snapshot", "backend": "Slint winit software under Xvfb",
              "scale": 2, "physical_device": False, "live_account": False, "results": results}
    (args.output / "capture-report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    return int(not results or not all(row["ready"] for row in results))


if __name__ == "__main__":
    raise SystemExit(main())

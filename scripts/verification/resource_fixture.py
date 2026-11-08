"""Seed isolated resource cases with the existing complete guest page fixture."""

import hashlib
import importlib.util
import json
import shutil
import sys


COVER_URL = "https://resource-fixture.invalid/cover.png"


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def file_identity(path):
    return {"path": str(path), "bytes": path.stat().st_size, "sha256": sha256(path)}


def load_guest_fixture(app_root):
    path = app_root / "scripts/verification/reader_guest_fixtures.py"
    require(path.is_file() and not path.is_symlink(), "The selected guest fixture source is unavailable")
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location("resource_guest_fixture", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module, path


def select_cover_asset(app_root, selected):
    if selected is not None:
        require(not selected.is_symlink(), "The selected cover asset cannot be a symlink")
        path = selected.resolve(strict=True)
    else:
        directory = app_root / "assets/demo/covers"
        preferred = directory / "cover-28601.png"
        candidates = [preferred] if preferred.is_file() else sorted(directory.glob("*.png"))
        candidates = [path for path in candidates if path.is_file() and not path.is_symlink()]
        require(candidates, "No fictional local cover asset is available")
        path = candidates[0].resolve(strict=True)
    require(path.is_relative_to(app_root.resolve()), "The cover must come from the selected application assets")
    require(path.is_relative_to((app_root / "assets/demo/covers").resolve()),
            "Only isolated fictional demo covers may be used")
    require(path.is_file() and not path.is_symlink(), "The cover asset must be a regular file")
    require(path.stat().st_size <= 8 * 1024 * 1024, "The local cover exceeds the production disk-cache limit")
    return path


def seed_case(guest, directory, assets_root, origin_page, cover_asset, cached):
    data, database, seed = guest.seed_data(directory, "resource-ready", assets_root, origin_page)
    cover_name = hashlib.sha256(COVER_URL.encode()).hexdigest() + ".png"
    cover_directory = data / "covers"
    cover_directory.mkdir(mode=0o700)
    cover_path = cover_directory / cover_name
    with guest.sql(database) as connection:
        comic_row = connection.execute(
            "SELECT payload FROM comics WHERE account_key=? AND comic_id=?", (guest.ACCOUNT, guest.COMIC)
        ).fetchone()
        detail_row = connection.execute(
            "SELECT payload FROM comic_details WHERE account_key=? AND comic_id=?", (guest.ACCOUNT, guest.COMIC)
        ).fetchone()
        require(comic_row and detail_row, "The complete guest fixture has no local comic")
        comic, detail = json.loads(comic_row[0]), json.loads(detail_row[0])
        comic["cover_url"] = COVER_URL
        detail["comic"]["cover_url"] = COVER_URL
        connection.execute("UPDATE comics SET payload=? WHERE account_key=? AND comic_id=?",
                           (json.dumps(comic), guest.ACCOUNT, guest.COMIC))
        connection.execute("UPDATE comic_details SET payload=? WHERE account_key=? AND comic_id=?",
                           (json.dumps(detail), guest.ACCOUNT, guest.COMIC))
    if cached:
        shutil.copyfile(cover_asset, cover_path)
        cover_path.chmod(0o600)
        require(sha256(cover_path) == sha256(cover_asset), "The cover seed differs from the selected asset")
    guest.check_isolation(data, database)
    proof = {
        "case": "cached-cover" if cached else "missing-cover",
        "account_key": guest.ACCOUNT,
        "comic_id": guest.COMIC,
        "episode_id": guest.EPISODE,
        "revision": guest.REVISION,
        "origin_manifest_page_number": origin_page,
        "page_sha256": seed["origin_sha256"],
        "page_bytes": seed["origin_bytes"],
        "page_dimensions": seed["origin_dimensions"],
        "page_count": 1,
        "cover_url_kind": "Reserved .invalid fixture URL; no account or service input",
        "cover_cache_name": cover_name,
        "cover_file_seeded": cached,
        "cover_asset": file_identity(cover_asset),
        "cover_cached_bytes": cover_path.stat().st_size if cached else 0,
        "cover_cached_sha256": sha256(cover_path) if cached else None,
        "session_seeded": False,
        "credentials_seeded": False,
        "source_success_simulated": False,
        "page_fixture_origin": "Existing reader_guest_fixtures.seed_data without a missing/corrupt fault",
        "cover_runtime_publication_observed": False,
    }
    return data, database, seed, proof


def complete_page_proof(guest, data, database, seed):
    observed = guest.loading_validation_state(data, database, seed)
    receipt = observed["receipt"]
    return {
        "schema_version": observed["schema_version"],
        "page_id": receipt["page_id"],
        "sha256": receipt["sha256"],
        "bytes": receipt["bytes"],
        "width": receipt["width"],
        "height": receipt["height"],
        "decode_validation_version": receipt["decode_validation_version"],
        "identity_history": observed["identity_history"],
        "proof_origin": observed["proof_origin"],
    }

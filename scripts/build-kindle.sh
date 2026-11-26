#!/bin/sh
set -eu

ROOT=${1:?Supply the isolated remote root}
shift
case "$#:${1-}" in
    0:) MODE=manifest ;;
    1:--byte-verified) MODE=byte-verified ;;
    *) printf 'Usage: %s ROOT [--byte-verified]\n' "$0" >&2; exit 64 ;;
esac
if [ -f /root/.cargo/env ]; then . /root/.cargo/env; fi
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-12}"
cd "$ROOT/app"
VERSION=$(python3 -c 'import pathlib,tomllib; print(tomllib.loads(pathlib.Path("Cargo.toml").read_text())["package"]["version"])')
case "$VERSION" in *[!A-Za-z0-9._-]*|'') printf '%s\n' 'Invalid package version.' >&2; exit 1 ;; esac
STAGING="$ROOT/kindle-package"
RELEASE="$CARGO_TARGET_DIR/armv7-unknown-linux-musleabihf/release"
ARCHIVE="$ROOT/app/artifacts/verification/bilicomics-kindle-$VERSION-dev.zip"
BYTE_RECEIPT="${ARCHIVE%.zip}.byte-verification.json"
mkdir -p artifacts/verification
if [ "$MODE" = byte-verified ]; then
    # Do not let a previous build's staging or sidecar describe this archive.
    for path in "$ARCHIVE" "${ARCHIVE%.zip}.manifest.json" "$BYTE_RECEIPT"; do
        if [ -e "$path" ] || [ -L "$path" ]; then
            printf 'Package output already exists: %s\n' "$path" >&2
            exit 1
        fi
    done
    mkdir "$STAGING"
fi
cargo zigbuild --locked --offline --release --target armv7-unknown-linux-musleabihf \
    --no-default-features --features kindle --bins > artifacts/verification/kindle-build.log 2>&1 || {
    cat artifacts/verification/kindle-build.log; exit 1;
}
mkdir -p "$STAGING/bilicomics/app" "$STAGING/bilicomics/assets" "$STAGING/bilicomics/data" "$STAGING/documents"
mkdir -p "$STAGING/notices/backend" "$STAGING/notices/fonts" "$STAGING/notices/pinyin" "$STAGING/notices/protocol"
mkdir -p "$STAGING/notices/jpeg"
mkdir -p "$STAGING/notices/image-triangle"
mkdir -p "$STAGING/notices/aidoku-native-media"
mkdir -p "$STAGING/notices/spatial-dither"
mkdir -p "$STAGING/notices/aidoku-startup"
mkdir -p "$STAGING/notices/aidoku-startup-sequence"
mkdir -p "$STAGING/notices/aidoku-native-library-entry"
cp "$RELEASE/bilicomics" "$STAGING/bilicomics/app/bilicomics"
cp "$RELEASE/kindle-jpeg-worker" "$STAGING/bilicomics/app/kindle-jpeg-worker"
cp packaging/kindle/launch.sh "$STAGING/bilicomics/launch.sh"
install -m 644 packaging/kindle/bilicomics-library-cover.png "$STAGING/bilicomics/assets/bilicomics-library-cover.png"
cp packaging/kindle/document-launcher.sh "$STAGING/documents/bilicomics.sh"
cp packaging/kindle/device-allowlist.example "$STAGING/bilicomics/data/device-allowlist"
printf '%s\n' pw5 > "$STAGING/bilicomics/data/device-profile"
cp packaging/kindle/README.md "$STAGING/README.md"
cp THIRD_PARTY_NOTICES.md "$STAGING/THIRD_PARTY_NOTICES.md"
cp Cargo.lock "$STAGING/notices/Cargo.lock"
cp vendor/slint-backend-kindle/LICENSE-MIT vendor/slint-backend-kindle/LICENSE-APACHE \
    vendor/slint-backend-kindle/UPSTREAM_REVISION vendor/slint-backend-kindle/PATCHES.md "$STAGING/notices/backend/"
cp assets/fonts/OFL.txt assets/fonts/NotoSansCJK-LICENSE.txt assets/fonts/NotoSerifCJK-LICENSE.txt assets/fonts/NOTICES.txt "$STAGING/notices/fonts/"
cp assets/fonts/startup-manifest.json "$STAGING/notices/fonts/startup-manifest.json"
cp assets/pinyin/LICENSE-AOSP.txt assets/pinyin/README.md "$STAGING/notices/pinyin/"
cp vendor/bili-protocol/README.md "$STAGING/notices/protocol/README.md"
cp vendor/bili-protocol/wasm3/LICENSE "$STAGING/notices/protocol/wasm3-LICENSE"
cp vendor/bili-protocol/cjson/LICENSE "$STAGING/notices/protocol/cjson-LICENSE"
cp vendor/bili-protocol/legacy_v5/vendor/mbedtls/LICENSE "$STAGING/notices/protocol/mbedtls-LICENSE"
cp vendor/zune-jpeg/LICENSE-APACHE vendor/zune-jpeg/LICENSE-MIT vendor/zune-jpeg/LICENSE-ZLIB \
    vendor/zune-jpeg/UPSTREAM.md vendor/zune-jpeg/PATCHES.md "$STAGING/notices/jpeg/"
cp vendor/zune-jpeg/patches/0001-strict-entropy-completeness.patch "$STAGING/notices/jpeg/"
cp vendor/image-triangle-reference-0.25.10/LICENSE-MIT vendor/image-triangle-reference-0.25.10/LICENSE-APACHE \
    vendor/image-triangle-reference-0.25.10/README.md vendor/image-triangle-reference-0.25.10/SHA256SUMS "$STAGING/notices/image-triangle/"
cp vendor/aidoku-native-media/LICENSE-GPL-3.0-or-later vendor/aidoku-native-media/README.md \
    vendor/aidoku-native-media/UPSTREAM_FILES.json "$STAGING/notices/aidoku-native-media/"
# Retain original source/license records without staging research images or code.
cp docs/research/aidoku-spatial-dither-20261116/LICENSE.AIDOKU \
    docs/research/aidoku-spatial-dither-20261116/README.md \
    docs/research/aidoku-spatial-dither-20261116/UPSTREAM_FILES.json "$STAGING/notices/spatial-dither/"
cp docs/research/aidoku-startup-20261117/LICENSE.AIDOKU \
    docs/research/aidoku-startup-20261117/README.md \
    docs/research/aidoku-startup-20261117/UPSTREAM_FILES.json "$STAGING/notices/aidoku-startup/"
cp docs/research/aidoku-startup-sequence-20261117/LICENSE.AIDOKU \
    docs/research/aidoku-startup-sequence-20261117/README.md \
    docs/research/aidoku-startup-sequence-20261117/UPSTREAM_FILES.json "$STAGING/notices/aidoku-startup-sequence/"
cp docs/research/aidoku-native-library-entry-20261117/LICENSE.AIDOKU \
    docs/research/aidoku-native-library-entry-20261117/README.md \
    docs/research/aidoku-native-library-entry-20261117/UPSTREAM_FILES.json "$STAGING/notices/aidoku-native-library-entry/"
chmod 755 "$STAGING/bilicomics/app/bilicomics" "$STAGING/bilicomics/app/kindle-jpeg-worker" "$STAGING/bilicomics/launch.sh" \
    "$STAGING/documents/bilicomics.sh"
PYTHONDONTWRITEBYTECODE=1 python3 - "$STAGING" "$ARCHIVE" "$MODE" <<'PY'
import json
import pathlib
import sys
import zipfile

sys.path.insert(0, "scripts/verification")
from verify_package import ALLOWED_FILES, FIXED_TIMESTAMP

root = pathlib.Path(sys.argv[1])
output = pathlib.Path(sys.argv[2])
write_manifest = sys.argv[3] == "manifest"
if write_manifest:
    import hashlib
    manifest = []

with zipfile.ZipFile(output, "w" if write_manifest else "x", zipfile.ZIP_DEFLATED) as archive:
    for relative in sorted(ALLOWED_FILES):
        path = root / relative
        if not path.is_file() or path.is_symlink():
            raise RuntimeError(f"Missing or unsafe package input: {relative}")
        path.resolve().relative_to(root.resolve())
        data = path.read_bytes()
        if relative in ("bilicomics/launch.sh", "documents/bilicomics.sh"):
            data = data.replace(b"\r\n", b"\n")
            if not data.startswith(b"#!/bin/sh\n") or not data.endswith(b"\n") or b"\r" in data or b"\0" in data:
                raise RuntimeError(f"Invalid POSIX shell package input: {relative}")
        info = zipfile.ZipInfo(relative, date_time=FIXED_TIMESTAMP)
        info.compress_type = zipfile.ZIP_DEFLATED
        info.external_attr = (path.stat().st_mode & 0xFFFF) << 16
        archive.writestr(info, data)
        if write_manifest:
            manifest.append({"path": relative, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
if write_manifest:
    output.with_suffix(".manifest.json").write_text(json.dumps({"development_candidate": True, "device_verified": False, "files": manifest}, indent=2) + "\n")
PY
if [ "$MODE" = byte-verified ]; then
    python3 -B scripts/verification/verify_dev_payload.py --archive "$ARCHIVE" \
        --app-root "$ROOT/app" --staging "$STAGING" --release-dir "$RELEASE" --output "$BYTE_RECEIPT" || {
        rm -f "$ARCHIVE"
        exit 1
    }
fi

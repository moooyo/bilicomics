#!/bin/sh
set -eu

ROOT=${1:?Supply the isolated remote root}
if [ -f /root/.cargo/env ]; then . /root/.cargo/env; fi
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-12}"
cd "$ROOT/app"
VERSION=$(python3 -c 'import pathlib,tomllib; print(tomllib.loads(pathlib.Path("Cargo.toml").read_text())["package"]["version"])')
case "$VERSION" in *[!A-Za-z0-9._-]*|'') printf '%s\n' 'Invalid package version.' >&2; exit 1 ;; esac
mkdir -p artifacts/verification
cargo zigbuild --locked --offline --release --target armv7-unknown-linux-musleabihf \
    --no-default-features --features kindle --bins > artifacts/verification/kindle-build.log 2>&1 || {
    cat artifacts/verification/kindle-build.log; exit 1;
}
STAGING="$ROOT/kindle-package"
mkdir -p "$STAGING/bilicomics/app" "$STAGING/bilicomics/data" "$STAGING/documents"
mkdir -p "$STAGING/notices/backend" "$STAGING/notices/fonts" "$STAGING/notices/pinyin" "$STAGING/notices/protocol"
mkdir -p "$STAGING/notices/jpeg"
mkdir -p "$STAGING/notices/image-triangle"
mkdir -p "$STAGING/notices/aidoku-native-media"
mkdir -p "$STAGING/notices/spatial-dither"
mkdir -p "$STAGING/notices/aidoku-startup"
mkdir -p "$STAGING/notices/aidoku-startup-sequence"
mkdir -p "$STAGING/notices/aidoku-native-library-entry"
cp "$CARGO_TARGET_DIR/armv7-unknown-linux-musleabihf/release/bilicomics" "$STAGING/bilicomics/app/bilicomics"
cp "$CARGO_TARGET_DIR/armv7-unknown-linux-musleabihf/release/kindle-jpeg-worker" "$STAGING/bilicomics/app/kindle-jpeg-worker"
cp packaging/kindle/launch.sh "$STAGING/bilicomics/launch.sh"
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
python3 - "$STAGING" "$ROOT/app/artifacts/verification/bilicomics-kindle-$VERSION-dev.zip" <<'PY'
import hashlib
import json
import pathlib
import sys
import zipfile

root = pathlib.Path(sys.argv[1])
output = pathlib.Path(sys.argv[2])
manifest = []
allowed = [
    "README.md", "THIRD_PARTY_NOTICES.md", "bilicomics/app/bilicomics", "bilicomics/app/kindle-jpeg-worker",
    "bilicomics/launch.sh", "documents/bilicomics.sh", "bilicomics/data/device-allowlist", "bilicomics/data/device-profile",
    "notices/Cargo.lock", "notices/backend/LICENSE-MIT", "notices/backend/LICENSE-APACHE",
    "notices/backend/UPSTREAM_REVISION", "notices/backend/PATCHES.md",
    "notices/fonts/OFL.txt", "notices/fonts/NotoSansCJK-LICENSE.txt",
    "notices/fonts/NotoSerifCJK-LICENSE.txt", "notices/fonts/NOTICES.txt",
    "notices/fonts/startup-manifest.json",
    "notices/pinyin/LICENSE-AOSP.txt", "notices/pinyin/README.md",
    "notices/protocol/README.md", "notices/protocol/wasm3-LICENSE",
    "notices/protocol/cjson-LICENSE", "notices/protocol/mbedtls-LICENSE",
    "notices/jpeg/LICENSE-APACHE", "notices/jpeg/LICENSE-MIT", "notices/jpeg/LICENSE-ZLIB",
    "notices/jpeg/UPSTREAM.md", "notices/jpeg/PATCHES.md", "notices/jpeg/0001-strict-entropy-completeness.patch",
    "notices/image-triangle/LICENSE-MIT", "notices/image-triangle/LICENSE-APACHE",
    "notices/image-triangle/README.md", "notices/image-triangle/SHA256SUMS",
    "notices/aidoku-native-media/LICENSE-GPL-3.0-or-later", "notices/aidoku-native-media/README.md",
    "notices/aidoku-native-media/UPSTREAM_FILES.json",
    "notices/spatial-dither/LICENSE.AIDOKU", "notices/spatial-dither/README.md",
    "notices/spatial-dither/UPSTREAM_FILES.json",
    "notices/aidoku-startup/LICENSE.AIDOKU", "notices/aidoku-startup/README.md",
    "notices/aidoku-startup/UPSTREAM_FILES.json",
    "notices/aidoku-startup-sequence/LICENSE.AIDOKU", "notices/aidoku-startup-sequence/README.md",
    "notices/aidoku-startup-sequence/UPSTREAM_FILES.json",
    "notices/aidoku-native-library-entry/LICENSE.AIDOKU", "notices/aidoku-native-library-entry/README.md",
    "notices/aidoku-native-library-entry/UPSTREAM_FILES.json",
]
with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
    for relative in sorted(allowed):
        path = root / relative
        if not path.is_file() or path.is_symlink():
            raise RuntimeError(f"Missing or unsafe package input: {relative}")
        path.resolve().relative_to(root.resolve())
        data = path.read_bytes()
        if relative in ("bilicomics/launch.sh", "documents/bilicomics.sh"):
            data = data.replace(b"\r\n", b"\n")
            if not data.startswith(b"#!/bin/sh\n") or not data.endswith(b"\n") or b"\r" in data or b"\0" in data:
                raise RuntimeError(f"Invalid POSIX shell package input: {relative}")
        info = zipfile.ZipInfo(relative, date_time=(1980, 1, 1, 0, 0, 0))
        info.compress_type = zipfile.ZIP_DEFLATED
        info.external_attr = (path.stat().st_mode & 0xFFFF) << 16
        archive.writestr(info, data)
        manifest.append({"path": relative, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
output.with_suffix(".manifest.json").write_text(json.dumps({"development_candidate": True, "device_verified": False, "files": manifest}, indent=2) + "\n")
PY

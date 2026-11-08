"""Prepare the pinned bootstrap face on the designated remote worker.

Run with Python's isolated mode, for example:
    python3 -I assets/fonts/prepare_startup_font.py

The operator must not execute this script locally unless local verification is
explicitly authorized. The source, strings, generator, and tool versions are
recorded in startup-manifest.json. Existing license files are read, never edited.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import platform

import fontTools
from fontTools import subset
from fontTools.ttLib import TTFont


FAMILY = "BiliComics Startup"
STYLE = "Regular"
POSTSCRIPT_NAME = "BiliComicsStartup-Regular"
FONTTOOLS_VERSION = "4.57.0"
SOURCE_GZIP_SHA256 = "be3992b12717e7cfa6fa268e7b97eaf5ec2c2aaac95284aed42cbfce4dbc4d4a"
SOURCE_FONT_SHA256 = "cadef73c0a4c4bf8b5e6fd58638197a8736bf5b2efde5b84168119be04ff26d6"
# OpenType timestamps use seconds since 1904; this value denotes 1970-01-01 UTC.
FIXED_TIMESTAMP = 2_082_844_800
PROTECTED_NAME_IDS = {0, 7, 13, 14}
RENAMED_NAME_IDS = {1, 2, 3, 4, 6, 16, 17, 18, 21, 22}
LAYOUT_FEATURES = ["ccmp", "liga", "clig", "kern", "calt"]
LICENSE_FILES = ["OFL.txt", "NotoSansCJK-LICENSE.txt", "NOTICES.txt"]


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def protected_names(font: TTFont) -> list[tuple[int, int, int, int, bytes]]:
    return sorted(
        (name.nameID, name.platformID, name.platEncID, name.langID, name.string)
        for name in font["name"].names
        if name.nameID in PROTECTED_NAME_IDS
    )


def name_values(font: TTFont, identifier: int) -> list[str]:
    return sorted(
        {name.toUnicode() for name in font["name"].names if name.nameID == identifier}
    )


def cff_notice(font: TTFont) -> str:
    require("CFF " in font, "The pinned source must be an OpenType CFF face")
    return str(font["CFF "].cff.topDictIndex[0].Notice)


def unicode_ranges(codepoints: set[int]) -> list[str]:
    result: list[str] = []
    ordered = sorted(codepoints)
    if not ordered:
        return result
    first = previous = ordered[0]
    for codepoint in ordered[1:]:
        if codepoint == previous + 1:
            previous = codepoint
            continue
        result.append(
            f"U+{first:04X}" if first == previous else f"U+{first:04X}-{previous:04X}"
        )
        first = previous = codepoint
    result.append(
        f"U+{first:04X}" if first == previous else f"U+{first:04X}-{previous:04X}"
    )
    return result


def collect_characters(strings: bytes) -> tuple[set[int], dict[str, str], str]:
    values = json.loads(strings.decode("utf-8-sig"))
    require(isinstance(values, dict) and bool(values), "Startup strings must be a nonempty object")
    require(
        all(isinstance(key, str) and isinstance(value, str) for key, value in values.items()),
        "Every startup localization value must be text",
    )
    codepoints = set(range(0x20, 0x7F))
    for value in values.values():
        codepoints.update(ord(character) for character in value if not character.isspace())
    # ASCII spaces are already included; control characters are not font glyphs.
    require(all(codepoint >= 0x20 and codepoint != 0x7F for codepoint in codepoints),
            "Startup localization contains unsupported control characters")
    canonical = json.dumps(sorted(codepoints), separators=(",", ":")).encode("ascii")
    return codepoints, values, sha256(canonical)


def rename_face(font: TTFont, charset_hash: str) -> None:
    names = font["name"]
    names.names = [name for name in names.names if name.nameID not in RENAMED_NAME_IDS]
    replacements = {
        1: FAMILY,
        2: STYLE,
        3: f"{POSTSCRIPT_NAME};startup-v1;{charset_hash[:16]}",
        4: f"{FAMILY} {STYLE}",
        6: POSTSCRIPT_NAME,
        16: FAMILY,
        17: STYLE,
        18: f"{FAMILY} {STYLE}",
        21: FAMILY,
        22: STYLE,
    }
    for identifier, value in replacements.items():
        names.setName(value, identifier, 3, 1, 0x409)
        names.setName(value, identifier, 1, 0, 0)
    cff = font["CFF "].cff
    cff.fontNames = [POSTSCRIPT_NAME]
    top = cff.topDictIndex[0]
    top.FamilyName = FAMILY
    top.FullName = f"{FAMILY} {STYLE}"
    if hasattr(top, "FDArray"):
        for index, dictionary in enumerate(top.FDArray):
            if hasattr(dictionary, "FontName"):
                dictionary.FontName = f"{POSTSCRIPT_NAME}-FD{index}"


def make_subset(source: bytes, codepoints: set[int], charset_hash: str) -> bytes:
    font = TTFont(io.BytesIO(source), recalcTimestamp=False)
    try:
        options = subset.Options()
        options.name_IDs = ["*"]
        options.name_languages = ["*"]
        options.name_legacy = True
        options.layout_features = LAYOUT_FEATURES
        options.notdef_glyph = True
        options.notdef_outline = True
        options.recommended_glyphs = True
        options.hinting = False
        options.recalc_timestamp = False
        engine = subset.Subsetter(options=options)
        engine.populate(unicodes=codepoints)
        engine.subset(font)
        rename_face(font, charset_hash)
        font["head"].created = FIXED_TIMESTAMP
        font["head"].modified = FIXED_TIMESTAMP
        font.recalcTimestamp = False
        output = io.BytesIO()
        font.save(output, reorderTables=True)
        return output.getvalue()
    finally:
        font.close()


def prepare(source_path: Path, strings_path: Path, output_path: Path,
            manifest_path: Path, license_dir: Path) -> None:
    require(fontTools.version == FONTTOOLS_VERSION,
            f"Use fontTools {FONTTOOLS_VERSION} for this reproducible preparation")
    compressed = source_path.read_bytes()
    require(sha256(compressed) == SOURCE_GZIP_SHA256, "The compressed source font differs from the reviewed input")
    source = gzip.decompress(compressed)
    require(sha256(source) == SOURCE_FONT_SHA256, "The decompressed source font differs from the reviewed input")
    strings = strings_path.read_bytes()
    codepoints, labels, charset_hash = collect_characters(strings)
    notices = {}
    for filename in LICENSE_FILES:
        notice = (license_dir / filename).read_bytes()
        notices[filename] = {"sha256": sha256(notice), "bytes": len(notice)}

    original = TTFont(io.BytesIO(source), recalcTimestamp=False)
    try:
        source_cmap = set(original.getBestCmap())
        missing = codepoints - source_cmap
        require(not missing, "The source lacks required startup glyphs: " + ", ".join(unicode_ranges(missing)))
        source_legal_names = protected_names(original)
        source_notice = cff_notice(original)
        require(any("SIL Open Font License" in text for text in name_values(original, 13)),
                "The source font is missing its original OFL metadata")
        require(original["OS/2"].usWeightClass == 400, "The source must be the Regular weight")
        source_metadata = {
            "family": name_values(original, 16) or name_values(original, 1),
            "style": name_values(original, 17) or name_values(original, 2),
            "version": name_values(original, 5),
            "copyright": name_values(original, 0),
            "trademark": name_values(original, 7),
            "license_statement": name_values(original, 13),
            "license_url": name_values(original, 14),
            "cff_notice": source_notice,
        }
    finally:
        original.close()

    payload = make_subset(source, codepoints, charset_hash)
    repeated = make_subset(source, codepoints, charset_hash)
    require(payload == repeated, "Repeated preparation did not produce identical font bytes")
    reloaded = TTFont(io.BytesIO(payload), recalcTimestamp=False)
    try:
        require(set(reloaded.getBestCmap()) == codepoints, "The output Unicode map differs from the required charset")
        require(protected_names(reloaded) == source_legal_names, "Original copyright, trademark, or OFL records changed")
        require(cff_notice(reloaded) == source_notice, "The original CFF copyright notice changed")
        require(name_values(reloaded, 1) == [FAMILY] and name_values(reloaded, 16) == [FAMILY],
                "The bootstrap family name was not isolated from the full font family")
        require(name_values(reloaded, 6) == [POSTSCRIPT_NAME], "The bootstrap PostScript name is invalid")
        require(reloaded["OS/2"].usWeightClass == 400, "The output weight changed")
        require(reloaded["head"].created == FIXED_TIMESTAMP and reloaded["head"].modified == FIXED_TIMESTAMP,
                "The output timestamps are not deterministic")
        glyphs = reloaded["maxp"].numGlyphs
    finally:
        reloaded.close()

    manifest = {
        "format_version": 1,
        "file": output_path.name,
        "purpose": "Bootstrap font for startup status, failure, and suspend messages only",
        "family": FAMILY,
        "style": STYLE,
        "postscript_name": POSTSCRIPT_NAME,
        "weight": 400,
        "license": "OFL-1.1",
        "retained_license_files": notices,
        "fonttools_version": fontTools.version,
        "python_version": platform.python_version(),
        "generator": "assets/fonts/prepare_startup_font.py",
        "generator_sha256": sha256(Path(__file__).read_bytes()),
        "source": {
            "file": "NotoSansSC-Regular.otf.gz",
            "bytes": len(compressed),
            "sha256": sha256(compressed),
            "decompressed_bytes": len(source),
            "decompressed_sha256": sha256(source),
            "origin_kind": "mutable_local_working_tree_reference",
            "local_reference": "C:/Users/moooyo/Documents/Codex/2026-11-02/d/outputs/weread/assets/fonts/NotoSansSC-Regular.otf.gz",
            "reference_head_is_complete_snapshot": False,
            "upstream": "https://github.com/notofonts/noto-cjk/tree/Sans2.004/Sans",
            "upstream_revision": "523d033d6cb47f4a80c58a35753646f5c3608a78",
            "upstream_collection": "NotoSansCJK-Regular.ttc",
            "upstream_collection_sha256": "b76b0433203017ca80401b2ee0dd69350349871c4b19d504c34dbdd80541690a",
            "upstream_face_index": 2,
            "metadata": source_metadata,
        },
        "charset": {
            "source": "assets/i18n/startup.json",
            "source_sha256": sha256(strings),
            "source_keys": sorted(labels),
            "ascii_range": "U+0020-U+007E",
            "collection": "All non-whitespace localization characters plus printable ASCII",
            "sha256": charset_hash,
            "sha256_encoding": "ASCII JSON sorted decimal codepoints with separators comma and colon",
            "unicode_codepoints": len(codepoints),
            "codepoints": sorted(codepoints),
            "ranges": unicode_ranges(codepoints),
        },
        "transformation": {
            "hinting": False,
            "layout_features": LAYOUT_FEATURES,
            "name_languages": "all source languages; family labels replaced with English records",
            "retained_name_ids": sorted(PROTECTED_NAME_IDS),
            "head_created": FIXED_TIMESTAMP,
            "head_modified": FIXED_TIMESTAMP,
            "recalc_timestamp": False,
            "reorder_tables": True,
        },
        "output": {"bytes": len(payload), "sha256": sha256(payload), "glyphs": glyphs},
        "verification": {
            "environment": "ssh test-env",
            "repeat_generation_identical": True,
            "exact_unicode_map": True,
            "original_legal_names_unchanged": True,
            "original_cff_notice_unchanged": True,
            "device_acceptance": "Not performed by this preparation",
        },
    }
    output_path.parent.mkdir(parents=True, exist_ok=True)
    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_bytes(payload)
    manifest_path.write_text(json.dumps(manifest, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"file": output_path.name, "bytes": len(payload), "sha256": sha256(payload),
                      "family": FAMILY, "codepoints": len(codepoints), "glyphs": glyphs}, sort_keys=True))


def main() -> None:
    root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=root / "assets/fonts/NotoSansSC-Regular.otf.gz")
    parser.add_argument("--strings", type=Path, default=root / "assets/i18n/startup.json")
    parser.add_argument("--output", type=Path, default=root / "assets/fonts/StartupSansSC.otf")
    parser.add_argument("--manifest", type=Path, default=root / "assets/fonts/startup-manifest.json")
    parser.add_argument("--license-dir", type=Path, default=root / "assets/fonts")
    arguments = parser.parse_args()
    prepare(arguments.source, arguments.strings, arguments.output, arguments.manifest,
            arguments.license_dir)


if __name__ == "__main__":
    main()

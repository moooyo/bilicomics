#!/usr/bin/env python3
"""Diagnose the preserved verifier manifest ordering mismatch on test-env."""

import hashlib
import json
from pathlib import Path
import sys

ROOT = Path("/root/bilicomics-native-library-entry-20261117-01a1d11a-r1")
SOURCE = ROOT / "candidate-r1/app"
OLD = Path("/root/bilicomics-startup-sequence-20261117-01a1d11a-r1/checks-r3/source-inputs.sha256")
CURRENT = ROOT / "checks-r1/source-inputs.sha256"
sys.dont_write_bytecode = True
sys.path.insert(0, str(SOURCE / "scripts/verification"))
from native_protocol import build_input_manifest

old = OLD.read_bytes()
current = CURRENT.read_bytes()
native = build_input_manifest(SOURCE).encode("utf-8")

def pairs(value):
    return {name: digest for digest, name in (line.split("  ", 1) for line in value.decode("utf-8").splitlines())}

report = {"old_manifest_sha256": hashlib.sha256(old).hexdigest(),
          "run_suite_manifest_sha256": hashlib.sha256(current).hexdigest(),
          "native_manifest_sha256": hashlib.sha256(native).hexdigest(),
          "run_suite_raw_identical": old == current, "native_raw_identical": old == native,
          "old_and_run_suite_pairs_identical": pairs(old) == pairs(current),
          "old_and_native_pairs_identical": pairs(old) == pairs(native),
          "old_file_count": len(pairs(old)), "current_file_count": len(pairs(current)),
          "missing_paths": sorted(set(pairs(old)) - set(pairs(current))),
          "additional_paths": sorted(set(pairs(current)) - set(pairs(old))),
          "changed_paths": sorted(name for name, value in pairs(current).items()
                                  if name in pairs(old) and pairs(old)[name] != value),
          "source_changed": False, "product_changes_performed": False,
          "device_verified": False}
with (ROOT / "manifest-order-diagnosis-r1.json").open("x", encoding="utf-8") as output:
    json.dump(report, output, indent=2)
    output.write("\n")
print(json.dumps(report, indent=2))
raise SystemExit(0 if report["native_raw_identical"] and report["old_and_run_suite_pairs_identical"] else 1)

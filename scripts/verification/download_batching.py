#!/usr/bin/env python3
"""Run synthetic download batching measurements in an isolated Linux namespace."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys


TEST_NAME = "controlled_download_batching_reduces_tokens_without_changing_commits"


def sha256(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def source_manifest(source):
    files = [source / name for name in ("Cargo.toml", "Cargo.lock", "build.rs")]
    configuration = source / ".cargo/config.toml"
    if configuration.is_file():
        files.append(configuration)
    for name in ("src", "ui", "assets", "vendor", "tests", "scripts", ".cargo"):
        root = source / name
        if root.is_dir():
            files.extend(path for path in root.rglob("*") if path.is_file())
    return "".join(f"{sha256(path)}  {path.relative_to(source).as_posix()}\n"
                   for path in sorted(files))


def native_asset_identity(source, asset_root):
    client = (source / "src" / "source" / "client.rs").read_text(encoding="utf-8")
    crypto = (source / "src" / "source" / "crypto.rs").read_text(encoding="utf-8")

    def constant(text, name):
        match = re.search(rf'const {name}: &str = "([^"\n]+)";', text)
        if not match:
            raise RuntimeError(f"The source does not identify its pinned {name} constant.")
        return match.group(1)

    name = constant(client, "SIGNING_FILE")
    expected = constant(client, "SIGNING_SHA256")
    if name != constant(crypto, "SIGNING_ASSET") or Path(name).name != name:
        raise RuntimeError("The client and NativeCrypto signing asset names do not agree.")
    if not re.fullmatch(r"[0-9a-f]{64}", expected):
        raise RuntimeError("The signing asset digest is not a SHA-256 pin.")
    asset = (asset_root / name).resolve(strict=True)
    actual = sha256(asset)
    if actual != expected:
        raise RuntimeError("The supplied public signing asset does not match the source digest pin.")
    return {"file": name, "path": str(asset), "sha256": actual,
            "expected_sha256": expected, "public_source_url": constant(client, "SIGNING_URL")}


def kill_owned_group(process):
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


def run_logged(command, source, environment, stdout_path, stderr_path, timeout):
    process = subprocess.Popen(command, cwd=source, env=environment,
                               stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE if stderr_path else subprocess.STDOUT,
                               text=True, start_new_session=True)
    failure = None
    stdout = stderr = ""
    try:
        try:
            stdout, stderr = process.communicate(timeout=timeout)
        except BaseException as error:
            failure = error
            kill_owned_group(process)
            stdout, stderr = process.communicate()
    finally:
        # A successful parent must not leave fixture descendants in its owned group.
        kill_owned_group(process)
        stdout_path.write_text(stdout or "", encoding="utf-8")
        if stderr_path:
            stderr_path.write_text(stderr or "", encoding="utf-8")
    if failure:
        raise failure
    return subprocess.CompletedProcess(command, process.returncode, stdout, stderr)


def interruption(signum, _frame):
    raise InterruptedError(f"The verification runner received signal {signum}.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--signer", choices=("fixture", "native"), default="fixture")
    parser.add_argument("--native-assets", type=Path)
    parser.add_argument("--token-latency-ms", type=int, default=20)
    parser.add_argument("--repetitions", type=int, default=4)
    arguments = parser.parse_args()
    if sys.platform != "linux":
        parser.error("This verification runner requires the designated remote Linux environment.")
    if not 0 <= arguments.token_latency_ms <= 10_000:
        parser.error("--token-latency-ms must be between zero and 10000.")
    if not 2 <= arguments.repetitions <= 20 or arguments.repetitions % 2:
        parser.error("--repetitions must be an even number between two and twenty for balanced ordering.")
    if arguments.signer == "native" and not arguments.native_assets:
        parser.error("--native-assets is required for the pinned native signing mode.")
    if arguments.signer == "fixture" and arguments.native_assets:
        parser.error("--native-assets is only accepted with --signer native.")
    source = arguments.source_root.resolve(strict=True)
    runner = Path(__file__).resolve()
    target = arguments.target_dir.resolve()
    output = arguments.output_dir.resolve()
    if output == source or source in output.parents:
        parser.error("--output-dir must be outside the source tree to preserve its input manifest.")
    output.mkdir(parents=True, exist_ok=False)
    signal.signal(signal.SIGTERM, interruption)
    environment = dict(os.environ, CARGO_TARGET_DIR=str(target),
                       CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS") or "12",
                       CARGO_NET_OFFLINE="true")
    environment.setdefault("RUST_TEST_THREADS", "1")
    command = ["cargo", "test", "--locked", "--offline", "--test",
               "download_batching", "--no-run", "--message-format=json"]
    report = {"passed": False, "evidence_kind": "synthetic_download_batching_benchmark",
              "physical_device": False, "live_account": False, "live_service_verified": False,
              "actual_network_speed_measured": False, "production_app_click_flow": False,
              "signer_mode": arguments.signer,
              "signer": "native_crypto_pinned_wasm" if arguments.signer == "native" else "injected_fixture",
              "native_signature_path_measured": arguments.signer == "native",
              "network_latency": "controlled_transport_with_injected_token_and_cdn_response_delays",
              "image_payload": "synthetic_plain_png", "encrypted_content_decode_verified": False,
              "token_latency_ms": arguments.token_latency_ms, "repetitions": arguments.repetitions,
              "source_root": str(source), "runner_sha256": sha256(runner),
              "commands": {"build": command}, "build_timeout_seconds": 600}
    try:
        manifest = source_manifest(source)
        (output / "source-inputs.sha256").write_text(manifest, encoding="utf-8")
        report["source_manifest_sha256"] = hashlib.sha256(manifest.encode()).hexdigest()
        native_root = None
        if arguments.signer == "native":
            native_root = arguments.native_assets.resolve(strict=True)
            if not native_root.is_dir():
                raise RuntimeError("--native-assets must identify a directory containing the pinned public WASM.")
            report["native_asset"] = native_asset_identity(source, native_root)
            report["native_asset_acquisition"] = "preprovisioned_public_pinned_asset; no download by runner"
        build = run_logged(command, source, environment, output / "build-events.jsonl",
                           output / "build.log", timeout=600)
        report["build_exit_code"] = build.returncode
        if build.returncode:
            raise RuntimeError("The integration test build failed; inspect build.log.")
        executables = set()
        for line in build.stdout.splitlines():
            event = json.loads(line)
            if (event.get("reason") == "compiler-artifact"
                    and event.get("target", {}).get("name") == "download_batching"
                    and event.get("executable")):
                executables.add(Path(event["executable"]).resolve(strict=True))
        if len(executables) != 1:
            raise RuntimeError("The build did not identify exactly one download_batching test executable.")
        binary = executables.pop()
        report["test_binary_path"] = str(binary)
        report["test_binary_sha256"] = sha256(binary)
        if source_manifest(source) != manifest or sha256(runner) != report["runner_sha256"]:
            raise RuntimeError("Source inputs or the runner changed while the test executable was being built.")
        if native_root and native_asset_identity(source, native_root) != report["native_asset"]:
            raise RuntimeError("The native signing asset changed while the executable was being built.")
        if not shutil.which("unshare"):
            raise RuntimeError("unshare is required; the fixture must run without host or service networking.")
        for key in list(environment):
            if key.startswith("BILICOMICS_BATCH_"):
                del environment[key]
        environment.update({"BILICOMICS_BATCH_OUTPUT": str(output / "fixtures"),
                            "BILICOMICS_BATCH_SIGNER": arguments.signer,
                            "BILICOMICS_BATCH_TOKEN_LATENCY_MS": str(arguments.token_latency_ms),
                            "BILICOMICS_BATCH_REPETITIONS": str(arguments.repetitions)})
        if native_root:
            environment["BILICOMICS_BATCH_NATIVE_ASSETS"] = str(native_root)
        command = ["unshare", "--net", "--", str(binary), "--exact", TEST_NAME,
                   "--nocapture", "--test-threads=1"]
        report["commands"]["test"] = command
        timeout = max(180, 90 + arguments.repetitions *
                      (45 * arguments.token_latency_ms / 1000 + (100 if native_root else 0)))
        report["test_timeout_seconds"] = timeout
        report["network_namespace"] = "unshare --net; no host or service network interfaces"
        run = run_logged(command, source, environment, output / "tests.log", None, timeout=timeout)
        report["test_exit_code"] = run.returncode
        if run.returncode:
            raise RuntimeError("The controlled benchmark failed; inspect tests.log and fixture evidence.")
        if (source_manifest(source) != manifest or sha256(binary) != report["test_binary_sha256"]
                or sha256(runner) != report["runner_sha256"]):
            raise RuntimeError("Source inputs, the runner, or the test executable changed during execution.")
        if native_root and native_asset_identity(source, native_root) != report["native_asset"]:
            raise RuntimeError("The native signing asset changed during execution.")
        result = json.loads((output / "fixtures" / "result.json").read_text(encoding="utf-8"))
        if not isinstance(result, dict) or result.get("passed") is not True:
            raise RuntimeError("The fixture did not produce a passing structured benchmark report.")
        report["benchmark"] = result
        report["passed"] = True
        return 0
    except (Exception, KeyboardInterrupt) as error:
        report["error"] = str(error) or type(error).__name__
        print(report["error"], file=sys.stderr)
        return 1
    finally:
        (output / "result.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    sys.exit(main())

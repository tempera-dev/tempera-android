#!/usr/bin/env python3
"""Execute one sealed mobile-gauntlet plan on an Android emulator.

The harness owns execution closure and evidence collection, not benchmark truth.
It emits content-addressed raw Android artifacts for Tempera Evals to grade behind
its sealed verifier boundary. Task text is hashed in the public evidence manifest
and remains only in the input plan / model invocation.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time
from typing import Any
from urllib.parse import urlparse

PLAN_SCHEMA = "tempera.android.mobile-gauntlet-plan/v1"
EVIDENCE_SCHEMA = "tempera.android.mobile-gauntlet-evidence/v1"
SHA40 = re.compile(r"^[0-9a-f]{40}$")
SHA64 = re.compile(r"^[0-9a-f]{64}$")
RUN_ID = re.compile(r"^[A-Za-z0-9._-]{1,128}$")
PACKAGE = re.compile(r"^[A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+$")
SERIAL = re.compile(r"^emulator-[0-9]+$")
SEMVER = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[a-z0-9.-]+)?$")
TRANSPORTS = {"auto", "adb", "bridge"}
ORIENTATIONS = {"portrait": "0", "landscape": "1"}


class PlanError(ValueError):
    pass


def canonical(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def exact_object(value: Any, required: set[str], optional: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise PlanError(f"{label} must be an object")
    unknown = set(value) - required - optional
    missing = required - set(value)
    if unknown or missing:
        raise PlanError(f"{label} fields drifted (missing={sorted(missing)}, unknown={sorted(unknown)})")
    return value


def nonempty(value: Any, label: str, limit: int) -> str:
    if not isinstance(value, str) or not value.strip() or value != value.strip() or len(value) > limit:
        raise PlanError(f"{label} must be a normalized non-empty string <= {limit} chars")
    return value


def relative_member(value: Any, label: str) -> str:
    text = nonempty(value, label, 1024)
    path = Path(text)
    if path.is_absolute() or ".." in path.parts or "\x00" in text:
        raise PlanError(f"{label} must be a bounded relative path without traversal")
    return text


def validate_plan(raw: Any) -> dict[str, Any]:
    plan = exact_object(
        raw,
        {
            "schemaVersion", "runId", "suite", "caseId", "seed", "task", "serial",
            "targetKind", "transport", "maxSteps", "approveSensitive", "networkPolicy",
            "fixtureClass", "fixtures", "setup", "bindings",
        },
        {"model"},
        "plan",
    )
    if plan["schemaVersion"] != PLAN_SCHEMA:
        raise PlanError(f"schemaVersion must be {PLAN_SCHEMA}")
    run_id = nonempty(plan["runId"], "runId", 128)
    if RUN_ID.fullmatch(run_id) is None:
        raise PlanError("runId has invalid characters")

    suite = exact_object(plan["suite"], {"id", "version"}, set(), "suite")
    nonempty(suite["id"], "suite.id", 128)
    version = nonempty(suite["version"], "suite.version", 64)
    if SEMVER.fullmatch(version) is None:
        raise PlanError("suite.version must be semantic x.y.z")
    nonempty(plan["caseId"], "caseId", 256)
    seed = plan["seed"]
    if isinstance(seed, bool) or not isinstance(seed, int) or not 0 <= seed <= 2_147_483_647:
        raise PlanError("seed must be an integer in [0, 2147483647]")
    nonempty(plan["task"], "task", 16_384)
    serial = nonempty(plan["serial"], "serial", 128)
    if SERIAL.fullmatch(serial) is None or plan["targetKind"] != "emulator":
        raise PlanError("v1 device gauntlet supports managed emulator-* targets only")
    if plan["transport"] not in TRANSPORTS:
        raise PlanError(f"transport must be one of {sorted(TRANSPORTS)}")
    max_steps = plan["maxSteps"]
    if isinstance(max_steps, bool) or not isinstance(max_steps, int) or not 1 <= max_steps <= 256:
        raise PlanError("maxSteps must be an integer in [1, 256]")
    if type(plan["approveSensitive"]) is not bool:
        raise PlanError("approveSensitive must be boolean")
    if plan["networkPolicy"] != "deny":
        raise PlanError("v1 official device plans require networkPolicy=deny")
    if plan["fixtureClass"] != "disposable-synthetic":
        raise PlanError("v1 device plans require disposable-synthetic fixtures")
    if plan["approveSensitive"] and plan["networkPolicy"] != "deny":
        raise PlanError("sensitive synthetic fixture actions require denied network")
    if "model" in plan:
        nonempty(plan["model"], "model", 256)

    fixtures = plan["fixtures"]
    if not isinstance(fixtures, list) or not 1 <= len(fixtures) <= 32:
        raise PlanError("fixtures must contain 1..32 entries")
    seen_packages: set[str] = set()
    for index, item in enumerate(fixtures):
        fixture = exact_object(item, {"package", "apkPath", "apkSha256"}, set(), f"fixtures[{index}]")
        package = nonempty(fixture["package"], f"fixtures[{index}].package", 256)
        if PACKAGE.fullmatch(package) is None or package in seen_packages:
            raise PlanError(f"fixtures[{index}].package is invalid or duplicated")
        seen_packages.add(package)
        relative_member(fixture["apkPath"], f"fixtures[{index}].apkPath")
        if not isinstance(fixture["apkSha256"], str) or SHA64.fullmatch(fixture["apkSha256"]) is None:
            raise PlanError(f"fixtures[{index}].apkSha256 must be 64 lowercase hex chars")

    setup = exact_object(
        plan["setup"],
        {"clearPackages", "orientation"},
        {"launchPackage", "launchUri", "settleMs"},
        "setup",
    )
    clear = setup["clearPackages"]
    if not isinstance(clear, list) or len(clear) > 32 or len(clear) != len(set(clear)):
        raise PlanError("setup.clearPackages must be a unique bounded array")
    for package in clear:
        if not isinstance(package, str) or package not in seen_packages:
            raise PlanError("setup.clearPackages may name only declared fixture packages")
    if setup["orientation"] not in ORIENTATIONS:
        raise PlanError("setup.orientation must be portrait or landscape")
    launch_package = setup.get("launchPackage")
    launch_uri = setup.get("launchUri")
    if (launch_package is None) == (launch_uri is None):
        raise PlanError("setup requires exactly one of launchPackage or launchUri")
    if launch_package is not None and launch_package not in seen_packages:
        raise PlanError("setup.launchPackage must name a declared fixture package")
    if launch_uri is not None:
        uri = nonempty(launch_uri, "setup.launchUri", 2048)
        parsed = urlparse(uri)
        if parsed.scheme != "tempera-fixture" or not parsed.netloc:
            raise PlanError("setup.launchUri must use the tempera-fixture:// scheme")
    settle = setup.get("settleMs", 500)
    if isinstance(settle, bool) or not isinstance(settle, int) or not 0 <= settle <= 10_000:
        raise PlanError("setup.settleMs must be in [0, 10000]")

    bindings = exact_object(
        plan["bindings"],
        {"gymSourceRevision", "gymAdapterDigest", "evalSuiteDigest", "perturbationDigest"},
        set(),
        "bindings",
    )
    if not isinstance(bindings["gymSourceRevision"], str) or SHA40.fullmatch(bindings["gymSourceRevision"]) is None:
        raise PlanError("bindings.gymSourceRevision must be a 40-hex commit")
    for field in ("gymAdapterDigest", "evalSuiteDigest", "perturbationDigest"):
        if not isinstance(bindings[field], str) or SHA64.fullmatch(bindings[field]) is None:
            raise PlanError(f"bindings.{field} must be a 64-hex digest")
    return plan


def run(argv: list[str], *, check: bool = True, timeout: int = 60) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(argv, capture_output=True, text=True, check=False, timeout=timeout)
    if check and completed.returncode != 0:
        stderr = completed.stderr.strip() or completed.stdout.strip()
        raise RuntimeError(f"command failed ({completed.returncode}): {argv[0]}: {stderr[:1000]}")
    return completed


def json_command(argv: list[str], *, check: bool = True, timeout: int = 60) -> tuple[subprocess.CompletedProcess[str], Any]:
    completed = run(argv, check=check, timeout=timeout)
    value = None
    if completed.stdout.strip():
        try:
            value = json.loads(completed.stdout)
        except json.JSONDecodeError as error:
            if check:
                raise RuntimeError(f"command returned invalid JSON: {argv[0]}: {error}") from error
    return completed, value


def write_raw(path: Path, completed: subprocess.CompletedProcess[str], parsed: Any) -> dict[str, Any]:
    value = {
        "exitCode": completed.returncode,
        "stdout": parsed if parsed is not None else completed.stdout,
        "stderr": completed.stderr,
    }
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return {"path": path.name, "sha256": sha256_file(path), "bytes": path.stat().st_size}


def adb(serial: str, *args: str, check: bool = True, timeout: int = 30) -> subprocess.CompletedProcess[str]:
    return run(["adb", "-s", serial, *args], check=check, timeout=timeout)


def android(binary: str, plan: dict[str, Any], *args: str, check: bool = True, timeout: int = 60):
    argv = [binary, "--serial", plan["serial"], "--transport", plan["transport"], *args, "--json"]
    return json_command(argv, check=check, timeout=timeout)


def setting(serial: str, namespace: str, key: str) -> str:
    return adb(serial, "shell", "settings", "get", namespace, key).stdout.strip()


def set_setting(serial: str, namespace: str, key: str, value: str) -> None:
    adb(serial, "shell", "settings", "put", namespace, key, value)


def restore_setting(serial: str, namespace: str, key: str, value: str) -> None:
    if value in {"", "null"}:
        adb(serial, "shell", "settings", "delete", namespace, key, check=False)
    else:
        set_setting(serial, namespace, key, value)


def device_facts(serial: str) -> dict[str, str]:
    keys = {
        "fingerprint": "ro.build.fingerprint",
        "sdk": "ro.build.version.sdk",
        "model": "ro.product.model",
        "manufacturer": "ro.product.manufacturer",
        "abi": "ro.product.cpu.abi",
    }
    return {name: adb(serial, "shell", "getprop", prop).stdout.strip() for name, prop in keys.items()}


def verify_fixture_files(plan_path: Path, plan: dict[str, Any]) -> list[dict[str, str]]:
    verified = []
    root = plan_path.parent.resolve()
    for fixture in plan["fixtures"]:
        path = (root / fixture["apkPath"]).resolve()
        try:
            path.relative_to(root)
        except ValueError as error:
            raise PlanError("fixture path escaped plan directory") from error
        if not path.is_file():
            raise PlanError(f"missing fixture APK: {fixture['apkPath']}")
        digest = sha256_file(path)
        if digest != fixture["apkSha256"]:
            raise PlanError(f"fixture APK digest mismatch for {fixture['package']}")
        verified.append({"package": fixture["package"], "path": str(path), "sha256": digest})
    return verified


def package_version(serial: str, package: str) -> dict[str, str | None]:
    output = adb(serial, "shell", "dumpsys", "package", package).stdout
    version_code = re.search(r"versionCode=(\d+)", output)
    version_name = re.search(r"versionName=([^\s]+)", output)
    return {
        "package": package,
        "versionCode": version_code.group(1) if version_code else None,
        "versionName": version_name.group(1) if version_name else None,
    }


def execute(plan_path: Path, output_dir: Path) -> dict[str, Any]:
    raw = json.loads(plan_path.read_text(encoding="utf-8"))
    plan = validate_plan(raw)
    if output_dir.exists():
        raise PlanError("output directory already exists; evidence is append-never")
    output_dir.mkdir(parents=True)

    binary = os.environ.get("TEMPERA_ANDROID_BIN") or shutil.which("tempera-android")
    if not binary:
        raise RuntimeError("tempera-android binary not found; set TEMPERA_ANDROID_BIN")
    if not shutil.which("adb"):
        raise RuntimeError("adb not found")
    serial = plan["serial"]
    state = adb(serial, "get-state").stdout.strip()
    if state != "device":
        raise RuntimeError(f"emulator {serial} is not ready: {state!r}")

    fixtures = verify_fixture_files(plan_path, plan)
    prior = {
        "accelerometerRotation": setting(serial, "system", "accelerometer_rotation"),
        "userRotation": setting(serial, "system", "user_rotation"),
        "wifiOn": setting(serial, "global", "wifi_on"),
        "mobileData": setting(serial, "global", "mobile_data"),
    }
    artifacts: dict[str, Any] = {}
    agent_exit = None
    installed_versions: list[dict[str, str | None]] = []
    try:
        adb(serial, "shell", "svc", "wifi", "disable")
        adb(serial, "shell", "svc", "data", "disable", check=False)
        set_setting(serial, "system", "accelerometer_rotation", "0")
        set_setting(serial, "system", "user_rotation", ORIENTATIONS[plan["setup"]["orientation"]])

        install_paths = [fixture["path"] for fixture in fixtures]
        install_completed, install_json = android(binary, plan, "app", "install", *install_paths, timeout=180)
        artifacts["install"] = write_raw(output_dir / "install.json", install_completed, install_json)
        installed_versions = [package_version(serial, fixture["package"]) for fixture in fixtures]

        for package in plan["setup"]["clearPackages"]:
            clear_completed, clear_json = android(binary, plan, "app", "clear", package)
            artifacts[f"clear:{package}"] = write_raw(
                output_dir / f"clear-{package.replace('.', '_')}.json", clear_completed, clear_json
            )

        setup = plan["setup"]
        if "launchPackage" in setup:
            launch_completed, launch_json = android(binary, plan, "app", "open", setup["launchPackage"])
        else:
            launch_completed, launch_json = android(binary, plan, "app", "deeplink", setup["launchUri"])
        artifacts["launch"] = write_raw(output_dir / "launch.json", launch_completed, launch_json)
        time.sleep(setup.get("settleMs", 500) / 1000.0)

        before_completed, before_json = android(binary, plan, "snapshot", timeout=60)
        artifacts["before"] = write_raw(output_dir / "before.json", before_completed, before_json)

        model = plan.get("model") or os.environ.get("TEMPERA_ANDROID_MODEL")
        if not model:
            raise RuntimeError("model missing from plan and TEMPERA_ANDROID_MODEL")
        run_args = ["run", plan["task"], "--model", model, "--max-steps", str(plan["maxSteps"])]
        if plan["approveSensitive"]:
            run_args.append("--approve-sensitive")
        agent_completed, agent_json = android(binary, plan, *run_args, check=False, timeout=1800)
        agent_exit = agent_completed.returncode
        artifacts["agentRun"] = write_raw(output_dir / "agent-run.json", agent_completed, agent_json)

        after_completed, after_json = android(binary, plan, "snapshot", timeout=60)
        artifacts["after"] = write_raw(output_dir / "after.json", after_completed, after_json)
        state_completed, state_json = android(binary, plan, "state", timeout=60)
        artifacts["state"] = write_raw(output_dir / "state.json", state_completed, state_json)
    finally:
        restore_setting(serial, "system", "accelerometer_rotation", prior["accelerometerRotation"])
        restore_setting(serial, "system", "user_rotation", prior["userRotation"])
        if prior["wifiOn"] == "1":
            adb(serial, "shell", "svc", "wifi", "enable", check=False)
        else:
            adb(serial, "shell", "svc", "wifi", "disable", check=False)
        if prior["mobileData"] == "1":
            adb(serial, "shell", "svc", "data", "enable", check=False)
        else:
            adb(serial, "shell", "svc", "data", "disable", check=False)

    evidence = {
        "schemaVersion": EVIDENCE_SCHEMA,
        "runId": plan["runId"],
        "suite": plan["suite"],
        "caseId": plan["caseId"],
        "seed": plan["seed"],
        "planSha256": sha256_bytes(canonical(plan)),
        "taskSha256": sha256_bytes(plan["task"].encode()),
        "bindings": plan["bindings"],
        "execution": {
            "serial": serial,
            "targetKind": plan["targetKind"],
            "transport": plan["transport"],
            "networkPolicy": plan["networkPolicy"],
            "fixtureClass": plan["fixtureClass"],
            "orientation": plan["setup"]["orientation"],
            "maxSteps": plan["maxSteps"],
            "approveSensitive": plan["approveSensitive"],
            "agentProcessExitCode": agent_exit,
        },
        "device": device_facts(serial),
        "fixtures": [
            {"package": fixture["package"], "apkSha256": fixture["sha256"]}
            for fixture in fixtures
        ],
        "installedPackageVersions": installed_versions,
        "artifacts": artifacts,
        "claimScope": "sealed Android execution evidence only; Tempera Evals owns capability grading",
        "officialResultEligible": False,
    }
    evidence_path = output_dir / "evidence.json"
    evidence_path.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return evidence


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("plan", type=Path)
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--validate-only", action="store_true")
    args = parser.parse_args()
    try:
        raw = json.loads(args.plan.read_text(encoding="utf-8"))
        plan = validate_plan(raw)
        if args.validate_only:
            print(json.dumps({
                "valid": True,
                "schemaVersion": PLAN_SCHEMA,
                "runId": plan["runId"],
                "planSha256": sha256_bytes(canonical(plan)),
                "taskSha256": sha256_bytes(plan["task"].encode()),
                "officialResultEligible": False,
            }, sort_keys=True))
            return 0
        if args.output_dir is None:
            parser.error("--output-dir is required unless --validate-only is used")
        evidence = execute(args.plan, args.output_dir)
        print(json.dumps({
            "ok": True,
            "evidence": str(args.output_dir / "evidence.json"),
            "planSha256": evidence["planSha256"],
            "agentProcessExitCode": evidence["execution"]["agentProcessExitCode"],
            "officialResultEligible": False,
        }, sort_keys=True))
        return 0
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired, json.JSONDecodeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())

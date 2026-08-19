#!/usr/bin/env python3
"""Run one sealed mobile-gauntlet plan on a disposable Android emulator.

Android owns execution closure and raw evidence. Tempera Evals owns the hidden
postcondition/effect grader. The evidence manifest contains task/artifact hashes,
not task text, and is never an official capability result by itself.
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
import time
from typing import Any
from urllib.parse import urlparse

PLAN_SCHEMA = "tempera.android.mobile-gauntlet-plan/v1"
EVIDENCE_SCHEMA = "tempera.android.mobile-gauntlet-evidence/v1"
SHA40 = re.compile(r"^[0-9a-f]{40}$")
SHA64 = re.compile(r"^[0-9a-f]{64}$")
PACKAGE = re.compile(r"^[A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+$")
SERIAL = re.compile(r"^emulator-[0-9]+$")
RUN_ID = re.compile(r"^[A-Za-z0-9._-]{1,128}$")
SEMVER = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[a-z0-9.-]+)?$")
ORIENTATION = {"portrait": "0", "landscape": "1"}


class PlanError(ValueError):
    pass


def canonical(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def digest_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def digest_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def exact(value: Any, required: set[str], optional: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise PlanError(f"{label} must be an object")
    missing = required - set(value)
    unknown = set(value) - required - optional
    if missing or unknown:
        raise PlanError(f"{label} fields drifted (missing={sorted(missing)}, unknown={sorted(unknown)})")
    return value


def text(value: Any, label: str, limit: int) -> str:
    if not isinstance(value, str) or not value or value != value.strip() or len(value) > limit:
        raise PlanError(f"{label} must be a normalized non-empty string <= {limit} chars")
    return value


def relative(value: Any, label: str) -> str:
    member = text(value, label, 1024)
    path = Path(member)
    if path.is_absolute() or ".." in path.parts or "\x00" in member:
        raise PlanError(f"{label} must be a relative path without traversal")
    return member


def validate_plan(raw: Any) -> dict[str, Any]:
    plan = exact(
        raw,
        {"schemaVersion", "runId", "suite", "caseId", "seed", "task", "serial", "targetKind", "transport", "maxSteps", "approveSensitive", "networkPolicy", "fixtureClass", "fixtures", "setup", "bindings"},
        {"model"},
        "plan",
    )
    if plan["schemaVersion"] != PLAN_SCHEMA:
        raise PlanError(f"schemaVersion must be {PLAN_SCHEMA}")
    if RUN_ID.fullmatch(text(plan["runId"], "runId", 128)) is None:
        raise PlanError("runId has invalid characters")
    suite = exact(plan["suite"], {"id", "version"}, set(), "suite")
    text(suite["id"], "suite.id", 128)
    if SEMVER.fullmatch(text(suite["version"], "suite.version", 64)) is None:
        raise PlanError("suite.version must be semantic x.y.z")
    text(plan["caseId"], "caseId", 256)
    if isinstance(plan["seed"], bool) or not isinstance(plan["seed"], int) or not 0 <= plan["seed"] <= 2_147_483_647:
        raise PlanError("seed must be an integer in [0, 2147483647]")
    text(plan["task"], "task", 16_384)
    if SERIAL.fullmatch(text(plan["serial"], "serial", 128)) is None or plan["targetKind"] != "emulator":
        raise PlanError("v1 supports disposable emulator-* targets only")
    if plan["transport"] not in {"auto", "adb", "bridge"}:
        raise PlanError("transport must be auto, adb, or bridge")
    if isinstance(plan["maxSteps"], bool) or not isinstance(plan["maxSteps"], int) or not 1 <= plan["maxSteps"] <= 40:
        raise PlanError("maxSteps must be in [1, 40], matching the compiled Android run bound")
    if type(plan["approveSensitive"]) is not bool:
        raise PlanError("approveSensitive must be boolean")
    if plan["networkPolicy"] != "deny" or plan["fixtureClass"] != "disposable-synthetic":
        raise PlanError("v1 requires networkPolicy=deny and fixtureClass=disposable-synthetic")
    if "model" in plan:
        text(plan["model"], "model", 256)

    fixtures = plan["fixtures"]
    if not isinstance(fixtures, list) or not 1 <= len(fixtures) <= 32:
        raise PlanError("fixtures must contain 1..32 entries")
    packages: set[str] = set()
    for index, raw_fixture in enumerate(fixtures):
        fixture = exact(raw_fixture, {"package", "apkPath", "apkSha256"}, set(), f"fixtures[{index}]")
        package = text(fixture["package"], f"fixtures[{index}].package", 256)
        if PACKAGE.fullmatch(package) is None or package in packages:
            raise PlanError(f"fixtures[{index}].package is invalid or duplicated")
        packages.add(package)
        relative(fixture["apkPath"], f"fixtures[{index}].apkPath")
        if not isinstance(fixture["apkSha256"], str) or SHA64.fullmatch(fixture["apkSha256"]) is None:
            raise PlanError(f"fixtures[{index}].apkSha256 must be 64 lowercase hex chars")

    setup = exact(plan["setup"], {"clearPackages", "orientation"}, {"launchPackage", "launchUri", "settleMs"}, "setup")
    clear = setup["clearPackages"]
    if not isinstance(clear, list) or len(clear) > 32 or len(clear) != len(set(clear)) or any(item not in packages for item in clear):
        raise PlanError("setup.clearPackages must be a unique subset of fixture packages")
    if setup["orientation"] not in ORIENTATION:
        raise PlanError("setup.orientation must be portrait or landscape")
    launch_package = setup.get("launchPackage")
    launch_uri = setup.get("launchUri")
    if (launch_package is None) == (launch_uri is None):
        raise PlanError("setup requires exactly one of launchPackage or launchUri")
    if launch_package is not None and launch_package not in packages:
        raise PlanError("setup.launchPackage must be a fixture package")
    if launch_uri is not None:
        parsed = urlparse(text(launch_uri, "setup.launchUri", 2048))
        if parsed.scheme != "tempera-fixture" or not parsed.netloc:
            raise PlanError("setup.launchUri must use tempera-fixture://")
    settle = setup.get("settleMs", 500)
    if isinstance(settle, bool) or not isinstance(settle, int) or not 0 <= settle <= 10_000:
        raise PlanError("setup.settleMs must be in [0, 10000]")

    bindings = exact(plan["bindings"], {"gymSourceRevision", "gymAdapterDigest", "evalSuiteDigest", "perturbationDigest"}, set(), "bindings")
    if not isinstance(bindings["gymSourceRevision"], str) or SHA40.fullmatch(bindings["gymSourceRevision"]) is None:
        raise PlanError("bindings.gymSourceRevision must be 40 lowercase hex chars")
    for name in ("gymAdapterDigest", "evalSuiteDigest", "perturbationDigest"):
        if not isinstance(bindings[name], str) or SHA64.fullmatch(bindings[name]) is None:
            raise PlanError(f"bindings.{name} must be 64 lowercase hex chars")
    return plan


def command(argv: list[str], *, check: bool = True, timeout: int = 60) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(argv, text=True, capture_output=True, check=False, timeout=timeout)
    if check and completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise RuntimeError(f"command failed ({completed.returncode}): {argv[0]}: {detail[:1200]}")
    return completed


def json_command(argv: list[str], *, check: bool = True, timeout: int = 60) -> tuple[subprocess.CompletedProcess[str], Any]:
    completed = command(argv, check=check, timeout=timeout)
    parsed = None
    if completed.stdout.strip():
        try:
            parsed = json.loads(completed.stdout)
        except json.JSONDecodeError as error:
            if check:
                raise RuntimeError(f"invalid JSON from {argv[0]}: {error}") from error
    return completed, parsed


def adb(serial: str, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return command(["adb", "-s", serial, *args], check=check, timeout=45)


def android(binary: str, plan: dict[str, Any], *args: str, check: bool = True, timeout: int = 60):
    return json_command([binary, "--serial", plan["serial"], "--transport", plan["transport"], *args, "--json"], check=check, timeout=timeout)


def write_artifact(path: Path, completed: subprocess.CompletedProcess[str], parsed: Any) -> dict[str, Any]:
    value = {"exitCode": completed.returncode, "stdout": parsed if parsed is not None else completed.stdout, "stderr": completed.stderr}
    path.write_text(json.dumps(value, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    return {"path": path.name, "sha256": digest_file(path), "bytes": path.stat().st_size}


def get_setting(serial: str, namespace: str, key: str) -> str:
    return adb(serial, "shell", "settings", "get", namespace, key).stdout.strip()


def put_setting(serial: str, namespace: str, key: str, value: str) -> None:
    adb(serial, "shell", "settings", "put", namespace, key, value)


def restore_setting(serial: str, namespace: str, key: str, value: str) -> None:
    if value in {"", "null"}:
        adb(serial, "shell", "settings", "delete", namespace, key, check=False)
    else:
        put_setting(serial, namespace, key, value)


def device_facts(serial: str) -> dict[str, str]:
    return {
        name: adb(serial, "shell", "getprop", prop).stdout.strip()
        for name, prop in {
            "fingerprint": "ro.build.fingerprint",
            "sdk": "ro.build.version.sdk",
            "model": "ro.product.model",
            "manufacturer": "ro.product.manufacturer",
            "abi": "ro.product.cpu.abi",
        }.items()
    }


def resolve_fixtures(plan_path: Path, plan: dict[str, Any]) -> list[dict[str, str]]:
    root = plan_path.parent.resolve()
    resolved = []
    for fixture in plan["fixtures"]:
        path = (root / fixture["apkPath"]).resolve()
        try:
            path.relative_to(root)
        except ValueError as error:
            raise PlanError("fixture path escaped plan directory") from error
        if not path.is_file():
            raise PlanError(f"fixture APK missing: {fixture['apkPath']}")
        observed = digest_file(path)
        if observed != fixture["apkSha256"]:
            raise PlanError(f"fixture APK digest mismatch: {fixture['package']}")
        resolved.append({"package": fixture["package"], "path": str(path), "sha256": observed})
    return resolved


def package_version(serial: str, package: str) -> dict[str, Any]:
    output = adb(serial, "shell", "dumpsys", "package", package).stdout
    code = re.search(r"versionCode=(\d+)", output)
    name = re.search(r"versionName=([^\s]+)", output)
    return {"package": package, "versionCode": code.group(1) if code else None, "versionName": name.group(1) if name else None}


def execute(plan_path: Path, output: Path) -> dict[str, Any]:
    plan = validate_plan(json.loads(plan_path.read_text(encoding="utf-8")))
    if output.exists():
        raise PlanError("output directory already exists; run evidence is append-never")
    output.mkdir(parents=True)
    binary = os.environ.get("TEMPERA_ANDROID_BIN") or shutil.which("tempera-android")
    if not binary or not shutil.which("adb"):
        raise RuntimeError("tempera-android and adb must be available")
    serial = plan["serial"]
    if adb(serial, "get-state").stdout.strip() != "device":
        raise RuntimeError(f"{serial} is not ready")
    fixtures = resolve_fixtures(plan_path, plan)
    prior = {
        "accelerometer": get_setting(serial, "system", "accelerometer_rotation"),
        "rotation": get_setting(serial, "system", "user_rotation"),
        "wifi": get_setting(serial, "global", "wifi_on"),
        "data": get_setting(serial, "global", "mobile_data"),
    }
    artifacts: dict[str, Any] = {}
    agent_exit: int | None = None
    versions: list[dict[str, Any]] = []
    try:
        adb(serial, "shell", "svc", "wifi", "disable")
        adb(serial, "shell", "svc", "data", "disable", check=False)
        put_setting(serial, "system", "accelerometer_rotation", "0")
        put_setting(serial, "system", "user_rotation", ORIENTATION[plan["setup"]["orientation"]])

        completed, parsed = android(binary, plan, "app", "install", *[item["path"] for item in fixtures], timeout=180)
        artifacts["install"] = write_artifact(output / "install.json", completed, parsed)
        versions = [package_version(serial, item["package"]) for item in fixtures]
        for package in plan["setup"]["clearPackages"]:
            completed, parsed = android(binary, plan, "app", "clear", package)
            artifacts[f"clear:{package}"] = write_artifact(output / f"clear-{package.replace('.', '_')}.json", completed, parsed)

        setup = plan["setup"]
        launch_args = ("app", "open", setup["launchPackage"]) if "launchPackage" in setup else ("app", "deeplink", setup["launchUri"])
        completed, parsed = android(binary, plan, *launch_args)
        artifacts["launch"] = write_artifact(output / "launch.json", completed, parsed)
        time.sleep(setup.get("settleMs", 500) / 1000.0)

        completed, parsed = android(binary, plan, "device", "info")
        artifacts["deviceInfo"] = write_artifact(output / "device-info.json", completed, parsed)
        completed, parsed = android(binary, plan, "eval", "--list")
        artifacts["evalCatalog"] = write_artifact(output / "eval-catalog.json", completed, parsed)
        completed, parsed = android(binary, plan, "snapshot")
        artifacts["before"] = write_artifact(output / "before.json", completed, parsed)

        model = plan.get("model") or os.environ.get("TEMPERA_ANDROID_MODEL")
        if not model:
            raise RuntimeError("model missing from plan and TEMPERA_ANDROID_MODEL")
        run_args = ["run", plan["task"], "--model", model, "--max-steps", str(plan["maxSteps"])]
        if plan["approveSensitive"]:
            run_args.append("--approve-sensitive")
        completed, parsed = android(binary, plan, *run_args, check=False, timeout=1800)
        agent_exit = completed.returncode
        artifacts["agentRun"] = write_artifact(output / "agent-run.json", completed, parsed)

        completed, parsed = android(binary, plan, "snapshot")
        artifacts["after"] = write_artifact(output / "after.json", completed, parsed)
    finally:
        restore_setting(serial, "system", "accelerometer_rotation", prior["accelerometer"])
        restore_setting(serial, "system", "user_rotation", prior["rotation"])
        adb(serial, "shell", "svc", "wifi", "enable" if prior["wifi"] == "1" else "disable", check=False)
        adb(serial, "shell", "svc", "data", "enable" if prior["data"] == "1" else "disable", check=False)

    evidence = {
        "schemaVersion": EVIDENCE_SCHEMA,
        "runId": plan["runId"],
        "suite": plan["suite"],
        "caseId": plan["caseId"],
        "seed": plan["seed"],
        "planSha256": digest_bytes(canonical(plan)),
        "taskSha256": digest_bytes(plan["task"].encode()),
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
        "fixtures": [{"package": item["package"], "apkSha256": item["sha256"]} for item in fixtures],
        "installedPackageVersions": versions,
        "artifacts": artifacts,
        "claimScope": "sealed Android execution evidence only; Tempera Evals owns capability grading",
        "officialResultEligible": False,
    }
    (output / "evidence.json").write_text(json.dumps(evidence, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    return evidence


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("plan", type=Path)
    parser.add_argument("--validate-only", action="store_true")
    parser.add_argument("--output-dir", type=Path)
    args = parser.parse_args()
    try:
        plan = validate_plan(json.loads(args.plan.read_text(encoding="utf-8")))
        if args.validate_only:
            print(json.dumps({"valid": True, "schemaVersion": PLAN_SCHEMA, "runId": plan["runId"], "planSha256": digest_bytes(canonical(plan)), "taskSha256": digest_bytes(plan["task"].encode()), "officialResultEligible": False}, sort_keys=True))
            return 0
        if args.output_dir is None:
            parser.error("--output-dir is required unless --validate-only is used")
        evidence = execute(args.plan, args.output_dir)
        print(json.dumps({"ok": True, "evidence": str(args.output_dir / "evidence.json"), "planSha256": evidence["planSha256"], "agentProcessExitCode": evidence["execution"]["agentProcessExitCode"], "officialResultEligible": False}, sort_keys=True))
        return 0
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired, json.JSONDecodeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())

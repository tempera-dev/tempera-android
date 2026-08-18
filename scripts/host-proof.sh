#!/usr/bin/env bash
# Capture live, human-owned host proof for the Tempera Android release gate.
#
# This script is deliberately not part of normal CI: it may create and delete
# one uniquely named AVD, or inspect a prepared, owner-authorized attached test
# device. It never installs the bridge, captures a screenshot, or sends input
# to an attached device. Physical bridge proof assumes that the device owner
# installed the APK and manually enabled the Accessibility service beforehand.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/host-proof.sh --mode managed --name tempera-proof-NAME [options]
  scripts/host-proof.sh --mode attached --serial SERIAL [options]

Options:
  --mode MODE          managed or attached (required)
  --name NAME          new managed AVD name; must start with tempera-proof-
  --serial SERIAL      ready attached-device serial
  --profile PROFILE    managed AVD profile (default: google)
  --api API            managed AVD API level (default: 36)
  --bridge-apk PATH    verified bridge APK; managed proof installs it
  --require-bridge     require a live bridge snapshot after setup/status
  --help               show this help

Environment:
  TEMPERA_ANDROID_BIN   canonical binary (default: target/release/tempera-android)
  TEMPERA_ANDROID_HOME  required for managed proof, to isolate managed metadata
  TEMPERA_ANDROID_ADB   adb path; otherwise discovered from PATH or SDK root
  TEMPERA_ANDROID_EMULATOR_LOG optional append-only emulator stdout/stderr log
  TEMPERA_PROOF_DATA_GB managed AVD data partition size (default: 8)
  TEMPERA_PROOF_SESSION session id to record (default: host-proof)
EOF
}

mode=""
name=""
serial=""
profile="google"
api="36"
bridge_apk=""
require_bridge=false
proof_data_gb="${TEMPERA_PROOF_DATA_GB:-8}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode) mode="${2:?missing mode}"; shift 2 ;;
    --name) name="${2:?missing name}"; shift 2 ;;
    --serial) serial="${2:?missing serial}"; shift 2 ;;
    --profile) profile="${2:?missing profile}"; shift 2 ;;
    --api) api="${2:?missing API}"; shift 2 ;;
    --bridge-apk) bridge_apk="${2:?missing APK path}"; shift 2 ;;
    --require-bridge) require_bridge=true; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ "$mode" == "managed" || "$mode" == "attached" ]] || {
  echo "--mode must be managed or attached" >&2; exit 2;
}
[[ "$proof_data_gb" =~ ^[1-9][0-9]*$ ]] || {
  echo "TEMPERA_PROOF_DATA_GB must be a positive integer" >&2; exit 2;
}

binary="${TEMPERA_ANDROID_BIN:-target/release/tempera-android}"
[[ -x "$binary" ]] || { echo "Tempera Android binary is not executable: $binary" >&2; exit 2; }
session="${TEMPERA_PROOF_SESSION:-host-proof}"

if [[ -n "${TEMPERA_ANDROID_ADB:-}" ]]; then
  adb="$TEMPERA_ANDROID_ADB"
elif command -v adb >/dev/null 2>&1; then
  adb="$(command -v adb)"
elif [[ -n "${ANDROID_SDK_ROOT:-}" && -x "$ANDROID_SDK_ROOT/platform-tools/adb" ]]; then
  adb="$ANDROID_SDK_ROOT/platform-tools/adb"
elif [[ -n "${ANDROID_HOME:-}" && -x "$ANDROID_HOME/platform-tools/adb" ]]; then
  adb="$ANDROID_HOME/platform-tools/adb"
else
  echo "ADB is required; set TEMPERA_ANDROID_ADB or ANDROID_SDK_ROOT" >&2
  exit 2
fi

run() {
  "$binary" --session "$session" --json "$@"
}

run_target() {
  "$binary" --session "$session" --serial "$serial" --transport adb --json "$@"
}

managed_created=false
cleanup() {
  if [[ "$mode" == "managed" && "$managed_created" == true ]]; then
    if [[ -n "$serial" ]]; then
      run_target device stop >/dev/null 2>&1 || true
    fi
    run device delete "$name" --yes >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT INT TERM

ready_emulator() {
  "$adb" devices | awk '$1 ~ /^emulator-[0-9]+$/ && $2 == "device" {print $1}'
}

wait_for_single_emulator() {
  local deadline candidates count
  deadline=$((SECONDS + 300))
  while (( SECONDS < deadline )); do
    candidates="$(ready_emulator)"
    count="$(printf '%s\n' "$candidates" | awk 'NF {count++} END {print count + 0}')"
    if [[ "$count" == "1" ]]; then
      printf '%s\n' "$candidates"
      return 0
    fi
    sleep 2
  done
  echo "Expected exactly one ready emulator after managed start; found: $(ready_emulator | tr '\n' ' ')" >&2
  return 1
}

wait_for_android_ui() {
  local target="$1" deadline boot_completed window_service
  deadline=$((SECONDS + 300))
  while (( SECONDS < deadline )); do
    boot_completed="$("$adb" -s "$target" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r' || true)"
    window_service="$("$adb" -s "$target" shell service check window 2>/dev/null | tr -d '\r' || true)"
    if [[ "$boot_completed" == "1" && "$window_service" == *"found"* ]]; then
      return 0
    fi
    sleep 2
  done
  # The managed AVD is still live at this point, so record compact failure
  # evidence before the cleanup trap stops and deletes it. The JSONL artifact
  # remains useful even when a host cannot complete the release gate.
  echo "Android UI readiness diagnostics for $target:" >&2
  "$adb" -s "$target" shell getprop >&2 || true
  "$adb" -s "$target" shell service list 2>&1 | sed -n '1,120p' >&2 || true
  printf '{"proof":"failed","stage":"android_ui_ready","serial":"%s","bootCompleted":"%s","windowService":"%s"}\n' \
    "$target" "$boot_completed" "$window_service"
  echo "Android UI services did not become ready on $target within 300 seconds" >&2
  return 1
}

if [[ "$mode" == "managed" ]]; then
  [[ -n "${TEMPERA_ANDROID_HOME:-}" ]] || {
    echo "Managed proof requires an explicit TEMPERA_ANDROID_HOME" >&2; exit 2;
  }
  [[ "$name" =~ ^tempera-proof-[A-Za-z0-9._-]+$ ]] || {
    echo "Managed proof names must start with tempera-proof-" >&2; exit 2;
  }
  [[ ! -e "$TEMPERA_ANDROID_HOME/devices/$name.json" ]] || {
    echo "Refusing to overwrite existing managed proof metadata: $name" >&2; exit 2;
  }
  run device create --name "$name" --profile "$profile" --api "$api" --data-gb "$proof_data_gb"
  managed_created=true
  run device start "$name" --cold --headless
  serial="$(wait_for_single_emulator)"
  "$adb" -s "$serial" wait-for-device
  wait_for_android_ui "$serial"
elif [[ "$serial" =~ ^emulator- ]]; then
  echo "Attached proof is reserved for a physical or remote test device, not $serial" >&2
  exit 2
fi

[[ -n "$serial" ]] || { echo "--serial is required for attached proof" >&2; exit 2; }
run doctor
run_target device info
run_target snapshot

if [[ "$mode" == "managed" && -n "$bridge_apk" ]]; then
  [[ -f "$bridge_apk" ]] || { echo "Bridge APK not found: $bridge_apk" >&2; exit 2; }
  run_target bridge setup --apk "$bridge_apk"
fi

if [[ "$require_bridge" == true ]]; then
  bridge_ready=false
  for _ in $(seq 1 10); do
    bridge_status="$(run_target bridge status)"
    printf '%s\n' "$bridge_status"
    if grep -q '"enabled": true' <<<"$bridge_status" && grep -q '"reachable": true' <<<"$bridge_status"; then
      bridge_ready=true
      break
    fi
    sleep 1
  done
  [[ "$bridge_ready" == true ]] || {
    echo "Native bridge did not become enabled and reachable after setup" >&2
    exit 1
  }
  "$binary" --session "$session" --serial "$serial" --transport bridge --json snapshot
fi

if [[ "$mode" == "managed" ]]; then
  # Reset only the unique record created above, then prove the replacement AVD
  # can boot and provide semantic state before cleanup deletes it.
  run_target device stop
  serial=""
  run device reset "$name" --yes
  run device start "$name" --cold --headless
  serial="$(wait_for_single_emulator)"
  "$adb" -s "$serial" wait-for-device
  wait_for_android_ui "$serial"
  run_target snapshot
fi

run_target close
echo "{\"proof\":\"passed\",\"mode\":\"$mode\",\"serial\":\"$serial\",\"session\":\"$session\"}"

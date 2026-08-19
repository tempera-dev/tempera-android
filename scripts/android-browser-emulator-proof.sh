#!/usr/bin/env bash
set -euo pipefail

apk="android-browser/app/build/outputs/apk/debug/app-debug.apk"
package="dev.tempera.android.browser"
activity="dev.tempera.android.browser.TemperaBrowserActivity"
port=7433

cleanup() {
  adb forward --remove "tcp:$port" >/dev/null 2>&1 || true
  adb shell am force-stop "$package" >/dev/null 2>&1 || true
}

diagnostics() {
  echo '--- Android browser control diagnostics ---' >&2
  adb forward --list >&2 || true
  adb shell dumpsys activity activities | grep -A8 -B2 "$package" >&2 || true
  adb logcat -d -v brief TemperaBrowser:D AndroidRuntime:E '*:S' >&2 || true
}

trap cleanup EXIT

[[ -f "$apk" ]]
adb install -r "$apk" >/dev/null
adb shell am force-stop "$package"
adb logcat -c || true
adb shell am start -W -n "$package/$activity" >/dev/null
adb forward --remove "tcp:$port" >/dev/null 2>&1 || true
adb forward "tcp:$port" "tcp:$port" >/dev/null

browser_token=""
for _ in $(seq 1 60); do
  browser_token="$(adb shell run-as "$package" cat files/control-token 2>/dev/null | tr -d '\r\n' || true)"
  if [[ "$browser_token" =~ ^[0-9a-f]{64}$ ]]; then
    break
  fi
  sleep 0.25
done
if [[ ! "$browser_token" =~ ^[0-9a-f]{64}$ ]]; then
  diagnostics
  echo 'browser control token was not created' >&2
  exit 1
fi

request() {
  local method="$1"
  local path="$2"
  local body="${3:-}"
  local common=(
    --fail
    --silent
    --show-error
    --http1.1
    --connect-timeout 2
    --max-time 8
    --request "$method"
    --header "Authorization: Bearer $browser_token"
  )
  if [[ -n "$body" ]]; then
    curl "${common[@]}" \
      --header 'Content-Type: application/json' \
      --data "$body" \
      "http://127.0.0.1:$port$path"
  else
    curl "${common[@]}" "http://127.0.0.1:$port$path"
  fi
}

health_ok=0
for _ in $(seq 1 40); do
  if request GET /v1/health > /tmp/tempera-browser-health.json 2>/tmp/tempera-browser-health.err; then
    health_ok=1
    break
  fi
  sleep 0.25
done
if [[ "$health_ok" -ne 1 ]]; then
  cat /tmp/tempera-browser-health.err >&2 || true
  diagnostics
  exit 1
fi
jq -e '.ok == true and .primaryTransport == "instrumented-webview-dom"' \
  /tmp/tempera-browser-health.json >/dev/null

request GET /v1/snapshot > /tmp/tempera-browser-before.json
jq -e '
  .schemaVersion == "tempera.android.browser.snapshot/v1" and
  (.documentStateHash | startswith("fnv1a64:")) and
  (.revision | type == "number") and
  (.nodes | type == "array") and
  .trustedForConsequentialActions == false
' /tmp/tempera-browser-before.json >/dev/null

before_hash="$(jq -r '.documentStateHash' /tmp/tempera-browser-before.json)"
stale_body="$(jq -cn '{kind:"tap",ref:"@d1",expectedStateHash:"fnv1a64:0000000000000000"}')"
request POST /v1/action "$stale_body" > /tmp/tempera-browser-stale.json
jq -e '.ok == false and .stale == true' /tmp/tempera-browser-stale.json >/dev/null
request GET /v1/snapshot > /tmp/tempera-browser-after.json
after_hash="$(jq -r '.documentStateHash' /tmp/tempera-browser-after.json)"
[[ "$before_hash" == "$after_hash" ]]

BROWSER_TOKEN="$browser_token" python3 - <<'PY'
import json
import os
import statistics
import time
import urllib.request

port = 7433
token = os.environ["BROWSER_TOKEN"]
latencies = []
last = None
for _ in range(30):
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/v1/snapshot",
        headers={"Authorization": f"Bearer {token}"},
    )
    started = time.perf_counter_ns()
    with urllib.request.urlopen(request, timeout=5) as response:
        last = json.load(response)
    latencies.append((time.perf_counter_ns() - started) / 1_000_000)
ordered = sorted(latencies)
result = {
    "schemaVersion": "tempera.android.browser.benchmark/v1",
    "samples": len(ordered),
    "snapshotMs": {
        "min": ordered[0],
        "mean": statistics.fmean(ordered),
        "p50": ordered[len(ordered) // 2],
        "p95": ordered[min(len(ordered) - 1, int(len(ordered) * 0.95))],
        "max": ordered[-1],
    },
    "lastRevision": last["revision"],
    "lastStateHash": last["documentStateHash"],
    "nodeCount": len(last["nodes"]),
    "claimScope": "single GitHub-hosted Linux emulator fixture",
}
print(json.dumps(result, sort_keys=True))
PY

#!/usr/bin/env bash
set -euo pipefail

apk="android-browser/app/build/outputs/apk/debug/app-debug.apk"
package="dev.tempera.android.browser"
activity="dev.tempera.android.browser.TemperaBrowserActivity"
port=7433

[[ -f "$apk" ]]
adb install -r "$apk" >/dev/null
adb shell am force-stop "$package"
adb shell am start -n "$package/$activity" >/dev/null
adb forward "tcp:$port" "tcp:$port" >/dev/null

browser_token=""
for _ in $(seq 1 60); do
  browser_token="$(adb shell run-as "$package" cat files/control-token 2>/dev/null | tr -d '\r\n' || true)"
  if [[ "$browser_token" =~ ^[0-9a-f]{64}$ ]]; then
    break
  fi
  sleep 0.25
done
[[ "$browser_token" =~ ^[0-9a-f]{64}$ ]]

request() {
  local method="$1"
  local path="$2"
  local body="${3:-}"
  if [[ -n "$body" ]]; then
    curl --fail --silent --show-error \
      --request "$method" \
      --header "Authorization: Bearer $browser_token" \
      --header 'Content-Type: application/json' \
      --data "$body" \
      "http://127.0.0.1:$port$path"
  else
    curl --fail --silent --show-error \
      --request "$method" \
      --header "Authorization: Bearer $browser_token" \
      "http://127.0.0.1:$port$path"
  fi
}

for _ in $(seq 1 40); do
  if request GET /v1/health > /tmp/tempera-browser-health.json 2>/dev/null; then
    break
  fi
  sleep 0.25
done
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

adb shell am force-stop "$package"
adb forward --remove "tcp:$port" >/dev/null

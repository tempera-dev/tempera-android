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
import http.client
import json
import os
import statistics
import time

PORT = 7433
TOKEN = os.environ["BROWSER_TOKEN"]
WARMUP = 10
SAMPLES = 80


def percentile(values, fraction):
    ordered = sorted(values)
    index = round((len(ordered) - 1) * fraction)
    return ordered[max(0, min(index, len(ordered) - 1))]


def summarize(values):
    ordered = sorted(values)
    return {
        "min": ordered[0],
        "mean": statistics.fmean(ordered),
        "p50": percentile(ordered, 0.50),
        "p90": percentile(ordered, 0.90),
        "p95": percentile(ordered, 0.95),
        "p99": percentile(ordered, 0.99),
        "max": ordered[-1],
    }


def summarize_bytes(values):
    ordered = sorted(values)
    return {
        "min": ordered[0],
        "p50": percentile(ordered, 0.50),
        "p95": percentile(ordered, 0.95),
        "max": ordered[-1],
    }


connection = http.client.HTTPConnection("127.0.0.1", PORT, timeout=5)
request_count = 0


def call(method, path, body=None):
    global request_count
    payload = None if body is None else json.dumps(body, separators=(",", ":"))
    headers = {
        "Authorization": f"Bearer {TOKEN}",
        "Accept": "application/json",
        "Connection": "keep-alive",
    }
    if payload is not None:
        headers["Content-Type"] = "application/json"
    started = time.perf_counter_ns()
    connection.request(method, path, body=payload, headers=headers)
    response = connection.getresponse()
    raw = response.read()
    elapsed_ms = (time.perf_counter_ns() - started) / 1_000_000
    request_count += 1
    if response.status != 200:
        raise RuntimeError(f"browser returned HTTP {response.status}: {raw!r}")
    value = json.loads(raw)
    if value.get("ok") is False:
        raise RuntimeError(f"browser operation failed: {value}")
    return elapsed_ms, len(raw), value


for _ in range(WARMUP):
    call("GET", "/v1/snapshot")

full_latencies = []
full_bytes = []
last_full = None
for _ in range(SAMPLES):
    elapsed, size, last_full = call("GET", "/v1/snapshot")
    full_latencies.append(elapsed)
    full_bytes.append(size)

state_hash = last_full["documentStateHash"]
delta_latencies = []
delta_bytes = []
unchanged = 0
last_delta = None
for _ in range(SAMPLES):
    elapsed, size, last_delta = call(
        "POST", "/v1/snapshot-delta", {"previousStateHash": state_hash}
    )
    delta_latencies.append(elapsed)
    delta_bytes.append(size)
    unchanged += int(last_delta.get("unchanged") is True)
    if last_delta.get("documentStateHash") not in (None, state_hash):
        raise RuntimeError("delta returned an unexpected document state hash")

connection.close()

if unchanged != SAMPLES:
    raise RuntimeError(f"expected {SAMPLES} unchanged deltas, got {unchanged}")
if request_count != WARMUP + SAMPLES + SAMPLES:
    raise RuntimeError(f"unexpected benchmark request count: {request_count}")

result = {
    "schemaVersion": "tempera.android.browser.hotpath-benchmark/v1",
    "transport": "single HTTP/1.1 keep-alive connection over one ADB forward",
    "samplesPerMode": SAMPLES,
    "warmup": WARMUP,
    "requestCountOnConnection": request_count,
    "serverConnectionRequestLimit": 256,
    "fullSnapshotMs": summarize(full_latencies),
    "unchangedDeltaMs": summarize(delta_latencies),
    "payloadBytes": {
        "fullSnapshot": summarize_bytes(full_bytes),
        "unchangedDelta": summarize_bytes(delta_bytes),
    },
    "unchangedDeltaRate": unchanged / SAMPLES,
    "lastRevision": last_full["revision"],
    "lastStateHash": state_hash,
    "nodeCount": len(last_full["nodes"]),
    "claimScope": "single GitHub-hosted Linux Android emulator fixture; no universal multiplier claim",
    "validity": {
        "samePage": True,
        "sameConnection": True,
        "allDeltasUnchanged": True,
        "universalMultiplierClaim": False,
    },
}
with open("android-browser-benchmark.json", "w", encoding="utf-8") as output:
    json.dump(result, output, indent=2, sort_keys=True)
    output.write("\n")
print(json.dumps(result, sort_keys=True))
PY

#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="$ROOT/companion"
BUILD="$APP/build"
ANDROID_SDK_ROOT="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
COMPILE_SDK="${TEMPERA_ANDROID_COMPILE_SDK:-36}"

if [[ ! -d "$ANDROID_SDK_ROOT" ]]; then
  echo "Android SDK not found at $ANDROID_SDK_ROOT" >&2
  exit 2
fi

latest_dir() {
  python3 - "$1" <<'PY'
from pathlib import Path
import re, sys
root = Path(sys.argv[1])
items = [p for p in root.iterdir() if p.is_dir()] if root.is_dir() else []
def key(p):
    nums = tuple(int(x) for x in re.findall(r"\d+", p.name))
    return nums, p.name
if not items:
    raise SystemExit(2)
print(max(items, key=key))
PY
}

BUILD_TOOLS="$(latest_dir "$ANDROID_SDK_ROOT/build-tools")"
PLATFORM="$ANDROID_SDK_ROOT/platforms/android-$COMPILE_SDK"
AAPT2="$BUILD_TOOLS/aapt2"
D8="$BUILD_TOOLS/d8"
ZIPALIGN="$BUILD_TOOLS/zipalign"
APKSIGNER="$BUILD_TOOLS/apksigner"
ANDROID_JAR="$PLATFORM/android.jar"

for tool in "$AAPT2" "$D8" "$ZIPALIGN" "$APKSIGNER" "$ANDROID_JAR"; do
  if [[ ! -e "$tool" ]]; then
    echo "Required Android build tool missing: $tool" >&2
    exit 2
  fi
done
for tool in javac jar keytool zip; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Required build command missing: $tool" >&2
    exit 2
  fi
done

rm -rf "$BUILD"
mkdir -p "$BUILD/compiled" "$BUILD/gen" "$BUILD/classes" "$BUILD/dex"

"$AAPT2" compile --dir "$APP/res" -o "$BUILD/compiled"

resources=()
while IFS= read -r -d '' resource; do
  resources+=("$resource")
done < <(find "$BUILD/compiled" -type f -name '*.flat' -print0)
if [[ ${#resources[@]} -eq 0 ]]; then
  echo "No compiled Android resources found" >&2
  exit 2
fi

"$AAPT2" link \
  -o "$BUILD/resources.apk" \
  -I "$ANDROID_JAR" \
  --manifest "$APP/AndroidManifest.xml" \
  --java "$BUILD/gen" \
  --min-sdk-version 30 \
  --target-sdk-version "$COMPILE_SDK" \
  --version-code 4 \
  --version-name 0.4.0-alpha.1 \
  "${resources[@]}"

java_sources=()
while IFS= read -r -d '' source; do
  java_sources+=("$source")
done < <(find "$APP/src" "$BUILD/gen" -type f -name '*.java' -print0)

javac \
  -encoding UTF-8 \
  -source 17 \
  -target 17 \
  -classpath "$ANDROID_JAR" \
  -d "$BUILD/classes" \
  "${java_sources[@]}"

jar cf "$BUILD/classes.jar" -C "$BUILD/classes" .
"$D8" --min-api 30 --lib "$ANDROID_JAR" --output "$BUILD/dex" "$BUILD/classes.jar"
cp "$BUILD/resources.apk" "$BUILD/with-dex.apk"
zip -q -j -u "$BUILD/with-dex.apk" "$BUILD/dex/classes.dex"
"$ZIPALIGN" -f 4 "$BUILD/with-dex.apk" "$BUILD/aligned.apk"

KEYSTORE="$BUILD/debug.keystore"
keytool -genkeypair \
  -keystore "$KEYSTORE" \
  -storepass android \
  -keypass android \
  -alias androiddebugkey \
  -dname "CN=Tempera Android Bridge,O=Tempera,C=US" \
  -keyalg RSA \
  -keysize 2048 \
  -validity 10000 \
  >/dev/null 2>&1

OUTPUT="$BUILD/tempera-android-bridge.apk"
"$APKSIGNER" sign \
  --ks "$KEYSTORE" \
  --ks-pass pass:android \
  --key-pass pass:android \
  --out "$OUTPUT" \
  "$BUILD/aligned.apk"
"$APKSIGNER" verify --verbose "$OUTPUT" >/dev/null

echo "$OUTPUT"

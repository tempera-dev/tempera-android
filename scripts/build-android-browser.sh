#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
project="$repo_root/android-browser"

if command -v gradle >/dev/null 2>&1; then
  gradle_bin="$(command -v gradle)"
elif [[ -x "$project/gradlew" ]]; then
  gradle_bin="$project/gradlew"
else
  printf 'error: Gradle is required; install it or add a verified wrapper\n' >&2
  exit 2
fi

"$gradle_bin" --no-daemon -p "$project" :app:assembleDebug
apk="$project/app/build/outputs/apk/debug/app-debug.apk"
[[ -f "$apk" ]]
printf '%s\n' "$apk"

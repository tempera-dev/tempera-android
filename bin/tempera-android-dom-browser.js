#!/usr/bin/env node
'use strict';

const { existsSync } = require('node:fs');
const { join } = require('node:path');
const { spawnSync } = require('node:child_process');

const platform = process.platform === 'darwin' ? 'macos' : process.platform;
const arch = process.arch === 'arm64' ? 'arm64' : process.arch === 'x64' ? 'x64' : process.arch;
const executable = process.platform === 'win32' ? 'tempera-android-dom-browser.exe' : 'tempera-android-dom-browser';
const binary = join(__dirname, '..', 'native', `${platform}-${arch}`, executable);
const bundledApk = join(__dirname, '..', 'assets', 'tempera-android-browser.apk');

if (!existsSync(binary)) {
  process.stderr.write(`tempera-android-dom-browser has no bundled binary for ${platform}-${arch}.\n`);
  process.exit(1);
}
const env = { ...process.env };
if (!env.TEMPERA_ANDROID_BROWSER_APK && existsSync(bundledApk)) {
  env.TEMPERA_ANDROID_BROWSER_APK = bundledApk;
}
const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit', env });
process.exit(result.status === null ? 1 : result.status);

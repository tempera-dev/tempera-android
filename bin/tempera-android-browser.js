#!/usr/bin/env node
'use strict';

const { existsSync } = require('node:fs');
const { join } = require('node:path');
const { spawnSync } = require('node:child_process');

const platform = process.platform === 'darwin' ? 'macos' : process.platform;
const arch = process.arch === 'arm64' ? 'arm64' : process.arch === 'x64' ? 'x64' : process.arch;
const executable = process.platform === 'win32' ? 'tempera-android-browser.exe' : 'tempera-android-browser';
const binary = join(__dirname, '..', 'native', `${platform}-${arch}`, executable);

if (!existsSync(binary)) {
  process.stderr.write(`tempera-android-browser has no bundled binary for ${platform}-${arch}.\n`);
  process.exit(1);
}
const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit' });
process.exit(result.status === null ? 1 : result.status);

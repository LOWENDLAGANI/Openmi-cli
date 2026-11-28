#!/usr/bin/env node
"use strict";

const { spawnSync } = require("child_process");

const PLATFORMS = {
  "linux-x64": { pkg: "@minetallestmc/linux-x64", bin: "bin/openmi" },
  "linux-arm64": { pkg: "@minetallestmc/linux-arm64", bin: "bin/openmi" },
  "darwin-arm64": { pkg: "@minetallestmc/darwin-arm64", bin: "bin/openmi" },
  "win32-x64": { pkg: "@minetallestmc/win32-x64", bin: "bin/openmi.exe" },
};

const key = `${process.platform}-${process.arch}`;
const target = PLATFORMS[key];

if (!target) {
  console.error(
    `openmi: unsupported platform "${key}".\n` +
      `Supported npm platforms: ${Object.keys(PLATFORMS).join(", ")}.\n` +
      `See https://github.com/LOWENDLAGANI/openmi-release for other installation options.`
  );
  process.exit(1);
}

let binaryPath;
try {
  binaryPath = require.resolve(`${target.pkg}/${target.bin}`);
} catch (err) {
  console.error(
    `openmi: could not find prebuilt binary package "${target.pkg}".\n` +
      `This usually happens if optional dependencies were skipped (e.g. --no-optional).\n` +
      `Try reinstalling with: npm install -g openmi`
  );
  process.exit(1);
}

const result = spawnSync(binaryPath, process.argv.slice(2), {
  stdio: "inherit",
});

if (result.error) {
  console.error(`openmi: failed to execute binary: ${result.error.message}`);
  process.exit(1);
}

if (result.signal) {
  process.kill(process.pid, result.signal);
}

process.exit(result.status ?? 0);

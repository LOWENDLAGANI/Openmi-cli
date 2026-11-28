#!/usr/bin/env node
/**
 * Prepares the main `openmi` npm package and the 4 `@minetallestmc/*` platform
 * packages inside `npm/dist/` using prebuilt binaries from CI artifacts.
 *
 * Usage:
 *   node npm/scripts/prepare-packages.mjs --version 0.1.0 --artifacts ./artifacts
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const NPM_ROOT = path.resolve(__dirname, "..");
const DIST_ROOT = path.join(NPM_ROOT, "dist");

const TARGETS = [
  {
    pkgName: "@minetallestmc/linux-x64",
    dirName: "linux-x64",
    artifactDir: "bin-linux-x64",
    os: ["linux"],
    cpu: ["x64"],
    binName: "openmi",
  },
  {
    pkgName: "@minetallestmc/linux-arm64",
    dirName: "linux-arm64",
    artifactDir: "bin-linux-arm64",
    os: ["linux"],
    cpu: ["arm64"],
    binName: "openmi",
  },
  {
    pkgName: "@minetallestmc/darwin-arm64",
    dirName: "darwin-arm64",
    artifactDir: "bin-darwin-arm64",
    os: ["darwin"],
    cpu: ["arm64"],
    binName: "openmi",
  },
  {
    pkgName: "@minetallestmc/win32-x64",
    dirName: "win32-x64",
    artifactDir: "bin-win32-x64",
    os: ["win32"],
    cpu: ["x64"],
    binName: "openmi.exe",
  },
];

function parseArgs(argv) {
  const out = {};
  for (let i = 2; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--version") out.version = argv[++i];
    else if (arg === "--artifacts") out.artifacts = argv[++i];
  }
  return out;
}

const { version: rawVersion, artifacts } = parseArgs(process.argv);
if (!rawVersion || !artifacts) {
  console.error("Usage: node prepare-packages.mjs --version <X.Y.Z> --artifacts <dir>");
  process.exit(1);
}

const version = rawVersion.replace(/^v/, "");
const artifactsDir = path.resolve(artifacts);

fs.rmSync(DIST_ROOT, { recursive: true, force: true });
fs.mkdirSync(DIST_ROOT, { recursive: true });

// 1. Build each platform package
for (const t of TARGETS) {
  const srcBin = path.join(artifactsDir, t.artifactDir, t.binName);
  if (!fs.existsSync(srcBin)) {
    throw new Error(`Missing built binary: ${srcBin}`);
  }

  const pkgDir = path.join(DIST_ROOT, t.dirName);
  const binDir = path.join(pkgDir, "bin");
  fs.mkdirSync(binDir, { recursive: true });

  const destBin = path.join(binDir, t.binName);
  fs.copyFileSync(srcBin, destBin);
  if (!t.binName.endsWith(".exe")) {
    fs.chmodSync(destBin, 0o755);
  }

  const pkgJson = {
    name: t.pkgName,
    version,
    description: `Prebuilt Openmi CLI binary for ${t.dirName}`,
    homepage: "https://github.com/LOWENDLAGANI/Openmi-cli",
    repository: {
      type: "git",
      url: "https://github.com/LOWENDLAGANI/Openmi-cli.git",
    },
    license: "MIT",
    os: t.os,
    cpu: t.cpu,
    files: ["bin/"],
    publishConfig: {
      access: "public",
    },
  };

  fs.writeFileSync(
    path.join(pkgDir, "package.json"),
    JSON.stringify(pkgJson, null, 2) + "\n"
  );
  console.log(`Prepared ${t.pkgName}@${version}`);
}

// 2. Copy & stamp the main `openmi` wrapper package
const mainSrc = path.join(NPM_ROOT, "openmi");
const mainDest = path.join(DIST_ROOT, "openmi");
fs.cpSync(mainSrc, mainDest, { recursive: true });

const mainPkgPath = path.join(mainDest, "package.json");
const mainPkg = JSON.parse(fs.readFileSync(mainPkgPath, "utf8"));
mainPkg.version = version;
mainPkg.optionalDependencies = Object.fromEntries(
  TARGETS.map((t) => [t.pkgName, version])
);
fs.writeFileSync(mainPkgPath, JSON.stringify(mainPkg, null, 2) + "\n");
fs.chmodSync(path.join(mainDest, "bin", "openmi.js"), 0o755);

console.log(`Prepared openmi@${version} in ${DIST_ROOT}`);

#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const PLATFORMS = [
  "linux-x64",
  "linux-arm64",
  "windows-x64",
  "windows-arm64",
  "macos-x64",
  "macos-arm64",
];

export const BINARIES = ["cdesktop", "cdesktop-mcp", "cdesktop-review"];

export function releaseAssetName(binary, platform) {
  return `${binary}-${platform}.zip`;
}

function usage() {
  return [
    "Usage:",
    "  node scripts/github-release-assets.mjs --tag <tag> --source <dir> --out <dir>",
    "",
    "Source directory must contain <platform>/<binary>.zip for every supported platform and binary.",
  ].join("\n");
}

function parseArgs(argv) {
  const args = {};
  for (let i = 2; i < argv.length; i += 2) {
    const key = argv[i];
    const value = argv[i + 1];
    if (!key?.startsWith("--") || !value) {
      throw new Error(usage());
    }
    args[key.slice(2)] = value;
  }
  for (const key of ["tag", "source", "out"]) {
    if (!args[key]) {
      throw new Error(usage());
    }
  }
  return args;
}

function sha256(filePath) {
  return crypto
    .createHash("sha256")
    .update(fs.readFileSync(filePath))
    .digest("hex");
}

export function buildReleaseAssets({ tag, sourceDir, outDir }) {
  fs.mkdirSync(outDir, { recursive: true });

  const manifest = {
    version: tag,
    assets: {},
    platforms: {},
  };

  for (const platform of PLATFORMS) {
    manifest.platforms[platform] = {};

    for (const binary of BINARIES) {
      const sourcePath = path.join(sourceDir, platform, `${binary}.zip`);
      if (!fs.existsSync(sourcePath)) {
        throw new Error(`Missing platform artifact: ${sourcePath}`);
      }

      const assetName = releaseAssetName(binary, platform);
      const outPath = path.join(outDir, assetName);
      fs.copyFileSync(sourcePath, outPath);

      const info = {
        sha256: sha256(outPath),
        size: fs.statSync(outPath).size,
      };
      manifest.assets[assetName] = info;
      manifest.platforms[platform][binary] = info;
    }
  }

  const manifestPath = path.join(outDir, "manifest.json");
  fs.writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  return manifest;
}

const isCli = process.argv[1] === fileURLToPath(import.meta.url);
if (isCli) {
  try {
    const args = parseArgs(process.argv);
    const manifest = buildReleaseAssets({
      tag: args.tag,
      sourceDir: args.source,
      outDir: args.out,
    });
    console.log(
      `Prepared ${Object.keys(manifest.assets).length} release binary assets plus manifest.json`,
    );
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}

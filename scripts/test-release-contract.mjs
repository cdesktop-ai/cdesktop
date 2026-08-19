#!/usr/bin/env node
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";

import {
  BINARIES,
  PLATFORMS,
  buildReleaseAssets,
  releaseAssetName,
  releaseAssetNames,
} from "./github-release-assets.mjs";

const root = process.cwd();
const tmp = fs.mkdtempSync(
  path.join(os.tmpdir(), "cdesktop-release-contract-"),
);

function sha256(data) {
  return crypto.createHash("sha256").update(data).digest("hex");
}

function compileDownloader() {
  const outfile = path.join(tmp, "download.cjs");
  execFileSync(
    "npx",
    [
      "esbuild",
      "npx-cli/src/download.ts",
      "--bundle",
      "--platform=node",
      "--target=node20",
      "--format=cjs",
      `--outfile=${outfile}`,
    ],
    { cwd: root, stdio: "pipe" },
  );
  return import(pathToFileURL(outfile));
}

function writeFixtureArtifacts(sourceDir) {
  for (const platform of PLATFORMS) {
    for (const binary of BINARIES) {
      const dir = path.join(sourceDir, platform);
      fs.mkdirSync(dir, { recursive: true });
      fs.writeFileSync(
        path.join(dir, `${binary}.zip`),
        `${binary}:${platform}`,
      );
    }
  }
}

function testReleaseAssetBuilder() {
  const sourceDir = path.join(tmp, "source");
  const outDir = path.join(tmp, "release-assets");
  writeFixtureArtifacts(sourceDir);

  const manifest = buildReleaseAssets({
    tag: "v0.2.6-20260819000000",
    sourceDir,
    outDir,
  });

  assert.equal(Object.keys(manifest.assets).length, 18);
  assert.deepEqual(Object.keys(manifest).sort(), ["assets", "version"]);
  assert.deepEqual(
    Object.keys(manifest.assets).sort(),
    releaseAssetNames().sort(),
  );
  assert.equal(
    releaseAssetName("cdesktop-mcp", "linux-arm64"),
    "cdesktop-mcp-linux-arm64.zip",
  );

  for (const platform of PLATFORMS) {
    for (const binary of BINARIES) {
      const assetName = `${binary}-${platform}.zip`;
      const assetPath = path.join(outDir, assetName);
      const data = fs.readFileSync(assetPath);
      assert.deepEqual(manifest.assets[assetName], {
        sha256: sha256(data),
        size: data.length,
      });
    }
  }

  assert.ok(fs.existsSync(path.join(outDir, "manifest.json")));
}

async function testDownloaderContract() {
  const download = await compileDownloader();
  const baseUrl =
    "https://github.com/clarkipeng/cdesktop/releases/download/v0.2.6-abc/";

  assert.equal(
    download.normalizeReleaseAssetBaseUrl(baseUrl),
    "https://github.com/clarkipeng/cdesktop/releases/download/v0.2.6-abc",
  );
  assert.equal(
    download.releaseAssetUrl(baseUrl, "manifest.json"),
    "https://github.com/clarkipeng/cdesktop/releases/download/v0.2.6-abc/manifest.json",
  );
  assert.equal(
    download.releaseAssetName("cdesktop-review", "windows-x64"),
    "cdesktop-review-windows-x64.zip",
  );

  const bytes = Buffer.from("release asset");
  const filePath = path.join(tmp, "asset.zip");
  fs.writeFileSync(filePath, bytes);
  const info = { sha256: sha256(bytes), size: bytes.length };
  download.validateDownloadedFile(filePath, info);

  assert.throws(
    () => download.validateDownloadedFile(filePath, { ...info, size: 1 }),
    /Size mismatch/,
  );
  assert.throws(
    () =>
      download.validateDownloadedFile(filePath, {
        ...info,
        sha256: "0".repeat(64),
      }),
    /Checksum mismatch/,
  );

  const manifest = {
    version: "v0.2.6-abc",
    assets: {
      "cdesktop-linux-x64.zip": info,
    },
  };
  assert.deepEqual(
    download.binaryInfoForAsset(manifest, "linux-x64", "cdesktop"),
    info,
  );
  assert.equal(
    download.binaryInfoForAsset(manifest, "linux-x64", "cdesktop-mcp"),
    undefined,
  );

  const manifestJson = `${JSON.stringify(manifest, null, 2)}\n`;
  const manifestSha = sha256(manifestJson);
  assert.deepEqual(
    download.validateBinaryManifestPayload(manifestJson, manifestSha),
    manifest,
  );
  assert.throws(
    () => download.validateBinaryManifestPayload(manifestJson, "0".repeat(64)),
    /Manifest checksum mismatch/,
  );
  assert.throws(
    () => download.validateBinaryManifestPayload(manifestJson),
    /checksum was not injected/,
  );
}

function matrixNamesForJob(workflow, jobName, nextJobName) {
  const start = workflow.indexOf(`  ${jobName}:`);
  const end = workflow.indexOf(`  ${nextJobName}:`, start + 1);
  assert.notEqual(start, -1, `Missing workflow job ${jobName}`);
  assert.notEqual(end, -1, `Missing workflow job ${nextJobName}`);
  const job = workflow.slice(start, end);
  return job
    .split(/\n\s+- target: /)
    .slice(1)
    .map((entry) => {
      const match = entry.match(/\n\s+name: ([a-z0-9-]+)/);
      assert.ok(match, `Missing matrix name in ${jobName}`);
      return match[1];
    })
    .sort();
}

function testWorkflowContract() {
  const workflow = fs.readFileSync(
    path.join(root, ".github/workflows/pre-release.yml"),
    "utf8",
  );

  assert.doesNotMatch(workflow, /upload-to-r2:/);
  assert.doesNotMatch(workflow, /R2_BINARIES_/);
  assert.match(workflow, /release-assets\/\*/);
  assert.match(workflow, /__GITHUB_RELEASE_ASSET_BASE_URL__/);
  assert.match(workflow, /__BINARY_MANIFEST_SHA256__/);
  assert.match(workflow, /github-release-assets\.mjs/);
  assert.match(workflow, /needs: \[bump-version, package-npx-cli\]/);

  for (const platform of PLATFORMS) {
    for (const binary of BINARIES) {
      assert.equal(
        releaseAssetName(binary, platform),
        `${binary}-${platform}.zip`,
      );
    }
  }

  assert.deepEqual(
    matrixNamesForJob(workflow, "build-backend", "package-npx-cli"),
    [...PLATFORMS].sort(),
  );
  assert.deepEqual(
    matrixNamesForJob(workflow, "package-npx-cli", "create-prerelease"),
    [...PLATFORMS].sort(),
  );
  assert.match(
    workflow,
    /test "\$\(find release-assets -maxdepth 1 -type f -name '\*\.zip' \| wc -l\)" -eq 18/,
  );
}

try {
  testReleaseAssetBuilder();
  await testDownloaderContract();
  testWorkflowContract();
  console.log("release contract tests passed");
} finally {
  fs.rmSync(tmp, { recursive: true, force: true });
}
